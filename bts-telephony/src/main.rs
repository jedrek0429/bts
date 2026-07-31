use anyhow::Context;
use asterisk_ari::{AriClient, Config, apis::channels};
use std::{collections::HashMap, sync::Arc};

use bts_protocol::addons::v1::{ActionId, ActionRequest, AddonManifest};
use bts_protocol::{EventKind, NewEvent};
use reqwest::Client;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const APPLICATION_NAME: &str = "bts";
const EVENT_SOURCE: &str = "bts-telephony";
const WELCOME_PROMPT: &str = "sound:bts/welcome";

#[derive(Clone)]
struct EventPublisher {
    client: Client,
    endpoint: String,
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

    /*
     * A call has entered Stasis(bts).
     */
    let start_publisher = publisher.clone();
    let start_menu_media_uris = menu_media_uris.clone();

    ari.on_stasis_start(move |client, event| {
        let publisher = start_publisher.clone();
        let menu_media_uris = start_menu_media_uris.clone();

        async move {
            let channel = event.data.channel;
            let channel_id = channel.id.clone();

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

            if let Err(error) = client
                .channels()
                .play(channels::params::PlayRequest::new(
                    &channel_id,
                    &menu_media_uris,
                ))
                .await
            {
                warn!(
                    channel_id = %channel_id,
                    media_uri = %menu_media_uris,
                    %error,
                    "failed to play menu prompts"
                );
            }

            if let Err(error) = publisher
                .publish(EventKind::PhoneCallStarted {
                    channel_id: channel_id.clone(),
                    caller: None,
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

    ari.on_channel_dtmf_received(move |_, event| {
        let publisher = dtmf_publisher.clone();
        let actions = dtmf_actions.clone();

        async move {
            let channel_id = event.data.channel.id.clone();
            let digit = event.data.digit.clone();

            info!(
                channel_id = %channel_id,
                digit = %digit,
                duration_ms = event.data.duration_ms,
                "DTMF received"
            );

            if let Err(error) = publisher
                .publish(EventKind::PhoneDtmfReceived {
                    channel_id: channel_id.clone(),
                    digit,
                })
                .await
            {
                warn!(
                    channel_id = %channel_id,
                    %error,
                    "failed to publish DTMF event"
                );
            }

            if let Some(action) = actions.get(&event.data.digit)
                && let Err(error) = publisher
                    .publish(EventKind::ActionRequested {
                        request: ActionRequest {
                            action: action.clone(),
                            parameters: serde_json::Value::Null,
                        },
                    })
                    .await
            {
                warn!(channel_id = %channel_id, %error, "failed to publish menu action");
            }

            Ok(())
        }
    });

    /*
     * The channel has left Stasis, normally because the caller hung up.
     */
    let end_publisher = publisher.clone();

    ari.on_stasis_end(move |_, event| {
        let publisher = end_publisher.clone();

        async move {
            let channel_id = event.data.channel.id.clone();

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
        menu_media_uris = %menu_media_uris,
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

async fn load_menu(core_url: &str) -> anyhow::Result<(String, HashMap<String, ActionId>)> {
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

fn build_menu(manifests: Vec<AddonManifest>) -> (String, HashMap<String, ActionId>) {
    let mut entries: Vec<_> = manifests
        .into_iter()
        .flat_map(|manifest| manifest.menu)
        .collect();
    entries.sort_by_key(|entry| (entry.order, entry.digit));
    let mut actions = HashMap::new();
    let mut media = vec![WELCOME_PROMPT.to_owned()];
    for entry in entries {
        actions.insert(entry.digit.to_string(), entry.action);
        media.push(entry.prompt);
    }
    (media.join(","), actions)
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
    use bts_protocol::addons::v1::{API_VERSION, ActionId, AddonId, AddonVersion, MenuEntry};

    fn manifest(id: &str, digit: char, order: u16, prompt: &str) -> AddonManifest {
        AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new(id),
            name: id.into(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![],
            menu: vec![MenuEntry {
                digit,
                prompt: prompt.into(),
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
            manifest("later", '3', 30, "sound:later"),
            manifest("first", '2', 20, "sound:first"),
        ]);
        assert_eq!(media, "sound:bts/welcome,sound:first,sound:later");
        assert_eq!(actions["2"], ActionId::new("first.run"));
        assert_eq!(actions["3"], ActionId::new("later.run"));
    }
}
