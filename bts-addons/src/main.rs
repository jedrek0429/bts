mod addons;

use std::time::Duration;

use anyhow::{Context, Result};
use bts_addons::{AddonContext, AddonFailure};
use bts_protocol::{Event, ServerMessage};
use futures_util::StreamExt;
use tokio_tungstenite::{connect_async, tungstenite::Message as WebSocketMessage};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_CORE_HTTP_URL: &str = "http://127.0.0.1:3100";
const DEFAULT_CORE_WS_URL: &str = "ws://127.0.0.1:3100/api/v1/events/ws";
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<()> {
    initialise_logging();

    let core_http_url =
        std::env::var("BTS_CORE_HTTP_URL").unwrap_or_else(|_| DEFAULT_CORE_HTTP_URL.to_owned());
    let core_ws_url =
        std::env::var("BTS_CORE_WS_URL").unwrap_or_else(|_| DEFAULT_CORE_WS_URL.to_owned());

    let context = AddonContext::new(core_http_url);
    let addons = addons::Addons::new();

    log_failures(addons.start(&context).await);

    info!(%core_ws_url, "BTS Addons started");

    loop {
        tokio::select! {
            result = run_connection(&core_ws_url, &context, &addons) => {
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

    log_failures(addons.stop(&context).await);
    Ok(())
}

async fn run_connection(
    core_ws_url: &str,
    context: &AddonContext,
    addons: &addons::Addons,
) -> Result<()> {
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
                    dispatch_event(context, addons, &event).await;
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

async fn dispatch_event(context: &AddonContext, addons: &addons::Addons, event: &Event) {
    for failure in addons.handle(context, event).await {
        error!(%failure, "addon failed");
        publish_addon_error(context, &failure).await;
    }
}

async fn publish_addon_error(context: &AddonContext, error: &AddonFailure) {
    if let Err(publish_error) = addons::message::show(
        context,
        "BTS service",
        &format!("The service is currently unavailable.\n\n{error}"),
    )
    .await
    {
        error!(%publish_error, "failed to publish addon error message");
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
