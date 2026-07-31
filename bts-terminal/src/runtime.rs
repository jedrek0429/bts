use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, TryRecvError},
    },
    thread,
    time::{Duration, Instant as StdInstant},
};

use bts_protocol::{
    CoreTerminalMessage, PresentationDeliveryContext, PresentationDispatch, PresentationGeneration,
    PresentationId, PresentationRejection, PresentationRejectionCode, ProtocolVersion,
    RegistrationRejection, TerminalClientMessage, TerminalConnectionId, TerminalId,
};
use tokio::{sync::mpsc as tokio_mpsc, time::Instant as TokioInstant};

use crate::{
    TerminalConfiguration,
    transport::{Connection, Connector, WebSocketConnector},
};

const RECENT_PRESENTATION_CAPACITY: usize = 256;

/// The connection lifecycle visible to a terminal implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    /// Registration has been acknowledged and presentations may be applied.
    Registered {
        terminal_id: TerminalId,
        connection_id: TerminalConnectionId,
    },
    Disconnected {
        reason: String,
    },
    Retrying {
        attempt: u32,
        delay: Duration,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IgnoredDispatchReason {
    ForeignRecipient,
    MissingDeliveryContext,
    StaleConnection,
    InvalidGeneration,
    InvalidValidity,
    OlderGeneration,
    Expired,
    /// The presentation ID was already observed on this or a previous connection.
    DuplicatePresentation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoredCommandReason {
    NotReady,
    UnknownOrSettledPresentation,
}

/// Why previously delivered work is no longer safe to apply or complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationInvalidationReason {
    Superseded,
    Expired,
}

/// Current local status of one presentation work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationStatus {
    Active,
    Superseded,
    Expired,
    Completed,
    ConnectionLost,
}

impl PresentationStatus {
    const fn encode(self) -> u8 {
        match self {
            Self::Active => 0,
            Self::Superseded => 1,
            Self::Expired => 2,
            Self::Completed => 3,
            Self::ConnectionLost => 4,
        }
    }

    const fn decode(value: u8) -> Self {
        match value {
            0 => Self::Active,
            1 => Self::Superseded,
            2 => Self::Expired,
            3 => Self::Completed,
            _ => Self::ConnectionLost,
        }
    }
}

/// Single-use completion identity for one connection-owned generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationCompletion {
    presentation_id: PresentationId,
    connection_id: TerminalConnectionId,
    generation: PresentationGeneration,
}

impl PresentationCompletion {
    pub const fn presentation_id(&self) -> PresentationId {
        self.presentation_id
    }

    pub const fn connection_id(&self) -> TerminalConnectionId {
        self.connection_id
    }

    pub const fn generation(&self) -> PresentationGeneration {
        self.generation
    }
}

/// Validated renderer work plus all ordering and validity information.
#[derive(Debug, Clone)]
pub struct PresentationWork {
    presentation: Box<PresentationDispatch>,
    delivery: PresentationDeliveryContext,
    valid_until: StdInstant,
    completion: PresentationCompletion,
    status: Arc<AtomicU8>,
}

impl PresentationWork {
    pub fn presentation(&self) -> &PresentationDispatch {
        &self.presentation
    }

    pub fn delivery(&self) -> &PresentationDeliveryContext {
        &self.delivery
    }

    pub const fn valid_until(&self) -> StdInstant {
        self.valid_until
    }

    pub fn completion(&self) -> &PresentationCompletion {
        &self.completion
    }

    pub fn status(&self) -> PresentationStatus {
        PresentationStatus::decode(self.status.load(Ordering::Acquire))
    }

    /// Must be checked immediately before applying the presentation.
    pub fn is_applicable(&self) -> bool {
        self.status() == PresentationStatus::Active && StdInstant::now() < self.valid_until
    }
}

impl PartialEq for PresentationWork {
    fn eq(&self, other: &Self) -> bool {
        self.presentation == other.presentation
            && self.delivery == other.delivery
            && self.valid_until == other.valid_until
            && self.completion == other.completion
            && self.status() == other.status()
    }
}

impl Eq for PresentationWork {}

/// Events delivered to the renderer or other terminal implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    ConnectionStateChanged(ConnectionState),
    RegistrationRejected(RegistrationRejection),
    /// A validated dispatch addressed to this terminal and ready for local work.
    PresentationReceived(PresentationWork),
    PresentationInvalidated {
        completion: PresentationCompletion,
        reason: PresentationInvalidationReason,
    },
    DispatchIgnored {
        presentation_id: PresentationId,
        reason: IgnoredDispatchReason,
    },
    CommandIgnored {
        presentation_id: PresentationId,
        reason: IgnoredCommandReason,
    },
    ProtocolError {
        detail: String,
    },
}

/// Commands sent by a concrete terminal implementation to the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCommand {
    PresentationAccepted {
        completion: PresentationCompletion,
    },
    PresentationRejected {
        completion: PresentationCompletion,
        rejection: PresentationRejection,
    },
    Shutdown {
        reason: Option<String>,
    },
}

/// Starts renderer-neutral terminal workers.
pub struct TerminalRuntime;

impl TerminalRuntime {
    /// Starts a WebSocket worker on a dedicated background thread.
    ///
    /// Events remain on a standard channel so an egui application can poll
    /// them from its main thread and request repaint itself.
    pub fn spawn(configuration: TerminalConfiguration) -> Result<TerminalHandle, HandleError> {
        spawn_with_connector(configuration, Arc::new(WebSocketConnector))
    }
}

/// Main-thread handle for observing and controlling a terminal runtime.
pub struct TerminalHandle {
    commands: tokio_mpsc::UnboundedSender<TerminalCommand>,
    events: Receiver<TerminalEvent>,
    worker: Option<thread::JoinHandle<()>>,
}

impl TerminalHandle {
    pub fn send(&self, command: TerminalCommand) -> Result<(), HandleError> {
        self.commands
            .send(command)
            .map_err(|_| HandleError::RuntimeStopped)
    }

    pub fn accept_presentation(
        &self,
        completion: PresentationCompletion,
    ) -> Result<(), HandleError> {
        self.send(TerminalCommand::PresentationAccepted { completion })
    }

    pub fn reject_presentation(
        &self,
        completion: PresentationCompletion,
        rejection: PresentationRejection,
    ) -> Result<(), HandleError> {
        self.send(TerminalCommand::PresentationRejected {
            completion,
            rejection,
        })
    }

    pub fn try_next_event(&self) -> Result<TerminalEvent, TryRecvError> {
        self.events.try_recv()
    }

    pub fn next_event_timeout(&self, timeout: Duration) -> Result<TerminalEvent, RecvTimeoutError> {
        self.events.recv_timeout(timeout)
    }

    pub fn shutdown(mut self, reason: Option<String>) -> Result<(), HandleError> {
        let sent = self
            .commands
            .send(TerminalCommand::Shutdown { reason })
            .is_ok();
        self.join_worker()?;
        if sent {
            Ok(())
        } else {
            Err(HandleError::RuntimeStopped)
        }
    }

    fn join_worker(&mut self) -> Result<(), HandleError> {
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            Err(HandleError::WorkerPanicked)
        } else {
            Ok(())
        }
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        let _ = self
            .commands
            .send(TerminalCommand::Shutdown { reason: None });
        let _ = self.join_worker();
    }
}

#[derive(Debug)]
pub enum HandleError {
    WorkerStart(std::io::Error),
    RuntimeStopped,
    WorkerPanicked,
}

impl fmt::Display for HandleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerStart(error) => {
                write!(formatter, "could not start terminal worker: {error}")
            }
            Self::RuntimeStopped => formatter.write_str("the terminal runtime has stopped"),
            Self::WorkerPanicked => formatter.write_str("the terminal worker panicked"),
        }
    }
}

impl Error for HandleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::WorkerStart(error) => Some(error),
            Self::RuntimeStopped | Self::WorkerPanicked => None,
        }
    }
}

fn spawn_with_connector<C>(
    configuration: TerminalConfiguration,
    connector: Arc<C>,
) -> Result<TerminalHandle, HandleError>
where
    C: Connector,
{
    let (command_sender, command_receiver) = tokio_mpsc::unbounded_channel();
    let (event_sender, event_receiver) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("bts-terminal-runtime".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => runtime.block_on(run_runtime(
                    configuration,
                    connector,
                    command_receiver,
                    event_sender,
                )),
                Err(error) => {
                    let _ = event_sender.send(TerminalEvent::ProtocolError {
                        detail: format!("could not initialise terminal runtime: {error}"),
                    });
                }
            }
        })
        .map_err(HandleError::WorkerStart)?;

    Ok(TerminalHandle {
        commands: command_sender,
        events: event_receiver,
        worker: Some(worker),
    })
}

enum ConnectionOutcome {
    Shutdown,
    Disconnected {
        reason: String,
        was_registered: bool,
    },
}

async fn run_runtime<C>(
    configuration: TerminalConfiguration,
    connector: Arc<C>,
    mut commands: tokio_mpsc::UnboundedReceiver<TerminalCommand>,
    events: mpsc::Sender<TerminalEvent>,
) where
    C: Connector,
{
    let mut consecutive_failures = 0_u32;
    let mut dispatch_history = DispatchHistory::default();

    loop {
        publish_state(&events, ConnectionState::Connecting);
        let outcome = connect_once(
            &configuration,
            connector.as_ref(),
            &mut commands,
            &events,
            &mut dispatch_history,
        )
        .await;

        let ConnectionOutcome::Disconnected {
            reason,
            was_registered,
        } = outcome
        else {
            return;
        };
        publish_state(
            &events,
            ConnectionState::Disconnected {
                reason: reason.clone(),
            },
        );

        consecutive_failures = if was_registered {
            1
        } else {
            consecutive_failures.saturating_add(1)
        };
        let delay = configuration
            .reconnect_policy()
            .delay_for_failure(consecutive_failures);
        publish_state(
            &events,
            ConnectionState::Retrying {
                attempt: consecutive_failures,
                delay,
            },
        );

        let retry = tokio::time::sleep(delay);
        tokio::pin!(retry);
        loop {
            tokio::select! {
                () = &mut retry => break,
                command = commands.recv() => match command {
                    Some(TerminalCommand::Shutdown { .. }) | None => return,
                    Some(command) => publish_ignored_command(&events, command, IgnoredCommandReason::NotReady),
                }
            }
        }
    }
}

async fn connect_once<C>(
    configuration: &TerminalConfiguration,
    connector: &C,
    commands: &mut tokio_mpsc::UnboundedReceiver<TerminalCommand>,
    events: &mpsc::Sender<TerminalEvent>,
    dispatch_history: &mut DispatchHistory,
) -> ConnectionOutcome
where
    C: Connector,
{
    let connection = connector.connect(configuration.core_websocket_url());
    tokio::pin!(connection);
    let mut connection = loop {
        tokio::select! {
            result = &mut connection => match result {
                Ok(connection) => break connection,
                Err(error) => return ConnectionOutcome::Disconnected {
                    reason: format!("could not connect to Core: {error}"),
                    was_registered: false,
                },
            },
            command = commands.recv() => match command {
                Some(TerminalCommand::Shutdown { .. }) | None => return ConnectionOutcome::Shutdown,
                Some(command) => publish_ignored_command(events, command, IgnoredCommandReason::NotReady),
            }
        }
    };

    let registration = match serialise_registration(configuration) {
        Ok(registration) => registration,
        Err(error) => {
            return ConnectionOutcome::Disconnected {
                reason: format!("could not serialise terminal registration: {error}"),
                was_registered: false,
            };
        }
    };
    if let Err(error) = connection.send_text(registration).await {
        return ConnectionOutcome::Disconnected {
            reason: format!("could not send terminal registration: {error}"),
            was_registered: false,
        };
    }

    let registration_timeout = tokio::time::sleep(configuration.registration_timeout());
    tokio::pin!(registration_timeout);
    loop {
        tokio::select! {
            () = &mut registration_timeout => {
                let _ = connection.close().await;
                return ConnectionOutcome::Disconnected {
                    reason: "Core did not acknowledge terminal registration in time".to_owned(),
                    was_registered: false,
                };
            }
            command = commands.recv() => match command {
                Some(TerminalCommand::Shutdown { .. }) | None => {
                    let _ = connection.close().await;
                    return ConnectionOutcome::Shutdown;
                }
                Some(command) => publish_ignored_command(events, command, IgnoredCommandReason::NotReady),
            },
            message = connection.receive_text() => {
                let text = match message {
                    Ok(Some(text)) => text,
                    Ok(None) => return ConnectionOutcome::Disconnected {
                        reason: "Core closed the connection before registration completed".to_owned(),
                        was_registered: false,
                    },
                    Err(error) => return ConnectionOutcome::Disconnected {
                        reason: format!("terminal connection failed during registration: {error}"),
                        was_registered: false,
                    },
                };
                let message = match serde_json::from_str::<CoreTerminalMessage>(&text) {
                    Ok(message) => message,
                    Err(error) => {
                        publish_protocol_error(events, format!("malformed Core terminal message: {error}"));
                        let _ = connection.close().await;
                        return ConnectionOutcome::Disconnected {
                            reason: "Core sent a malformed registration response".to_owned(),
                            was_registered: false,
                        };
                    }
                };
                match message {
                    CoreTerminalMessage::RegistrationAcknowledged {
                        terminal_id,
                        connection_id,
                        protocol_version,
                        heartbeat_interval_seconds,
                    } => {
                        if terminal_id != *configuration.terminal_id() {
                            publish_protocol_error(events, "Core acknowledged registration for another terminal".to_owned());
                            let _ = connection.close().await;
                            return ConnectionOutcome::Disconnected {
                                reason: "Core returned a foreign terminal identity".to_owned(),
                                was_registered: false,
                            };
                        }
                        if !ProtocolVersion::CURRENT.is_compatible_with(protocol_version) {
                            publish_protocol_error(
                                events,
                                format!(
                                    "Core acknowledged incompatible terminal protocol {}.{}",
                                    protocol_version.major, protocol_version.minor
                                ),
                            );
                            let _ = connection.close().await;
                            return ConnectionOutcome::Disconnected {
                                reason: "Core selected an incompatible terminal protocol".to_owned(),
                                was_registered: false,
                            };
                        }
                        if heartbeat_interval_seconds == 0 {
                            publish_protocol_error(events, "Core returned a zero heartbeat interval".to_owned());
                            let _ = connection.close().await;
                            return ConnectionOutcome::Disconnected {
                                reason: "Core returned an invalid heartbeat interval".to_owned(),
                                was_registered: false,
                            };
                        }
                        publish_state(
                            events,
                            ConnectionState::Registered {
                                terminal_id,
                                connection_id,
                            },
                        );
                        return run_registered(
                            configuration,
                            connection,
                            connection_id,
                            Duration::from_secs(u64::from(heartbeat_interval_seconds)),
                            commands,
                            events,
                            dispatch_history,
                        )
                        .await;
                    }
                    CoreTerminalMessage::RegistrationRejected { rejection } => {
                        let _ = events.send(TerminalEvent::RegistrationRejected(rejection));
                        let _ = connection.close().await;
                        return ConnectionOutcome::Disconnected {
                            reason: "Core rejected terminal registration".to_owned(),
                            was_registered: false,
                        };
                    }
                    _ => {
                        publish_protocol_error(
                            events,
                            "Core sent a terminal message before registration was acknowledged".to_owned(),
                        );
                        let _ = connection.close().await;
                        return ConnectionOutcome::Disconnected {
                            reason: "Core violated terminal readiness gating".to_owned(),
                            was_registered: false,
                        };
                    }
                }
            }
        }
    }
}

async fn run_registered(
    configuration: &TerminalConfiguration,
    mut connection: Box<dyn Connection>,
    connection_id: TerminalConnectionId,
    heartbeat_interval: Duration,
    commands: &mut tokio_mpsc::UnboundedReceiver<TerminalCommand>,
    events: &mpsc::Sender<TerminalEvent>,
    dispatch_history: &mut DispatchHistory,
) -> ConnectionOutcome {
    let mut pending = BTreeMap::<PresentationId, PendingPresentation>::new();
    let heartbeat = tokio::time::sleep(heartbeat_interval);
    tokio::pin!(heartbeat);

    loop {
        let next_deadline = pending
            .values()
            .map(|pending| pending.deadline)
            .min()
            .unwrap_or_else(|| TokioInstant::now() + Duration::from_secs(365 * 24 * 60 * 60));
        let acknowledgement = tokio::time::sleep_until(next_deadline);
        tokio::pin!(acknowledgement);

        tokio::select! {
            () = &mut heartbeat => {
                if let Err(error) = send_client_message(
                    connection.as_mut(),
                    &TerminalClientMessage::Heartbeat {
                        terminal_id: configuration.terminal_id().clone(),
                        connection_id,
                    },
                )
                .await
                {
                    return ConnectionOutcome::Disconnected {
                        reason: format!("could not send terminal heartbeat: {error}"),
                        was_registered: true,
                    };
                }
                heartbeat.as_mut().reset(TokioInstant::now() + heartbeat_interval);
            }
            () = &mut acknowledgement, if !pending.is_empty() => {
                let now = TokioInstant::now();
                let expired = pending
                    .iter()
                    .filter(|(_, pending)| pending.deadline <= now)
                    .map(|(presentation_id, _)| *presentation_id)
                    .collect::<Vec<_>>();
                for presentation_id in expired {
                    let pending = pending
                        .remove(&presentation_id)
                        .expect("an expired presentation was selected from pending work");
                    pending.set_status(PresentationStatus::Expired);
                    let _ = events.send(TerminalEvent::PresentationInvalidated {
                        completion: pending.completion.clone(),
                        reason: PresentationInvalidationReason::Expired,
                    });
                }
            }
            command = commands.recv() => match command {
                Some(TerminalCommand::PresentationAccepted { completion }) => {
                    let presentation_id = completion.presentation_id;
                    if pending
                        .get(&presentation_id)
                        .is_some_and(|pending| pending.completion == completion)
                    {
                        let pending = pending
                            .remove(&presentation_id)
                            .expect("a matching completion has pending work");
                        if pending.deadline <= TokioInstant::now() {
                            pending.set_status(PresentationStatus::Expired);
                            let _ = events.send(TerminalEvent::PresentationInvalidated {
                                completion: pending.completion.clone(),
                                reason: PresentationInvalidationReason::Expired,
                            });
                            let _ = events.send(TerminalEvent::CommandIgnored {
                                presentation_id,
                                reason: IgnoredCommandReason::UnknownOrSettledPresentation,
                            });
                            continue;
                        }
                        if let Err(error) = send_client_message(
                            connection.as_mut(),
                            &TerminalClientMessage::PresentationAccepted {
                                terminal_id: configuration.terminal_id().clone(),
                                connection_id,
                                presentation_id,
                            },
                        )
                        .await
                        {
                            return ConnectionOutcome::Disconnected {
                                reason: format!("could not acknowledge presentation acceptance: {error}"),
                                was_registered: true,
                            };
                        }
                        pending.set_status(PresentationStatus::Completed);
                    } else {
                        let _ = events.send(TerminalEvent::CommandIgnored {
                            presentation_id,
                            reason: IgnoredCommandReason::UnknownOrSettledPresentation,
                        });
                    }
                }
                Some(TerminalCommand::PresentationRejected {
                    completion,
                    rejection,
                }) => {
                    let presentation_id = completion.presentation_id;
                    if pending
                        .get(&presentation_id)
                        .is_some_and(|pending| pending.completion == completion)
                    {
                        let pending = pending
                            .remove(&presentation_id)
                            .expect("a matching completion has pending work");
                        if pending.deadline <= TokioInstant::now() {
                            pending.set_status(PresentationStatus::Expired);
                            let _ = events.send(TerminalEvent::PresentationInvalidated {
                                completion: pending.completion.clone(),
                                reason: PresentationInvalidationReason::Expired,
                            });
                            let _ = events.send(TerminalEvent::CommandIgnored {
                                presentation_id,
                                reason: IgnoredCommandReason::UnknownOrSettledPresentation,
                            });
                            continue;
                        }
                        if let Err(error) = send_rejection(
                            connection.as_mut(),
                            configuration.terminal_id(),
                            connection_id,
                            presentation_id,
                            rejection,
                        )
                        .await
                        {
                            return ConnectionOutcome::Disconnected {
                                reason: format!("could not acknowledge presentation rejection: {error}"),
                                was_registered: true,
                            };
                        }
                        pending.set_status(PresentationStatus::Completed);
                    } else {
                        let _ = events.send(TerminalEvent::CommandIgnored {
                            presentation_id,
                            reason: IgnoredCommandReason::UnknownOrSettledPresentation,
                        });
                    }
                }
                Some(TerminalCommand::Shutdown { reason }) => {
                    let _ = send_client_message(
                        connection.as_mut(),
                        &TerminalClientMessage::Disconnect {
                            terminal_id: configuration.terminal_id().clone(),
                            connection_id,
                            reason,
                        },
                    )
                    .await;
                    let _ = connection.close().await;
                    return ConnectionOutcome::Shutdown;
                }
                None => {
                    let _ = connection.close().await;
                    return ConnectionOutcome::Shutdown;
                }
            },
            message = connection.receive_text() => {
                let text = match message {
                    Ok(Some(text)) => text,
                    Ok(None) => return ConnectionOutcome::Disconnected {
                        reason: "Core closed the terminal connection".to_owned(),
                        was_registered: true,
                    },
                    Err(error) => return ConnectionOutcome::Disconnected {
                        reason: format!("terminal connection failed: {error}"),
                        was_registered: true,
                    },
                };
                let message = match serde_json::from_str::<CoreTerminalMessage>(&text) {
                    Ok(message) => message,
                    Err(error) => {
                        publish_protocol_error(events, format!("malformed Core terminal message: {error}"));
                        let _ = connection.close().await;
                        return ConnectionOutcome::Disconnected {
                            reason: "Core sent a malformed terminal message".to_owned(),
                            was_registered: true,
                        };
                    }
                };
                match message {
                    CoreTerminalMessage::HeartbeatAcknowledged {
                        connection_id: acknowledged_connection,
                    } if acknowledged_connection == connection_id => {}
                    CoreTerminalMessage::HeartbeatAcknowledged { .. } => {
                        publish_protocol_error(events, "Core acknowledged a heartbeat for another connection".to_owned());
                        let _ = connection.close().await;
                        return ConnectionOutcome::Disconnected {
                            reason: "Core returned a foreign connection identity".to_owned(),
                            was_registered: true,
                        };
                    }
                    CoreTerminalMessage::PresentationDispatch { presentation } => {
                        let presentation_id = presentation.request.id;
                        if !presentation
                            .resolved_target
                            .terminals
                            .contains(configuration.terminal_id())
                        {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::ForeignRecipient,
                            });
                            continue;
                        }
                        let Some(delivery) = presentation
                            .deliveries
                            .get(configuration.terminal_id())
                            .cloned()
                        else {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::MissingDeliveryContext,
                            });
                            continue;
                        };
                        if delivery.connection_id != connection_id {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::StaleConnection,
                            });
                            continue;
                        }
                        if dispatch_history.recent.contains(presentation_id) {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::DuplicatePresentation,
                            });
                            continue;
                        }
                        if delivery.generation.get() == 0 {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::InvalidGeneration,
                            });
                            continue;
                        }
                        if dispatch_history
                            .greatest_generation
                            .is_some_and(|greatest| delivery.generation <= greatest)
                        {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::OlderGeneration,
                            });
                            continue;
                        }

                        dispatch_history.greatest_generation = Some(delivery.generation);
                        dispatch_history.recent.insert(presentation_id);
                        let superseded = pending.keys().copied().collect::<Vec<_>>();
                        for superseded_id in superseded {
                            let superseded = pending
                                .remove(&superseded_id)
                                .expect("a selected presentation is pending");
                            superseded.set_status(PresentationStatus::Superseded);
                            let _ = events.send(TerminalEvent::PresentationInvalidated {
                                completion: superseded.completion.clone(),
                                reason: PresentationInvalidationReason::Superseded,
                            });
                        }

                        let validity = Duration::from_millis(delivery.valid_for_millis);
                        let runtime_now = TokioInstant::now();
                        let local_now = StdInstant::now();
                        let Some(deadline) = runtime_now.checked_add(validity) else {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::InvalidValidity,
                            });
                            continue;
                        };
                        let Some(valid_until) = local_now.checked_add(validity) else {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::InvalidValidity,
                            });
                            continue;
                        };
                        if validity.is_zero() {
                            let _ = events.send(TerminalEvent::DispatchIgnored {
                                presentation_id,
                                reason: IgnoredDispatchReason::Expired,
                            });
                            continue;
                        }
                        if !configuration
                            .capabilities()
                            .supports_all(&presentation.request.required_capabilities)
                        {
                            let rejection = PresentationRejection {
                                code: PresentationRejectionCode::new(
                                    PresentationRejectionCode::UNSUPPORTED_CAPABILITIES,
                                )
                                .expect("built-in rejection code is valid"),
                                detail: None,
                            };
                            if let Err(error) = send_rejection(
                                connection.as_mut(),
                                configuration.terminal_id(),
                                connection_id,
                                presentation_id,
                                rejection,
                            )
                            .await
                            {
                                return ConnectionOutcome::Disconnected {
                                    reason: format!("could not reject incompatible presentation: {error}"),
                                    was_registered: true,
                                };
                            }
                            continue;
                        }
                        if pending.len() >= configuration.maximum_pending_presentations() {
                            let rejection = PresentationRejection {
                                code: PresentationRejectionCode::new(PresentationRejectionCode::BUSY)
                                    .expect("built-in rejection code is valid"),
                                detail: Some("terminal presentation queue is full".to_owned()),
                            };
                            if let Err(error) = send_rejection(
                                connection.as_mut(),
                                configuration.terminal_id(),
                                connection_id,
                                presentation_id,
                                rejection,
                            )
                            .await
                            {
                                return ConnectionOutcome::Disconnected {
                                    reason: format!("could not reject queued presentation: {error}"),
                                    was_registered: true,
                                };
                            }
                            continue;
                        }
                        let completion = PresentationCompletion {
                            presentation_id,
                            connection_id,
                            generation: delivery.generation,
                        };
                        let status = Arc::new(AtomicU8::new(PresentationStatus::Active.encode()));
                        pending.insert(
                            presentation_id,
                            PendingPresentation {
                                completion: completion.clone(),
                                deadline,
                                status: status.clone(),
                            },
                        );
                        let _ = events.send(TerminalEvent::PresentationReceived(PresentationWork {
                            presentation,
                            delivery,
                            valid_until,
                            completion,
                            status,
                        }));
                    }
                    CoreTerminalMessage::RegistrationAcknowledged { .. }
                    | CoreTerminalMessage::RegistrationRejected { .. } => {
                        publish_protocol_error(
                            events,
                            "Core sent a registration response after the terminal became ready".to_owned(),
                        );
                        let _ = connection.close().await;
                        return ConnectionOutcome::Disconnected {
                            reason: "Core sent an unexpected registration response".to_owned(),
                            was_registered: true,
                        };
                    }
                }
            }
        }
    }
}

struct PendingPresentation {
    completion: PresentationCompletion,
    deadline: TokioInstant,
    status: Arc<AtomicU8>,
}

impl PendingPresentation {
    fn set_status(&self, status: PresentationStatus) {
        self.status.store(status.encode(), Ordering::Release);
    }
}

impl Drop for PendingPresentation {
    fn drop(&mut self) {
        let _ = self.status.compare_exchange(
            PresentationStatus::Active.encode(),
            PresentationStatus::ConnectionLost.encode(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

#[derive(Default)]
struct DispatchHistory {
    recent: RecentPresentationIds,
    greatest_generation: Option<PresentationGeneration>,
}

#[derive(Default)]
struct RecentPresentationIds {
    ids: BTreeSet<PresentationId>,
    order: VecDeque<PresentationId>,
}

impl RecentPresentationIds {
    fn contains(&self, presentation_id: PresentationId) -> bool {
        self.ids.contains(&presentation_id)
    }

    fn insert(&mut self, presentation_id: PresentationId) -> bool {
        if !self.ids.insert(presentation_id) {
            return false;
        }

        self.order.push_back(presentation_id);
        if self.order.len() > RECENT_PRESENTATION_CAPACITY {
            let expired = self
                .order
                .pop_front()
                .expect("a recent presentation was just inserted");
            self.ids.remove(&expired);
        }
        true
    }
}

async fn send_rejection(
    connection: &mut dyn Connection,
    terminal_id: &TerminalId,
    connection_id: TerminalConnectionId,
    presentation_id: PresentationId,
    rejection: PresentationRejection,
) -> Result<(), crate::transport::TransportError> {
    send_client_message(
        connection,
        &TerminalClientMessage::PresentationRejected {
            terminal_id: terminal_id.clone(),
            connection_id,
            presentation_id,
            rejection,
        },
    )
    .await
}

async fn send_client_message(
    connection: &mut dyn Connection,
    message: &TerminalClientMessage,
) -> Result<(), crate::transport::TransportError> {
    let message = serde_json::to_string(message)
        .map_err(|error| crate::transport::TransportError::new(error.to_string()))?;
    connection.send_text(message).await
}

fn serialise_registration(
    configuration: &TerminalConfiguration,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(&TerminalClientMessage::Register {
        registration: configuration.registration().clone(),
    })
}

fn publish_state(events: &mpsc::Sender<TerminalEvent>, state: ConnectionState) {
    let _ = events.send(TerminalEvent::ConnectionStateChanged(state));
}

fn publish_protocol_error(events: &mpsc::Sender<TerminalEvent>, detail: String) {
    let _ = events.send(TerminalEvent::ProtocolError { detail });
}

fn publish_ignored_command(
    events: &mpsc::Sender<TerminalEvent>,
    command: TerminalCommand,
    reason: IgnoredCommandReason,
) {
    let presentation_id = match command {
        TerminalCommand::PresentationAccepted { completion }
        | TerminalCommand::PresentationRejected { completion, .. } => completion.presentation_id,
        TerminalCommand::Shutdown { .. } => return,
    };
    let _ = events.send(TerminalEvent::CommandIgnored {
        presentation_id,
        reason,
    });
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use bts_protocol::{
        CoreTerminalMessage, DisplayState, PresentationRequest, RegistrationRejectionReason,
        ResolvedTarget, TerminalCapabilities, TerminalCapability, TerminalImplementationId,
        TerminalName, TerminalTarget,
    };
    use semver::Version;
    use serde_json::{Value, json};
    use tokio::{sync::Mutex as AsyncMutex, task::JoinHandle};

    use super::*;
    use crate::{ReconnectPolicy, RuntimeDiagnostics, transport::TransportError};

    enum Inbound {
        Text(String),
        Closed,
    }

    #[derive(Debug)]
    enum Outbound {
        Text(String),
        Closed,
    }

    struct FakeConnection {
        inbound: tokio_mpsc::UnboundedReceiver<Inbound>,
        outbound: tokio_mpsc::UnboundedSender<Outbound>,
    }

    #[async_trait]
    impl Connection for FakeConnection {
        async fn send_text(&mut self, message: String) -> Result<(), TransportError> {
            self.outbound
                .send(Outbound::Text(message))
                .map_err(|_| TransportError::new("fake peer disconnected"))
        }

        async fn receive_text(&mut self) -> Result<Option<String>, TransportError> {
            Ok(match self.inbound.recv().await {
                Some(Inbound::Text(message)) => Some(message),
                Some(Inbound::Closed) | None => None,
            })
        }

        async fn close(&mut self) -> Result<(), TransportError> {
            let _ = self.outbound.send(Outbound::Closed);
            Ok(())
        }
    }

    struct FakePeer {
        inbound: tokio_mpsc::UnboundedSender<Inbound>,
        outbound: tokio_mpsc::UnboundedReceiver<Outbound>,
    }

    impl FakePeer {
        async fn next_text(&mut self) -> String {
            match self.outbound.recv().await.expect("runtime closed") {
                Outbound::Text(message) => message,
                Outbound::Closed => panic!("runtime closed before sending a message"),
            }
        }

        async fn next_client_message(&mut self) -> TerminalClientMessage {
            serde_json::from_str(&self.next_text().await).unwrap()
        }

        fn send_core(&self, message: CoreTerminalMessage) {
            self.inbound
                .send(Inbound::Text(serde_json::to_string(&message).unwrap()))
                .unwrap();
        }

        fn send_raw(&self, message: impl Into<String>) {
            self.inbound.send(Inbound::Text(message.into())).unwrap();
        }

        fn close_from_core(&self) {
            self.inbound.send(Inbound::Closed).unwrap();
        }

        fn assert_no_outbound(&mut self) {
            assert!(
                self.outbound.try_recv().is_err(),
                "runtime unexpectedly sent a message"
            );
        }
    }

    fn fake_connection() -> (Box<dyn Connection>, FakePeer) {
        let (inbound_sender, inbound_receiver) = tokio_mpsc::unbounded_channel();
        let (outbound_sender, outbound_receiver) = tokio_mpsc::unbounded_channel();
        (
            Box::new(FakeConnection {
                inbound: inbound_receiver,
                outbound: outbound_sender,
            }),
            FakePeer {
                inbound: inbound_sender,
                outbound: outbound_receiver,
            },
        )
    }

    struct FakeConnector {
        connections:
            AsyncMutex<tokio_mpsc::UnboundedReceiver<Result<Box<dyn Connection>, TransportError>>>,
        calls: tokio_mpsc::UnboundedSender<String>,
    }

    #[async_trait]
    impl Connector for FakeConnector {
        async fn connect(&self, url: &str) -> Result<Box<dyn Connection>, TransportError> {
            self.calls.send(url.to_owned()).unwrap();
            self.connections
                .lock()
                .await
                .recv()
                .await
                .unwrap_or_else(|| Err(TransportError::new("connector stopped")))
        }
    }

    struct Harness {
        configuration: TerminalConfiguration,
        connections: tokio_mpsc::UnboundedSender<Result<Box<dyn Connection>, TransportError>>,
        calls: tokio_mpsc::UnboundedReceiver<String>,
        commands: tokio_mpsc::UnboundedSender<TerminalCommand>,
        events: Receiver<TerminalEvent>,
        task: JoinHandle<()>,
    }

    impl Harness {
        fn start() -> Self {
            let terminal_id = TerminalId::new("hall-display").unwrap();
            let capability = TerminalCapability::new(TerminalCapability::RENDER_TEXT).unwrap();
            let diagnostics =
                RuntimeDiagnostics::new([("os.name".to_owned(), "Test OS".to_owned())]).unwrap();
            let configuration = TerminalConfiguration::new(
                "ws://core.test/terminals",
                terminal_id,
                TerminalName::new("Hall Display").unwrap(),
                TerminalImplementationId::new("test-terminal").unwrap(),
                Version::new(1, 2, 3),
                TerminalCapabilities::new([capability]),
            )
            .unwrap()
            .with_runtime_diagnostics(diagnostics)
            .with_reconnect_policy(
                ReconnectPolicy::new(Duration::from_secs(1), Duration::from_secs(4)).unwrap(),
            )
            .with_registration_timeout(Duration::from_secs(5))
            .unwrap();

            let (connection_sender, connection_receiver) = tokio_mpsc::unbounded_channel();
            let (call_sender, call_receiver) = tokio_mpsc::unbounded_channel();
            let connector = Arc::new(FakeConnector {
                connections: AsyncMutex::new(connection_receiver),
                calls: call_sender,
            });
            let (command_sender, command_receiver) = tokio_mpsc::unbounded_channel();
            let (event_sender, event_receiver) = mpsc::channel();
            let task_configuration = configuration.clone();
            let task = tokio::spawn(async move {
                run_runtime(
                    task_configuration,
                    connector,
                    command_receiver,
                    event_sender,
                )
                .await;
            });

            Self {
                configuration,
                connections: connection_sender,
                calls: call_receiver,
                commands: command_sender,
                events: event_receiver,
                task,
            }
        }

        async fn open_connection(&mut self) -> (FakePeer, Value) {
            let url = self.calls.recv().await.unwrap();
            assert_eq!(url, self.configuration.core_websocket_url());
            let (connection, mut peer) = fake_connection();
            self.connections.send(Ok(connection)).unwrap();
            let registration = serde_json::from_str(&peer.next_text().await).unwrap();
            (peer, registration)
        }

        async fn fail_connection(&mut self, detail: &str) {
            let url = self.calls.recv().await.unwrap();
            assert_eq!(url, self.configuration.core_websocket_url());
            self.connections
                .send(Err(TransportError::new(detail)))
                .unwrap();
        }

        async fn next_event_matching(
            &self,
            predicate: impl Fn(&TerminalEvent) -> bool,
        ) -> TerminalEvent {
            for _ in 0..200 {
                while let Ok(event) = self.events.try_recv() {
                    if predicate(&event) {
                        return event;
                    }
                }
                tokio::task::yield_now().await;
            }
            panic!("matching terminal event was not published")
        }

        async fn next_presentation(&self) -> PresentationWork {
            match self
                .next_event_matching(|event| {
                    matches!(event, TerminalEvent::PresentationReceived(_))
                })
                .await
            {
                TerminalEvent::PresentationReceived(work) => work,
                _ => unreachable!("the event predicate selected a presentation"),
            }
        }

        async fn finish(self) {
            self.commands
                .send(TerminalCommand::Shutdown { reason: None })
                .unwrap();
            self.task.await.unwrap();
        }
    }

    fn acknowledgement(
        configuration: &TerminalConfiguration,
        connection_id: TerminalConnectionId,
        heartbeat_interval_seconds: u32,
    ) -> CoreTerminalMessage {
        CoreTerminalMessage::RegistrationAcknowledged {
            terminal_id: configuration.terminal_id().clone(),
            connection_id,
            protocol_version: ProtocolVersion::CURRENT,
            heartbeat_interval_seconds,
        }
    }

    fn dispatch(
        requested_terminal: &TerminalId,
        recipients: impl IntoIterator<Item = TerminalId>,
        delivery_terminal: &TerminalId,
        connection_id: TerminalConnectionId,
        generation: u64,
        valid_for: Duration,
    ) -> PresentationDispatch {
        let target = TerminalTarget::Terminal {
            id: requested_terminal.clone(),
            scope: Default::default(),
        };
        let request = PresentationRequest {
            id: PresentationId::new(),
            target: target.clone(),
            required_capabilities: TerminalCapabilities::default(),
            display: DisplayState::Message {
                title: "Test".to_owned(),
                body: "Presentation".to_owned(),
            },
        };
        PresentationDispatch::with_deliveries(
            request,
            ResolvedTarget::new(target, recipients).unwrap(),
            BTreeMap::from([(
                delivery_terminal.clone(),
                PresentationDeliveryContext {
                    connection_id,
                    generation: PresentationGeneration::new(generation),
                    valid_for_millis: u64::try_from(valid_for.as_millis()).unwrap(),
                },
            )]),
        )
        .unwrap()
    }

    async fn register(
        harness: &mut Harness,
        peer: &FakePeer,
        connection_id: TerminalConnectionId,
        heartbeat_interval_seconds: u32,
    ) {
        peer.send_core(acknowledgement(
            &harness.configuration,
            connection_id,
            heartbeat_interval_seconds,
        ));
        let registered = harness
            .next_event_matching(|event| {
                matches!(
                    event,
                    TerminalEvent::ConnectionStateChanged(ConnectionState::Registered { .. })
                )
            })
            .await;
        assert_eq!(
            registered,
            TerminalEvent::ConnectionStateChanged(ConnectionState::Registered {
                terminal_id: harness.configuration.terminal_id().clone(),
                connection_id,
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn registration_wire_is_typed_and_gates_readiness() {
        let mut harness = Harness::start();
        let (peer, registration) = harness.open_connection().await;
        assert_eq!(registration["message"], "register");
        assert_eq!(
            registration["registration"]["identity"]["id"],
            "hall-display"
        );
        assert_eq!(
            registration["registration"]["identity"]["name"],
            "Hall Display"
        );
        assert!(registration["registration"]["implementation_version"].is_null());
        assert!(registration["registration"]["runtime_diagnostics"].is_null());
        assert_eq!(
            harness.configuration.implementation_version(),
            &Version::new(1, 2, 3)
        );
        assert_eq!(
            harness.configuration.runtime_diagnostics().iter().count(),
            1
        );
        assert!(!harness.events.try_iter().any(|event| matches!(
            event,
            TerminalEvent::ConnectionStateChanged(ConnectionState::Registered { .. })
        )));

        let connection_id = TerminalConnectionId::new();
        register(&mut harness, &peer, connection_id, 60).await;
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn duplicate_terminal_registration_is_observable_and_retried() {
        let mut harness = Harness::start();
        let (peer, _) = harness.open_connection().await;
        let rejection = RegistrationRejection {
            terminal_id: Some(harness.configuration.terminal_id().clone()),
            reason: RegistrationRejectionReason::DuplicateTerminalId,
        };
        peer.send_core(CoreTerminalMessage::RegistrationRejected {
            rejection: rejection.clone(),
        });

        assert_eq!(
            harness
                .next_event_matching(|event| {
                    matches!(event, TerminalEvent::RegistrationRejected(_))
                })
                .await,
            TerminalEvent::RegistrationRejected(rejection)
        );
        assert_eq!(
            harness
                .next_event_matching(|event| {
                    matches!(
                        event,
                        TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying { .. })
                    )
                })
                .await,
            TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying {
                attempt: 1,
                delay: Duration::from_secs(1),
            })
        );
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_uses_the_owning_connection_and_schedule() {
        let mut harness = Harness::start();
        let (mut peer, _) = harness.open_connection().await;
        let connection_id = TerminalConnectionId::new();
        register(&mut harness, &peer, connection_id, 5).await;

        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        peer.assert_no_outbound();
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            peer.next_client_message().await,
            TerminalClientMessage::Heartbeat {
                terminal_id: harness.configuration.terminal_id().clone(),
                connection_id,
            }
        );
        peer.send_core(CoreTerminalMessage::HeartbeatAcknowledged { connection_id });
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_preserves_identity_and_resets_backoff_after_registration() {
        let mut harness = Harness::start();
        harness.fail_connection("first failure").await;
        assert_eq!(
            harness
                .next_event_matching(|event| {
                    matches!(
                        event,
                        TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying { .. })
                    )
                })
                .await,
            TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying {
                attempt: 1,
                delay: Duration::from_secs(1),
            })
        );

        tokio::time::advance(Duration::from_millis(999)).await;
        tokio::task::yield_now().await;
        assert!(harness.calls.try_recv().is_err());
        tokio::time::advance(Duration::from_millis(1)).await;
        let (second_peer, second_registration) = harness.open_connection().await;
        assert_eq!(
            second_registration["registration"]["identity"]["id"],
            harness.configuration.terminal_id().as_str()
        );
        let second_connection = TerminalConnectionId::new();
        register(&mut harness, &second_peer, second_connection, 60).await;
        second_peer.close_from_core();

        assert_eq!(
            harness
                .next_event_matching(|event| {
                    matches!(
                        event,
                        TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying { .. })
                    )
                })
                .await,
            TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying {
                attempt: 1,
                delay: Duration::from_secs(1),
            })
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        let (_third_peer, third_registration) = harness.open_connection().await;
        assert_eq!(
            third_registration["registration"]["identity"]["id"],
            harness.configuration.terminal_id().as_str()
        );
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn foreign_dispatches_are_never_applied_or_acknowledged() {
        let mut harness = Harness::start();
        let (mut peer, _) = harness.open_connection().await;
        let connection_id = TerminalConnectionId::new();
        register(&mut harness, &peer, connection_id, 60).await;

        let foreign = TerminalId::new("foreign-display").unwrap();
        let presentation = dispatch(
            &foreign,
            [foreign.clone()],
            &foreign,
            connection_id,
            1,
            Duration::from_secs(10),
        );
        let presentation_id = presentation.request.id;
        peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(presentation),
        });
        assert_eq!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::DispatchIgnored { .. }))
                .await,
            TerminalEvent::DispatchIgnored {
                presentation_id,
                reason: IgnoredDispatchReason::ForeignRecipient,
            }
        );
        peer.assert_no_outbound();

        let mut missing_context = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            connection_id,
            1,
            Duration::from_secs(10),
        );
        let missing_id = missing_context.request.id;
        missing_context.deliveries.clear();
        peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(missing_context),
        });
        assert_eq!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::DispatchIgnored { .. }))
                .await,
            TerminalEvent::DispatchIgnored {
                presentation_id: missing_id,
                reason: IgnoredDispatchReason::MissingDeliveryContext,
            }
        );
        peer.assert_no_outbound();
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn settled_dispatch_is_not_reapplied_after_reconnect() {
        let mut harness = Harness::start();
        let (mut first_peer, _) = harness.open_connection().await;
        let first_connection = TerminalConnectionId::new();
        register(&mut harness, &first_peer, first_connection, 60).await;

        let presentation = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            first_connection,
            1,
            Duration::from_secs(10),
        );
        let presentation_id = presentation.request.id;
        first_peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(presentation.clone()),
        });
        let work = harness.next_presentation().await;
        harness
            .commands
            .send(TerminalCommand::PresentationAccepted {
                completion: work.completion().clone(),
            })
            .unwrap();
        assert!(matches!(
            first_peer.next_client_message().await,
            TerminalClientMessage::PresentationAccepted {
                presentation_id: accepted,
                ..
            } if accepted == presentation_id
        ));
        first_peer.close_from_core();
        harness
            .next_event_matching(|event| {
                matches!(
                    event,
                    TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying { .. })
                )
            })
            .await;

        tokio::time::advance(Duration::from_secs(1)).await;
        let (mut second_peer, _) = harness.open_connection().await;
        let second_connection = TerminalConnectionId::new();
        register(&mut harness, &second_peer, second_connection, 60).await;
        second_peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(presentation),
        });
        assert_eq!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::DispatchIgnored { .. }))
                .await,
            TerminalEvent::DispatchIgnored {
                presentation_id,
                reason: IgnoredDispatchReason::StaleConnection,
            }
        );
        second_peer.assert_no_outbound();
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn newer_generation_supersedes_work_and_order_survives_reconnect() {
        let mut harness = Harness::start();
        let (mut first_peer, _) = harness.open_connection().await;
        let first_connection = TerminalConnectionId::new();
        register(&mut harness, &first_peer, first_connection, 60).await;

        let older = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            first_connection,
            5,
            Duration::from_secs(10),
        );
        first_peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(older),
        });
        let older_work = harness.next_presentation().await;

        let newer = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            first_connection,
            6,
            Duration::from_secs(10),
        );
        first_peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(newer),
        });
        assert_eq!(
            harness
                .next_event_matching(|event| {
                    matches!(
                        event,
                        TerminalEvent::PresentationInvalidated {
                            reason: PresentationInvalidationReason::Superseded,
                            ..
                        }
                    )
                })
                .await,
            TerminalEvent::PresentationInvalidated {
                completion: older_work.completion().clone(),
                reason: PresentationInvalidationReason::Superseded,
            }
        );
        assert_eq!(older_work.status(), PresentationStatus::Superseded);
        let newer_work = harness.next_presentation().await;
        assert_eq!(newer_work.completion().generation().get(), 6);

        harness
            .commands
            .send(TerminalCommand::PresentationAccepted {
                completion: older_work.completion().clone(),
            })
            .unwrap();
        assert!(matches!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::CommandIgnored { .. }))
                .await,
            TerminalEvent::CommandIgnored {
                reason: IgnoredCommandReason::UnknownOrSettledPresentation,
                ..
            }
        ));
        first_peer.assert_no_outbound();
        first_peer.close_from_core();
        harness
            .next_event_matching(|event| {
                matches!(
                    event,
                    TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying { .. })
                )
            })
            .await;
        assert_eq!(newer_work.status(), PresentationStatus::ConnectionLost);

        tokio::time::advance(Duration::from_secs(1)).await;
        let (mut second_peer, _) = harness.open_connection().await;
        let second_connection = TerminalConnectionId::new();
        register(&mut harness, &second_peer, second_connection, 60).await;
        let delayed = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            second_connection,
            4,
            Duration::from_secs(10),
        );
        let delayed_id = delayed.request.id;
        second_peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(delayed),
        });
        assert_eq!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::DispatchIgnored { .. }))
                .await,
            TerminalEvent::DispatchIgnored {
                presentation_id: delayed_id,
                reason: IgnoredDispatchReason::OlderGeneration,
            }
        );
        second_peer.assert_no_outbound();
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn consumer_acceptance_and_rejection_include_connection_ownership() {
        let mut harness = Harness::start();
        let (mut peer, _) = harness.open_connection().await;
        let connection_id = TerminalConnectionId::new();
        register(&mut harness, &peer, connection_id, 60).await;

        let accepted = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            connection_id,
            1,
            Duration::from_secs(10),
        );
        let accepted_id = accepted.request.id;
        peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(accepted),
        });
        let accepted_work = harness.next_presentation().await;
        assert_eq!(accepted_work.presentation().request.id, accepted_id);
        assert_eq!(accepted_work.delivery().generation.get(), 1);
        assert_eq!(accepted_work.completion().connection_id(), connection_id);
        harness
            .commands
            .send(TerminalCommand::PresentationAccepted {
                completion: accepted_work.completion().clone(),
            })
            .unwrap();
        assert_eq!(
            peer.next_client_message().await,
            TerminalClientMessage::PresentationAccepted {
                terminal_id: harness.configuration.terminal_id().clone(),
                connection_id,
                presentation_id: accepted_id,
            }
        );
        assert_eq!(accepted_work.status(), PresentationStatus::Completed);
        harness
            .commands
            .send(TerminalCommand::PresentationAccepted {
                completion: accepted_work.completion().clone(),
            })
            .unwrap();
        assert_eq!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::CommandIgnored { .. }))
                .await,
            TerminalEvent::CommandIgnored {
                presentation_id: accepted_id,
                reason: IgnoredCommandReason::UnknownOrSettledPresentation,
            }
        );
        peer.assert_no_outbound();

        let rejected = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            connection_id,
            2,
            Duration::from_secs(10),
        );
        let rejected_id = rejected.request.id;
        peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(rejected),
        });
        let rejected_work = harness.next_presentation().await;
        let rejection = PresentationRejection {
            code: PresentationRejectionCode::new(PresentationRejectionCode::BUSY).unwrap(),
            detail: Some("renderer is busy".to_owned()),
        };
        harness
            .commands
            .send(TerminalCommand::PresentationRejected {
                completion: rejected_work.completion().clone(),
                rejection: rejection.clone(),
            })
            .unwrap();

        assert_eq!(
            peer.next_client_message().await,
            TerminalClientMessage::PresentationRejected {
                terminal_id: harness.configuration.terminal_id().clone(),
                connection_id,
                presentation_id: rejected_id,
                rejection,
            }
        );
        assert_eq!(rejected_work.status(), PresentationStatus::Completed);
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn wire_expiry_invalidates_work_without_an_early_local_rejection() {
        let mut harness = Harness::start();
        let (mut peer, _) = harness.open_connection().await;
        let connection_id = TerminalConnectionId::new();
        register(&mut harness, &peer, connection_id, 60).await;

        let presentation = dispatch(
            harness.configuration.terminal_id(),
            [harness.configuration.terminal_id().clone()],
            harness.configuration.terminal_id(),
            connection_id,
            1,
            Duration::from_secs(3),
        );
        let presentation_id = presentation.request.id;
        peer.send_core(CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(presentation),
        });
        let work = harness.next_presentation().await;
        assert_eq!(work.status(), PresentationStatus::Active);
        tokio::time::advance(Duration::from_secs(3)).await;

        assert_eq!(
            harness
                .next_event_matching(|event| {
                    matches!(
                        event,
                        TerminalEvent::PresentationInvalidated {
                            reason: PresentationInvalidationReason::Expired,
                            ..
                        }
                    )
                })
                .await,
            TerminalEvent::PresentationInvalidated {
                completion: work.completion().clone(),
                reason: PresentationInvalidationReason::Expired,
            }
        );
        assert_eq!(work.status(), PresentationStatus::Expired);
        peer.assert_no_outbound();

        harness
            .commands
            .send(TerminalCommand::PresentationAccepted {
                completion: work.completion().clone(),
            })
            .unwrap();
        assert_eq!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::CommandIgnored { .. }))
                .await,
            TerminalEvent::CommandIgnored {
                presentation_id,
                reason: IgnoredCommandReason::UnknownOrSettledPresentation,
            }
        );
        peer.assert_no_outbound();
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn malformed_and_incompatible_messages_are_rejected_safely() {
        let mut harness = Harness::start();
        let (first_peer, _) = harness.open_connection().await;
        first_peer.send_raw("{not-json");
        assert!(matches!(
            harness
                .next_event_matching(|event| matches!(event, TerminalEvent::ProtocolError { .. }))
                .await,
            TerminalEvent::ProtocolError { .. }
        ));
        harness
            .next_event_matching(|event| {
                matches!(
                    event,
                    TerminalEvent::ConnectionStateChanged(ConnectionState::Retrying { .. })
                )
            })
            .await;

        tokio::time::advance(Duration::from_secs(1)).await;
        let (second_peer, _) = harness.open_connection().await;
        second_peer.send_raw(
            json!({
                "message": "registration_acknowledged",
                "terminal_id": harness.configuration.terminal_id(),
                "connection_id": TerminalConnectionId::new(),
                "protocol_version": { "major": 0, "minor": 4 },
                "heartbeat_interval_seconds": 5
            })
            .to_string(),
        );
        let error = harness
            .next_event_matching(|event| matches!(event, TerminalEvent::ProtocolError { .. }))
            .await;
        assert!(matches!(
            error,
            TerminalEvent::ProtocolError { detail } if detail.contains("incompatible")
        ));
        harness.finish().await;
    }

    #[tokio::test(start_paused = true)]
    async fn graceful_shutdown_sends_disconnect_and_leaves_no_worker_task() {
        let mut harness = Harness::start();
        let (mut peer, _) = harness.open_connection().await;
        let connection_id = TerminalConnectionId::new();
        register(&mut harness, &peer, connection_id, 60).await;

        harness
            .commands
            .send(TerminalCommand::Shutdown {
                reason: Some("application closing".to_owned()),
            })
            .unwrap();
        assert_eq!(
            peer.next_client_message().await,
            TerminalClientMessage::Disconnect {
                terminal_id: harness.configuration.terminal_id().clone(),
                connection_id,
                reason: Some("application closing".to_owned()),
            }
        );
        harness.task.await.unwrap();
    }

    #[test]
    fn registration_serialisation_uses_only_the_typed_protocol_contract() {
        let configuration = Harness::start_configuration_for_sync_test();
        let value = serialise_registration(&configuration).unwrap();
        let message: TerminalClientMessage = serde_json::from_str(&value).unwrap();
        assert_eq!(
            message,
            TerminalClientMessage::Register {
                registration: configuration.registration().clone(),
            }
        );
    }

    impl Harness {
        fn start_configuration_for_sync_test() -> TerminalConfiguration {
            TerminalConfiguration::new(
                "ws://core.test/terminals",
                TerminalId::new("sync-test").unwrap(),
                TerminalName::new("Sync Test").unwrap(),
                TerminalImplementationId::new("test-terminal").unwrap(),
                Version::new(1, 0, 0),
                TerminalCapabilities::default(),
            )
            .unwrap()
        }
    }
}
