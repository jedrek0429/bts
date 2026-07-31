mod addons;

use std::time::Duration;

use anyhow::{Context, Result};
use bts_addons::AddonFailure;
use bts_protocol::{Event, ServerMessage};
use futures_util::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message as WebSocketMessage};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_CORE_HTTP_URL: &str = "http://127.0.0.1:3100";
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<()> {
    initialise_logging();

    let core_http_url =
        std::env::var("BTS_CORE_HTTP_URL").unwrap_or_else(|_| DEFAULT_CORE_HTTP_URL.to_owned());
    let core_ws_url = std::env::var("BTS_CORE_WS_URL")
        .unwrap_or_else(|_| bts_protocol::core::LOCAL_CORE_WEBSOCKET_URL.to_owned());

    let data_root =
        std::env::var("BTS_ADDON_DATA_ROOT").unwrap_or_else(|_| "/var/lib/bts/addons".to_owned());
    let addons = addons::Addons::new(core_http_url, data_root.into())?;

    log_failures(addons.start().await);

    info!(%core_ws_url, "BTS Addons started");

    loop {
        tokio::select! {
            result = run_connection(&core_ws_url, &addons) => {
                if let Err(error) = result {
                    warn!(%error, "BTS Core event connection ended");
                }

                tokio::time::sleep(RECONNECT_DELAY).await;
            }
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to listen for Ctrl+C")?;
                break;
            }
        }
    }

    log_failures(addons.stop().await);
    Ok(())
}

async fn run_connection(core_ws_url: &str, addons: &addons::Addons) -> Result<()> {
    let (socket, _) = connect_async(core_ws_url)
        .await
        .with_context(|| format!("failed to connect to {core_ws_url}"))?;

    info!("connected to BTS Core event stream");

    let (_, mut receiver) = socket.split();

    while let Some(message) = receiver.next().await {
        let message = message.context("failed to receive BTS Core WebSocket message")?;

        match message {
            WebSocketMessage::Text(text) => {
                let server_message: ServerMessage =
                    serde_json::from_str(&text).context("invalid BTS Core message")?;

                if let ServerMessage::Event { event, .. } = server_message {
                    dispatch_event(addons, &event).await;
                }
            }

            WebSocketMessage::Close(_) => break,

            WebSocketMessage::Binary(_)
            | WebSocketMessage::Ping(_)
            | WebSocketMessage::Pong(_)
            | WebSocketMessage::Frame(_) => {}
        }
    }

    Ok(())
}

async fn dispatch_event(addons: &addons::Addons, event: &Event) {
    for failure in addons.handle(event).await {
        error!(%failure, "addon failed");
    }
}

fn log_failures(failures: Vec<AddonFailure>) {
    for failure in failures {
        error!(%failure, "addon lifecycle hook failed");
    }
}

fn initialise_logging() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("bts_addons=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .compact()
        .init();
}
