use anyhow::Context;
use asterisk_ari::{AriClient, Config, apis::channels};
use bts_protocol::{EventKind, NewEvent};
use reqwest::Client;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const APPLICATION_NAME: &str = "bts";
const EVENT_SOURCE: &str = "bts-telephony";
const DEFAULT_MENU_MEDIA_URIS: &str = concat!(
    "sound:bts/welcome,",
    "sound:bts/press-2-time,",
    "sound:bts/press-3-weather,",
    "sound:bts/press-0-clear"
);

#[derive(Clone)]
struct EventPublisher {
    client: Client,
    endpoint: String,
}

impl EventPublisher {
    fn new(core_url: &str) -> Self {
        Self {
            client: Client::new(),
            endpoint: format!("{}/api/v1/events", core_url.trim_end_matches('/')),
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

    let menu_media_uris = std::env::var("BTS_MENU_MEDIA_URIS")
        .unwrap_or_else(|_| DEFAULT_MENU_MEDIA_URIS.to_owned());

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

    ari.on_channel_dtmf_received(move |_, event| {
        let publisher = dtmf_publisher.clone();

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

fn initialise_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("bts_telephony=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .compact()
        .init();
}
