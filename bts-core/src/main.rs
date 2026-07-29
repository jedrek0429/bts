use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{any, get, post},
};
use bts_protocol::{BtsState, Event, EventKind, NewEvent, ServerMessage};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{RwLock, broadcast};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const EVENT_CHANNEL_CAPACITY: usize = 128;

#[derive(Clone)]
struct AppState {
    current: Arc<RwLock<BtsState>>,
    events: broadcast::Sender<ServerMessage>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialise_logging();

    let bind_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 3100);
    let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

    let state = AppState {
        current: Arc::new(RwLock::new(BtsState::default())),
        events,
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/state", get(get_state))
        .route("/api/v1/events", post(submit_event))
        .route("/api/v1/events/ws", any(websocket_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .with_context(|| format!("failed to bind to {bind_address}"))?;

    info!(address = %bind_address, "BTS Core started");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("BTS Core HTTP server failed")?;

    info!("BTS Core stopped");

    Ok(())
}

fn initialise_logging() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("bts_core=info,tower_http=info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .compact()
        .init();
}

async fn health() -> &'static str {
    "BTS Core is online\n"
}

async fn get_state(State(state): State<AppState>) -> Json<BtsState> {
    let current = state.current.read().await.clone();
    Json(current)
}

async fn submit_event(
    State(state): State<AppState>,
    Json(new_event): Json<NewEvent>,
) -> impl IntoResponse {
    let event = Event::new(new_event.source, new_event.kind);

    let updated_state = {
        let mut current = state.current.write().await;
        apply_event(&mut current, &event);
        current.clone()
    };

    info!(
        event_id = %event.id,
        source = %event.source,
        kind = ?event.kind,
        "event accepted"
    );

    let message = ServerMessage::Event {
        event: event.clone(),
        state: updated_state,
    };

    // No active WebSocket receivers is normal and must not reject the event.
    let _ = state.events.send(message);

    (StatusCode::ACCEPTED, Json(event))
}

fn apply_event(state: &mut BtsState, event: &Event) {
    if let EventKind::DisplaySet { display } = &event.kind {
        state.display = display.clone();
    }
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket_connection(socket, state))
}

async fn websocket_connection(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut event_receiver = state.events.subscribe();

    let snapshot = {
        let current = state.current.read().await.clone();
        ServerMessage::Snapshot { state: current }
    };

    if send_json(&mut sender, &snapshot).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            server_message = event_receiver.recv() => {
                match server_message {
                    Ok(message) => {
                        if send_json(&mut sender, &message).await.is_err() {
                            break;
                        }
                    }

                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            skipped,
                            "WebSocket client lagged behind BTS events"
                        );

                        let snapshot = {
                            let current = state.current.read().await.clone();
                            ServerMessage::Snapshot { state: current }
                        };

                        if send_json(&mut sender, &snapshot).await.is_err() {
                            break;
                        }
                    }

                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            client_message = receiver.next() => {
                match client_message {
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }

                    Some(Ok(Message::Ping(data))) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }

                    Some(Ok(_)) => {
                        // WebSocket clients receive events only.
                    }

                    Some(Err(error)) => {
                        warn!(%error, "WebSocket receive error");
                        break;
                    }
                }
            }
        }
    }

    info!("BTS event client disconnected");
}

async fn send_json(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    value: &ServerMessage,
) -> Result<(), ()> {
    let json = match serde_json::to_string(value) {
        Ok(json) => json,

        Err(error) => {
            error!(%error, "failed to serialise WebSocket message");
            return Err(());
        }
    };

    sender
        .send(Message::Text(json.into()))
        .await
        .map_err(|error| {
            warn!(%error, "failed to send WebSocket message");
        })
}

async fn shutdown_signal() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            info!("shutdown signal received");
        }

        Err(error) => {
            error!(%error, "failed to listen for shutdown signal");
        }
    }
}
