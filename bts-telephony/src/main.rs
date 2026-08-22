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

    let config = Config::new(&ari_url, &ari_username, &ari_password);
    let mut ari = AriClient::with_config(config);

    let publisher = EventPublisher::new(&core_url);
    let sessions = Arc::new(Mutex::new(HashMap::<String, TelephonySession>::new()));
    let voice = Arc::new(runtime_voice_cache());
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
    voice: &VoiceCache<KokoroSynthesizer>,
) -> Vec<String> {
    let mut playback_ids = Vec::new();
    for item in media {
        let mut stop_after_item = false;
        let media_uri = match item {
            MediaItem::Uri(uri) => uri.clone(),
            MediaItem::Speech(text) => match voice.render(text).await {
                Ok(prompt) => prompt.media_uri,
                Err(error) => {
                    warn!(channel_id = %channel_id, prompt_text = %text, %error, "failed to render voice prompt; playing the emergency error tone and abandoning the incomplete queue");
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
    use asterisk_ari::apis::client::Client as AriHttpClient;
    use async_trait::async_trait;
    use bts_protocol::DtmfMenuKey;
    use bts_protocol::addons::v2::{API_VERSION, ActionId, AddonId, AddonVersion, MenuEntry};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::{Semaphore, mpsc},
        time::{Duration, timeout},
    };

    #[derive(Clone)]
    struct GatedSynthesizer {
        calls: Arc<AtomicUsize>,
        permits: Arc<Semaphore>,
    }

    #[async_trait]
    impl SpeechSynthesizer for GatedSynthesizer {
        async fn synthesise(
            &self,
            _text: &str,
            _settings: &VoiceSettings,
        ) -> anyhow::Result<Vec<u8>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.permits.acquire().await.unwrap().forget();
            Ok(b"RIFF\0\0\0\0WAVEtest".to_vec())
        }
    }

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

    #[tokio::test]
    async fn vanished_channel_is_not_sent_a_playback_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (requests_tx, mut requests_rx) = mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let size = socket.read(&mut request).await.unwrap();
            request.truncate(size);
            requests_tx.send(request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 31\r\nConnection: close\r\n\r\n{\"message\":\"Channel not found\"}",
                )
                .await
                .unwrap();
        });
        let config = Config::new(format!("http://{address}"), "bts", "secret");
        let client = AriHttpClient::with_config(config);
        let root = tempfile::tempdir().unwrap();
        let voice = Arc::new(VoiceCache::new(
            KokoroSynthesizer::new("http://127.0.0.1:1"),
            VoiceSettings::default(),
            root.path().join("cache"),
            root.path().join("sounds"),
        ));

        let playback_ids = play_media_queue(
            &client,
            "departed-channel",
            &[MediaItem::Uri("sound:already-cached".into())],
            &voice,
        )
        .await;

        assert!(playback_ids.is_empty());
        let request = timeout(Duration::from_secs(1), requests_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(request.starts_with(b"GET "));
        assert!(!request.windows(5).any(|window| window == b"/play"));
        server.abort();
    }

    #[tokio::test]
    async fn readiness_waits_for_static_speech_and_live_lookup_never_waits_for_synthesis() {
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let permits = Arc::new(Semaphore::new(0));
        let cache = Arc::new(VoiceCache::new(
            GatedSynthesizer {
                calls: calls.clone(),
                permits: permits.clone(),
            },
            VoiceSettings::default(),
            root.path().join("cache"),
            root.path().join("sounds"),
        ));
        let menu = vec![MediaItem::Speech("Welcome before calls.".into())];
        let warming_cache = cache.clone();
        let warming = tokio::spawn(async move { warm_static_speech(&menu, &warming_cache).await });

        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert!(
            !warming.is_finished(),
            "readiness must wait for the slow synthesiser"
        );
        assert!(
            timeout(Duration::from_millis(20), cache.cached("Dynamic miss."))
                .await
                .expect("a live cache lookup must be bounded")
                .unwrap()
                .is_none()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        permits.add_permits(100);
        warming.await.unwrap().unwrap();
        assert!(
            cache
                .cached("Welcome before calls.")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn warming_a_changed_menu_synthesises_only_new_static_speech() {
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let permits = Arc::new(Semaphore::new(100));
        let cache = VoiceCache::new(
            GatedSynthesizer {
                calls: calls.clone(),
                permits,
            },
            VoiceSettings::default(),
            root.path().join("cache"),
            root.path().join("sounds"),
        );
        warm_static_speech(
            &[MediaItem::Speech("Press two for the time.".into())],
            &cache,
        )
        .await
        .unwrap();
        let initial = calls.load(Ordering::SeqCst);

        warm_static_speech(
            &[
                MediaItem::Speech("Press two for the time.".into()),
                MediaItem::Speech("Press five for departures.".into()),
            ],
            &cache,
        )
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), initial + 1);
    }

    #[tokio::test]
    async fn changed_menu_becomes_visible_only_after_its_static_speech_is_warm() {
        let root = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let permits = Arc::new(Semaphore::new(0));
        let cache = Arc::new(VoiceCache::new(
            GatedSynthesizer {
                calls: calls.clone(),
                permits: permits.clone(),
            },
            VoiceSettings::default(),
            root.path().join("cache"),
            root.path().join("sounds"),
        ));
        let state = Arc::new(tokio::sync::RwLock::new(MenuState {
            media: vec![MediaItem::Speech("Old menu.".into())],
            actions: HashMap::from([("2".into(), ActionId::new("clock.show"))]),
        }));
        let replacement = MenuState {
            media: vec![MediaItem::Speech("New menu.".into())],
            actions: HashMap::from([("5".into(), ActionId::new("weather.show"))]),
        };
        let updating_state = state.clone();
        let updating_cache = cache.clone();
        let update = tokio::spawn(async move {
            install_menu_update(replacement, &updating_cache, &updating_state).await
        });

        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            state.read().await.actions.get("2").unwrap().as_str(),
            "clock.show"
        );
        assert!(!update.is_finished());

        permits.add_permits(100);
        update.await.unwrap().unwrap();
        let state = state.read().await;
        assert_eq!(state.actions.get("5").unwrap().as_str(), "weather.show");
        assert!(!state.actions.contains_key("2"));
    }
}
