use anyhow::Context;
use asterisk_ari::{AriClient, Config, apis::channels};
use std::{collections::HashMap, sync::Arc};

use bts_protocol::addons::v2::{ActionId, AddonManifest};
use bts_protocol::{EventKind, NewEvent, TelephonyTargets};
use bts_telephony::semantic_menu::render_menu;
use bts_telephony::{
    session::{CallerIdentity, MediaItem, TelephonySession},
    voice::{KokoroSynthesizer, VoiceCache, VoiceSettings},
};
use reqwest::Client;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const APPLICATION_NAME: &str = "bts";
const EVENT_SOURCE: &str = "bts-telephony";
const WELCOME_PROMPT: &str = "Welcome to Bansleben Telephone Services.";
const STATIC_SESSION_PROMPTS: &[&str] = &[
    "Configuration.",
    "Press one to change terminal.",
    "Press star to return.",
    "No terminals are online.",
    "Press zero for configuration.",
    "Select a terminal target.",
    "The selected target is unavailable.",
    "That selection is not valid.",
    "Returning to the previous service.",
    "Press hash to confirm.",
];

#[derive(Clone)]
struct EventPublisher {
    client: Client,
    endpoint: String,
    targets_endpoint: String,
}

impl EventPublisher {
    fn new(core_url: &str) -> Self {
        Self {
            client: Client::new(),
            endpoint: format!(
                "{}{}",
                core_url.trim_end_matches('/'),
                bts_protocol::core::CORE_EVENTS_PATH
            ),
            targets_endpoint: format!(
                "{}{}",
                core_url.trim_end_matches('/'),
                bts_protocol::core::CORE_TELEPHONY_TARGETS_PATH
            ),
        }
    }

    async fn publish(&self, kind: EventKind) -> anyhow::Result<()> {
        let event = NewEvent {
            source: EVENT_SOURCE.to_owned(),
            kind,
        };

        self.client
            .post(&self.endpoint)
            .json(&event)
            .send()
            .await
            .context("failed to send event to bts-core")?
            .error_for_status()
            .context("bts-core rejected event")?;

        Ok(())
    }

    async fn targets(&self) -> anyhow::Result<TelephonyTargets> {
        self.client
            .get(&self.targets_endpoint)
            .send()
            .await
            .context("failed to request telephony targets from bts-core")?
            .error_for_status()
            .context("bts-core rejected the telephony target request")?
            .json()
            .await
            .context("failed to decode telephony targets")
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialise_logging();

    let ari_url =
        std::env::var("BTS_ARI_URL").unwrap_or_else(|_| "http://127.0.0.1:8088".to_owned());

    let ari_username = std::env::var("BTS_ARI_USERNAME").unwrap_or_else(|_| "bts".to_owned());

    let ari_password = std::env::var("BTS_ARI_PASSWORD").context("BTS_ARI_PASSWORD is not set")?;

    let core_url =
        std::env::var("BTS_CORE_URL").unwrap_or_else(|_| "http://127.0.0.1:3100".to_owned());

    let (menu_media_uris, menu_actions) = load_menu(&core_url).await?;
    let menu_actions = Arc::new(menu_actions);
    let voice = Arc::new(runtime_voice_cache());
    warm_static_speech(&menu_media_uris, &voice)
        .await
        .context("required Telephony speech could not be warmed")?;

    let config = Config::new(&ari_url, &ari_username, &ari_password);
    let mut ari = AriClient::with_config(config);

    let publisher = EventPublisher::new(&core_url);
    let sessions = Arc::new(Mutex::new(HashMap::<String, TelephonySession>::new()));
    let playbacks = Arc::new(Mutex::new(HashMap::<String, Vec<String>>::new()));

    /*
     * A call has entered Stasis(bts).
     */
    let start_publisher = publisher.clone();
    let start_menu_media_uris = menu_media_uris.clone();
    let start_sessions = sessions.clone();
    let start_voice = voice.clone();
    let start_playbacks = playbacks.clone();

    ari.on_stasis_start(move |client, event| {
        let publisher = start_publisher.clone();
        let menu_media_uris = start_menu_media_uris.clone();
        let sessions = start_sessions.clone();
        let voice = start_voice.clone();
        let playbacks = start_playbacks.clone();

        async move {
            let channel = event.data.channel;
            let channel_id = channel.id.clone();
            let caller = CallerIdentity {
                number: non_empty(channel.caller.number.clone()),
                name: non_empty(channel.caller.name.clone()),
            };
            let caller_event = caller.number.clone().or_else(|| caller.name.clone());

            info!(
                channel_id = %channel_id,
                channel_name = %channel.name,
                "call entered BTS"
            );

            if let Err(error) = client.channels().answer(&channel_id).await {
                error!(
                    channel_id = %channel_id,
                    %error,
                    "failed to answer call"
                );

                return Ok(());
            }

            let targets = match publisher.targets().await {
                Ok(targets) => targets,
                Err(error) => {
                    warn!(channel_id = %channel_id, %error, "failed to load initial targets");
                    TelephonyTargets {
                        terminals: Vec::new(),
                        groups: Vec::new(),
                        all: None,
                    }
                }
            };
            let (session, outcome) =
                TelephonySession::new(caller, &targets, menu_media_uris.clone());
            sessions.lock().await.insert(channel_id.clone(), session);
            let ids = play_media_queue(&client, &channel_id, &outcome.media, &voice).await;
            playbacks.lock().await.insert(channel_id.clone(), ids);

            if let Err(error) = publisher
                .publish(EventKind::PhoneCallStarted {
                    channel_id: channel_id.clone(),
                    caller: caller_event,
                })
                .await
            {
                warn!(
                    channel_id = %channel_id,
                    %error,
                    "failed to publish call-start event"
                );
            }

            Ok(())
        }
    });

    /*
     * A digit was pressed during the call.
     */
    let dtmf_publisher = publisher.clone();
    let dtmf_actions = menu_actions.clone();
    let dtmf_sessions = sessions.clone();
    let dtmf_voice = voice.clone();
    let dtmf_playbacks = playbacks.clone();

    ari.on_channel_dtmf_received(move |client, event| {
        let publisher = dtmf_publisher.clone();
        let actions = dtmf_actions.clone();
        let sessions = dtmf_sessions.clone();
        let voice = dtmf_voice.clone();
        let playbacks = dtmf_playbacks.clone();

        async move {
            let channel_id = event.data.channel.id.clone();
            let digit = event.data.digit.clone();

            info!(
                channel_id = %channel_id,
                digit = %digit,
                duration_ms = event.data.duration_ms,
                "DTMF received"
            );

            let interrupted = playbacks.lock().await.remove(&channel_id).unwrap_or_default();
            for playback_id in interrupted {
                if let Err(error) = client.playbacks().stop(&playback_id).await {
                    tracing::debug!(channel_id = %channel_id, %playback_id, %error, "playback completed while being interrupted");
                }
            }

            if let Err(error) = publisher
                .publish(EventKind::PhoneDtmfReceived {
                    channel_id: channel_id.clone(),
                    digit: digit.clone(),
                })
                .await
            {
                warn!(
                    channel_id = %channel_id,
                    %error,
                    "failed to publish DTMF event"
                );
            }

            let targets = match publisher.targets().await {
                Ok(targets) => targets,
                Err(error) => {
                    warn!(channel_id = %channel_id, %error, "failed to refresh telephony targets");
                    TelephonyTargets {
                        terminals: Vec::new(),
                        groups: Vec::new(),
                        all: None,
                    }
                }
            };
            let outcome = {
                let mut sessions = sessions.lock().await;
                sessions
                    .get_mut(&channel_id)
                    .map(|session| session.handle_dtmf(&digit, &targets, &actions))
            };
            let Some(outcome) = outcome else {
                return Ok(());
            };
            if !outcome.media.is_empty() {
                let ids = play_media_queue(&client, &channel_id, &outcome.media, &voice).await;
                playbacks.lock().await.insert(channel_id.clone(), ids);
            }
            if let Some(request) = outcome.action
                && let Err(error) = publisher
                    .publish(EventKind::ActionRequested { request })
                    .await
            {
                warn!(channel_id = %channel_id, %error, "failed to publish targeted menu action");
            }

            Ok(())
        }
    });

    let finished_playbacks = playbacks.clone();
    ari.on_playback_finished(move |_, event| {
        let playbacks = finished_playbacks.clone();
        async move {
            let playback = event.data.playback;
            if playback.state == asterisk_ari::apis::playbacks::models::PlaybackState::Failed {
                warn!(media_uri = ?playback.media_uri, playback_id = ?playback.id, "voice prompt playback failed");
            }
            if let Some(id) = playback.id {
                let mut queues = playbacks.lock().await;
                for queue in queues.values_mut() {
                    queue.retain(|queued_id| queued_id != &id);
                }
            }
            Ok(())
        }
    });

    /*
     * The channel has left Stasis, normally because the caller hung up.
     */
    let end_publisher = publisher.clone();
    let end_sessions = sessions.clone();
    let end_playbacks = playbacks.clone();

    ari.on_stasis_end(move |_, event| {
        let publisher = end_publisher.clone();
        let sessions = end_sessions.clone();
        let playbacks = end_playbacks.clone();

        async move {
            let channel_id = event.data.channel.id.clone();
            if sessions.lock().await.remove(&channel_id).is_none() {
                return Ok(());
            }
            playbacks.lock().await.remove(&channel_id);

            info!(
                channel_id = %channel_id,
                "call left BTS"
            );

            if let Err(error) = publisher
                .publish(EventKind::PhoneCallEnded {
                    channel_id: channel_id.clone(),
                })
                .await
            {
                warn!(
                    channel_id = %channel_id,
                    %error,
                    "failed to publish call-end event"
                );
            }

            Ok(())
        }
    });

    info!(
        application = APPLICATION_NAME,
        ari_url = %ari_url,
        core_url = %core_url,
        menu_items = ?menu_media_uris,
        "starting BTS telephony"
    );

    ari.start(APPLICATION_NAME.to_owned())
        .await
        .context("could not start ARI event listener")?;

    info!("ARI event listener connected");

    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for Ctrl+C")?;

    info!("shutting down BTS telephony");

    ari.stop().await.context("could not stop ARI client")?;

    Ok(())
}

async fn play_media_queue(
    client: &asterisk_ari::apis::client::Client,
    channel_id: &str,
    media: &[MediaItem],
    voice: &Arc<VoiceCache<KokoroSynthesizer>>,
) -> Vec<String> {
    let mut playback_ids = Vec::new();
    for item in media {
        let mut stop_after_item = false;
        let media_uri = match item {
            MediaItem::Uri(uri) => uri.clone(),
            MediaItem::Speech(text) => match voice.cached(text).await {
                Ok(Some(prompt)) => prompt.media_uri,
                Ok(None) => {
                    let voice = Arc::clone(voice);
                    let prompt_text = text.clone();
                    tokio::spawn(async move {
                        if let Err(error) = voice.render(&prompt_text).await {
                            warn!(%error, prompt = %prompt_text, "background voice-cache warm failed");
                        }
                    });
                    warn!(
                        %channel_id,
                        prompt = %text,
                        "uncached dynamic speech skipped on live call; warming in background"
                    );
                    stop_after_item = true;
                    "sound:beeperr".to_owned()
                }
                Err(error) => {
                    warn!(
                        %channel_id,
                        %error,
                        "failed to read cached voice prompt; playing the emergency error tone and abandoning the incomplete queue"
                    );
                    stop_after_item = true;
                    "sound:beeperr".to_owned()
                }
            },
        };
        match client
            .channels()
            .play(channels::params::PlayRequest::new(channel_id, &media_uri))
            .await
        {
            Ok(playback) => {
                if let Some(id) = playback.id {
                    playback_ids.push(id);
                }
            }
            Err(error) => {
                warn!(channel_id = %channel_id, %media_uri, %error, "failed to enqueue voice prompt item")
            }
        }
        if stop_after_item {
            break;
        }
    }
    playback_ids
}

async fn warm_static_speech(
    menu: &[MediaItem],
    voice: &VoiceCache<KokoroSynthesizer>,
) -> anyhow::Result<()> {
    let mut prompts = STATIC_SESSION_PROMPTS
        .iter()
        .map(|prompt| (*prompt).to_owned())
        .collect::<Vec<_>>();
    for item in menu {
        if let MediaItem::Speech(text) = item
            && !prompts.contains(text)
        {
            prompts.push(text.clone());
        }
    }
    for prompt in prompts {
        voice
            .render(&prompt)
            .await
            .with_context(|| format!("failed to warm static speech {prompt:?}"))?;
    }
    Ok(())
}

fn runtime_voice_cache() -> VoiceCache<KokoroSynthesizer> {
    let endpoint = std::env::var("BTS_KOKORO_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8880/v1/audio/speech".into());
    let mut settings = VoiceSettings::default();
    settings.language = std::env::var("BTS_VOICE_LANGUAGE").unwrap_or(settings.language);
    settings.voice = std::env::var("BTS_KOKORO_VOICE").unwrap_or(settings.voice);
    settings.model = std::env::var("BTS_KOKORO_MODEL").unwrap_or(settings.model);
    settings.model_version =
        std::env::var("BTS_KOKORO_MODEL_VERSION").unwrap_or(settings.model_version);
    settings.speed = std::env::var("BTS_KOKORO_SPEED")
        .ok()
        .and_then(|speed| speed.parse().ok())
        .filter(|speed| *speed > 0.0)
        .unwrap_or(settings.speed);
    VoiceCache::new(
        KokoroSynthesizer::new(endpoint),
        settings,
        std::env::var("BTS_VOICE_CACHE_DIR").unwrap_or_else(|_| "/var/cache/bts/voice".into()),
        std::env::var("BTS_ASTERISK_GENERATED_SOUNDS_DIR")
            .unwrap_or_else(|_| "/var/lib/asterisk/sounds/en/bts-generated".into()),
    )
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

async fn load_menu(core_url: &str) -> anyhow::Result<(Vec<MediaItem>, HashMap<String, ActionId>)> {
    let endpoint = format!(
        "{}{}",
        core_url.trim_end_matches('/'),
        bts_protocol::core::CORE_ADDONS_PATH
    );
    let manifests = Client::new()
        .get(endpoint)
        .send()
        .await
        .context("failed to request addon menu from bts-core")?
        .error_for_status()
        .context("bts-core rejected addon menu request")?
        .json::<Vec<AddonManifest>>()
        .await
        .context("failed to decode addon menu")?;
    let menu = build_menu(manifests);
    anyhow::ensure!(
        !menu.1.is_empty(),
        "bts-core has no registered telephone menu entries"
    );
    Ok(menu)
}

fn build_menu(manifests: Vec<AddonManifest>) -> (Vec<MediaItem>, HashMap<String, ActionId>) {
    let mut actions = HashMap::new();
    let mut media = vec![MediaItem::Speech(WELCOME_PROMPT.to_owned())];
    for entry in render_menu(manifests) {
        actions.insert(entry.digit.to_string(), entry.action);
        media.push(MediaItem::Speech(entry.speech));
    }
    (media, actions)
}

fn initialise_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("bts_telephony=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .compact()
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bts_protocol::DtmfMenuKey;
    use bts_protocol::addons::v2::{API_VERSION, ActionId, AddonId, AddonVersion, MenuEntry};

    fn manifest(id: &str, digit: char, order: u16, label: &str) -> AddonManifest {
        AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new(id),
            name: id.into(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![],
            menu: vec![MenuEntry {
                digit: DtmfMenuKey::new(digit).unwrap(),
                label: label.into(),
                spoken_label: None,
                speech_style: Default::default(),
                action: ActionId::new(format!("{id}.run")),
                order,
            }],
            capabilities: vec![],
            screens: vec![],
        }
    }

    #[test]
    fn menu_is_ordered_by_manifest_order_then_digit() {
        let (media, actions) = build_menu(vec![
            manifest("later", '3', 30, "Later"),
            manifest("first", '2', 20, "First"),
        ]);
        assert_eq!(
            media,
            vec![
                MediaItem::Speech(WELCOME_PROMPT.into()),
                MediaItem::Speech("Press two for first.".into()),
                MediaItem::Speech("Press three for later.".into()),
            ]
        );
        assert_eq!(actions["2"], ActionId::new("first.run"));
        assert_eq!(actions["3"], ActionId::new("later.run"));
    }
}
