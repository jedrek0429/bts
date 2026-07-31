use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::extract::ws::{Message, WebSocket};
use bts_protocol::{
    CoreTerminalMessage, ProtocolVersion, TerminalClientMessage, TerminalConnectionId, TerminalId,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use crate::{
    presentations::{AcknowledgementDisposition, PresentationManager, PresentationPlan},
    terminals::{DEFAULT_HEARTBEAT_INTERVAL, RegisterError, TerminalRegistry},
};

const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);
const OUTBOUND_CAPACITY: usize = 32;

#[derive(Clone)]
pub(crate) struct TerminalTransport {
    registry: TerminalRegistry,
    presentations: PresentationManager,
    connections: Arc<Mutex<HashMap<TerminalConnectionId, ConnectionSender>>>,
    shutdown: watch::Receiver<bool>,
    core_epoch: uuid::Uuid,
}

#[derive(Clone)]
struct ConnectionSender {
    terminal_id: TerminalId,
    sender: mpsc::Sender<CoreTerminalMessage>,
}

impl TerminalTransport {
    pub(crate) fn new(
        registry: TerminalRegistry,
        presentations: PresentationManager,
        shutdown: watch::Receiver<bool>,
        core_epoch: uuid::Uuid,
    ) -> Self {
        Self {
            registry,
            presentations,
            connections: Arc::new(Mutex::new(HashMap::new())),
            shutdown,
            core_epoch,
        }
    }

    pub(crate) fn fan_out(&self, plans: impl IntoIterator<Item = PresentationPlan>) {
        for plan in plans {
            let Some(dispatch) = plan.dispatch else {
                continue;
            };
            let message = CoreTerminalMessage::PresentationDispatch {
                presentation: Box::new(dispatch),
            };
            for recipient in plan.recipients {
                let send_result = self
                    .lock_connections()
                    .get(&recipient.connection_id)
                    .filter(|connection| connection.terminal_id == recipient.terminal_id)
                    .map(|connection| connection.sender.try_send(message.clone()));
                if !matches!(send_result, Some(Ok(()))) {
                    self.remove_connection(&recipient.terminal_id, recipient.connection_id);
                }
            }
        }
    }

    pub(crate) fn expire_connections(
        &self,
        expired: impl IntoIterator<Item = crate::terminals::ExpiredTerminalPresence>,
    ) {
        for presence in expired {
            self.remove_connection(&presence.terminal_id, presence.connection_id);
        }
    }

    pub(crate) async fn connection(&self, socket: WebSocket, remote_address: SocketAddr) {
        let (mut sender, mut receiver) = socket.split();
        let mut shutdown = self.shutdown.clone();
        let first = tokio::select! {
            _ = shutdown.changed() => None,
            message = tokio::time::timeout(REGISTRATION_TIMEOUT, receiver.next()) => {
                message.ok().flatten().and_then(Result::ok)
            }
        };
        let Some(Message::Text(text)) = first else {
            let _ = sender.send(Message::Close(None)).await;
            return;
        };
        let Ok(TerminalClientMessage::Register {
            registration,
            implementation_version,
            runtime_diagnostics,
        }) = serde_json::from_str(&text)
        else {
            let _ = sender.send(Message::Close(None)).await;
            return;
        };

        let terminal_id = registration.identity.id.clone();
        let connection_id = TerminalConnectionId::new();
        let (outbound_sender, mut outbound_receiver) = mpsc::channel(OUTBOUND_CAPACITY);
        self.lock_connections().insert(
            connection_id,
            ConnectionSender {
                terminal_id: terminal_id.clone(),
                sender: outbound_sender,
            },
        );

        match self.registry.register_with_metadata(
            registration,
            connection_id,
            Some(remote_address),
            Instant::now(),
            implementation_version,
            runtime_diagnostics,
        ) {
            Ok(_) => {}
            Err(RegisterError::Rejected(rejection)) => {
                let _ = send_json(
                    &mut sender,
                    &CoreTerminalMessage::RegistrationRejected { rejection },
                )
                .await;
                self.lock_connections().remove(&connection_id);
                let _ = sender.send(Message::Close(None)).await;
                return;
            }
            Err(RegisterError::Persistence(error)) => {
                warn!(%error, %terminal_id, "terminal registration persistence failed");
                self.lock_connections().remove(&connection_id);
                let _ = sender.send(Message::Close(None)).await;
                return;
            }
        }

        if send_json(
            &mut sender,
            &CoreTerminalMessage::RegistrationAcknowledged {
                terminal_id: terminal_id.clone(),
                connection_id,
                core_epoch: self.core_epoch,
                protocol_version: ProtocolVersion::CURRENT,
                heartbeat_interval_seconds: u32::try_from(DEFAULT_HEARTBEAT_INTERVAL.as_secs())
                    .expect("heartbeat interval fits the protocol field"),
            },
        )
        .await
        .is_err()
        {
            self.remove_connection(&terminal_id, connection_id);
            return;
        }

        info!(%terminal_id, ?connection_id, "terminal WebSocket registered");
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                outbound = outbound_receiver.recv() => {
                    let Some(message) = outbound else { break };
                    if send_json(&mut sender, &message).await.is_err() {
                        break;
                    }
                }
                inbound = receiver.next() => {
                    let Some(Ok(message)) = inbound else { break };
                    match message {
                        Message::Text(text) => {
                            let Ok(message) = serde_json::from_str::<TerminalClientMessage>(&text) else {
                                break;
                            };
                            if !self.process_message(
                                &terminal_id,
                                connection_id,
                                message,
                                &mut sender,
                            ).await {
                                break;
                            }
                        }
                        Message::Ping(payload) => {
                            if sender.send(Message::Pong(payload)).await.is_err() {
                                break;
                            }
                        }
                        Message::Pong(_) => {}
                        Message::Close(_) | Message::Binary(_) => break,
                    }
                }
            }
        }
        self.remove_connection(&terminal_id, connection_id);
        let _ = sender.send(Message::Close(None)).await;
        info!(%terminal_id, ?connection_id, "terminal WebSocket disconnected");
    }

    async fn process_message<S>(
        &self,
        bound_terminal: &TerminalId,
        bound_connection: TerminalConnectionId,
        message: TerminalClientMessage,
        sender: &mut S,
    ) -> bool
    where
        S: futures_util::Sink<Message> + Unpin,
    {
        match message {
            TerminalClientMessage::Register { .. } => false,
            TerminalClientMessage::Heartbeat {
                terminal_id,
                connection_id,
            } => {
                if &terminal_id != bound_terminal || connection_id != bound_connection {
                    return false;
                }
                if self
                    .registry
                    .heartbeat(&terminal_id, connection_id, Instant::now())
                    .is_err()
                {
                    return false;
                }
                send_json(
                    sender,
                    &CoreTerminalMessage::HeartbeatAcknowledged { connection_id },
                )
                .await
                .is_ok()
            }
            TerminalClientMessage::Disconnect {
                terminal_id,
                connection_id,
                ..
            } => {
                if &terminal_id != bound_terminal || connection_id != bound_connection {
                    tracing::warn!(
                        terminal_id = %terminal_id,
                        connection_id = ?connection_id,
                        "closing terminal socket after an unowned disconnect"
                    );
                }
                false
            }
            TerminalClientMessage::PresentationAccepted {
                terminal_id,
                connection_id,
                presentation_id,
            } => {
                if &terminal_id != bound_terminal || connection_id != bound_connection {
                    return false;
                }
                self.valid_acknowledgement(self.presentations.acknowledge_accepted(
                    &terminal_id,
                    connection_id,
                    presentation_id,
                    Instant::now(),
                ))
            }
            TerminalClientMessage::PresentationRejected {
                terminal_id,
                connection_id,
                presentation_id,
                rejection,
            } => {
                if &terminal_id != bound_terminal || connection_id != bound_connection {
                    return false;
                }
                self.valid_acknowledgement(self.presentations.acknowledge_rejected(
                    &terminal_id,
                    connection_id,
                    presentation_id,
                    rejection,
                    Instant::now(),
                ))
            }
        }
    }

    fn valid_acknowledgement(&self, disposition: AcknowledgementDisposition) -> bool {
        !matches!(
            disposition,
            AcknowledgementDisposition::StaleConnection
                | AcknowledgementDisposition::UnexpectedTerminal
        )
    }

    fn remove_connection(&self, terminal_id: &TerminalId, connection_id: TerminalConnectionId) {
        let removed = {
            let mut connections = self.lock_connections();
            if connections
                .get(&connection_id)
                .is_some_and(|connection| &connection.terminal_id == terminal_id)
            {
                connections.remove(&connection_id);
                true
            } else {
                false
            }
        };
        if removed {
            let _ = self.registry.disconnect(terminal_id, connection_id);
            self.presentations
                .connection_disconnected(terminal_id, connection_id);
        }
    }

    fn lock_connections(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<TerminalConnectionId, ConnectionSender>> {
        self.connections
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

async fn send_json<S>(sender: &mut S, value: &CoreTerminalMessage) -> Result<(), ()>
where
    S: futures_util::Sink<Message> + Unpin,
{
    let json = serde_json::to_string(value).map_err(|_| ())?;
    sender
        .send(Message::Text(json.into()))
        .await
        .map_err(|_| ())
}
