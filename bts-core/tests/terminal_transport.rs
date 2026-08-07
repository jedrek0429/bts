use std::{path::PathBuf, time::Duration};

use bts_core::server::{CoreConfiguration, CoreServer, CoreServices};
use bts_protocol::{
    DisplayState, EventKind, NewEvent, PresentationDeliveryOutcome, PresentationId,
    PresentationRejection, PresentationRejectionCode, PresentationRequest, ProtocolVersion,
    RegistrationRejectionReason, ScreenKind, TargetScope, TelephonyTargets, TerminalCapabilities,
    TerminalCapability, TerminalClientMessage, TerminalEvent as ProtocolTerminalEvent,
    TerminalEventKind, TerminalId, TerminalIdentity, TerminalImplementationId, TerminalName,
    TerminalRegistration, TerminalRuntimeDiagnostics, TerminalTarget,
    addons::v1::{API_VERSION, AddonCapability, AddonId, AddonManifest, AddonVersion},
    core::{
        CORE_EVENTS_PATH, CORE_TELEPHONY_TARGETS_PATH, CORE_TERMINAL_EVENTS_WEBSOCKET_PATH,
        CORE_TERMINALS_WEBSOCKET_PATH,
    },
};
use bts_terminal::{
    ConnectionState, RuntimeDiagnostics, TerminalConfiguration, TerminalEvent, TerminalHandle,
    TerminalRuntime,
};
use futures_util::{SinkExt, StreamExt};
use semver::Version;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio_tungstenite::{connect_async, tungstenite::Message};

struct RunningCore {
    _directory: TempDir,
    state_path: PathBuf,
    http_url: String,
    terminal_url: String,
    services: CoreServices,
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl RunningCore {
    async fn start() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("terminals.json");
        let configuration = CoreConfiguration {
            terminal_state_path: state_path.clone(),
            presence_timeout: Duration::from_secs(60),
            acknowledgement_timeout: Duration::from_secs(30),
            presence_expiry_interval: Duration::from_secs(3600),
            acknowledgement_expiry_interval: Duration::from_secs(3600),
        };
        let server = CoreServer::new(configuration).unwrap();
        let services = server.services();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let task = tokio::spawn(server.serve(listener, Some(ready_sender), async move {
            let _ = shutdown_receiver.await;
        }));
        let address = ready_receiver.await.unwrap();
        Self {
            _directory: directory,
            state_path,
            http_url: format!("http://{address}"),
            terminal_url: format!("ws://{address}{CORE_TERMINALS_WEBSOCKET_PATH}"),
            services,
            shutdown: shutdown_sender,
            task,
        }
    }

    async fn register_addon(&self) {
        let manifest = AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new("test-addon"),
            name: "Test addon".to_owned(),
            version: AddonVersion::new(1, 0, 0),
            actions: Vec::new(),
            menu: Vec::new(),
            capabilities: vec![AddonCapability::Display],
            screens: vec![ScreenKind::Message],
        };
        self.post(EventKind::AddonRegistered { manifest }).await;
    }

    async fn post(&self, kind: EventKind) {
        reqwest::Client::new()
            .post(format!("{}{}", self.http_url, CORE_EVENTS_PATH))
            .json(&NewEvent {
                source: "test-addon".to_owned(),
                kind,
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    async fn request(&self, id: PresentationId, terminal_id: &TerminalId) {
        self.post(EventKind::PresentationRequested {
            request: PresentationRequest {
                id,
                target: TerminalTarget::Terminal {
                    id: terminal_id.clone(),
                    scope: TargetScope::Online,
                },
                required_capabilities: TerminalCapabilities::default(),
                display: DisplayState::Message {
                    title: "Integration".to_owned(),
                    body: "Production transport".to_owned(),
                },
            },
        })
        .await;
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        self.task.await.unwrap().unwrap();
    }
}

fn terminal_configuration(url: &str, id: &str) -> TerminalConfiguration {
    TerminalConfiguration::new(
        url,
        TerminalId::new(id).unwrap(),
        TerminalName::new(format!("{id} terminal")).unwrap(),
        TerminalImplementationId::new("integration-terminal").unwrap(),
        Version::new(1, 2, 3),
        TerminalCapabilities::new([
            TerminalCapability::new(TerminalCapability::RENDER_TEXT).unwrap()
        ]),
    )
    .unwrap()
    .with_runtime_diagnostics(
        RuntimeDiagnostics::new([
            ("platform".to_owned(), "test".to_owned()),
            ("display.resolution".to_owned(), "1280x720".to_owned()),
        ])
        .unwrap(),
    )
}

fn next_registered(handle: &TerminalHandle) -> bts_protocol::TerminalConnectionId {
    loop {
        if let TerminalEvent::ConnectionStateChanged(ConnectionState::Registered {
            connection_id,
            ..
        }) = handle.next_event_timeout(Duration::from_secs(5)).unwrap()
        {
            return connection_id;
        }
    }
}

fn next_work(handle: &TerminalHandle) -> bts_terminal::PresentationWork {
    loop {
        if let TerminalEvent::PresentationReceived(work) =
            handle.next_event_timeout(Duration::from_secs(5)).unwrap()
        {
            return work;
        }
    }
}

async fn wait_for_outcome(
    services: &CoreServices,
    presentation_id: PresentationId,
    terminal_id: &TerminalId,
    predicate: impl Fn(&PresentationDeliveryOutcome) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if services
                .presentations
                .delivery_result(presentation_id)
                .and_then(|result| result.outcomes.get(terminal_id).cloned())
                .is_some_and(|outcome| predicate(&outcome))
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_runtime_routes_and_completes_real_presentations() {
    let core = RunningCore::start().await;
    core.register_addon().await;
    let terminal_events_url = format!(
        "ws://{}{}",
        core.http_url.trim_start_matches("http://"),
        CORE_TERMINAL_EVENTS_WEBSOCKET_PATH
    );
    let (mut terminal_events, _) = connect_async(terminal_events_url).await.unwrap();
    let alpha_id = TerminalId::new("alpha").unwrap();
    let bravo_id = TerminalId::new("bravo").unwrap();
    let alpha =
        TerminalRuntime::spawn(terminal_configuration(&core.terminal_url, "alpha")).unwrap();
    let bravo =
        TerminalRuntime::spawn(terminal_configuration(&core.terminal_url, "bravo")).unwrap();
    let alpha_connection = next_registered(&alpha);
    next_registered(&bravo);

    let targets = reqwest::Client::new()
        .get(format!("{}{}", core.http_url, CORE_TELEPHONY_TARGETS_PATH))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<TelephonyTargets>()
        .await
        .unwrap();
    assert_eq!(
        targets
            .terminals
            .iter()
            .map(|target| target.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha terminal", "bravo terminal"]
    );
    assert!(targets.all.is_some());

    let presence = core.services.terminals.presence(&alpha_id).unwrap();
    assert_eq!(
        presence
            .implementation_version
            .as_ref()
            .unwrap()
            .as_version(),
        &Version::new(1, 2, 3)
    );
    assert_eq!(
        presence.runtime_diagnostics.iter().collect::<Vec<_>>(),
        vec![("display.resolution", "1280x720"), ("platform", "test")]
    );
    let persisted = std::fs::read_to_string(&core.state_path).unwrap();
    assert!(!persisted.contains("1280x720"));
    assert!(!persisted.contains("1.2.3"));

    let duplicate =
        TerminalRuntime::spawn(terminal_configuration(&core.terminal_url, "alpha")).unwrap();
    loop {
        if let TerminalEvent::RegistrationRejected(rejection) = duplicate
            .next_event_timeout(Duration::from_secs(5))
            .unwrap()
        {
            assert_eq!(
                rejection.reason,
                RegistrationRejectionReason::DuplicateTerminalId
            );
            break;
        }
    }
    duplicate.shutdown(None).unwrap();

    let accepted_id = PresentationId::new();
    core.request(accepted_id, &alpha_id).await;
    let accepted = next_work(&alpha);
    assert_eq!(accepted.presentation().request.id, accepted_id);
    alpha
        .accept_presentation(accepted.completion().clone())
        .unwrap();
    wait_for_outcome(&core.services, accepted_id, &alpha_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Accepted)
    })
    .await;
    let completion = terminal_events
        .next()
        .await
        .unwrap()
        .unwrap()
        .into_text()
        .unwrap();
    let completion: ProtocolTerminalEvent = serde_json::from_str(&completion).unwrap();
    assert!(matches!(
        completion.kind,
        TerminalEventKind::PresentationDeliveryCompleted { result }
            if result.presentation_id == accepted_id
    ));

    let bravo_only_id = PresentationId::new();
    core.request(bravo_only_id, &bravo_id).await;
    let bravo_work = next_work(&bravo);
    assert_eq!(bravo_work.presentation().request.id, bravo_only_id);
    bravo
        .accept_presentation(bravo_work.completion().clone())
        .unwrap();

    let rejected_id = PresentationId::new();
    core.request(rejected_id, &alpha_id).await;
    let rejected = next_work(&alpha);
    alpha
        .reject_presentation(
            rejected.completion().clone(),
            PresentationRejection {
                code: PresentationRejectionCode::new(PresentationRejectionCode::BUSY).unwrap(),
                detail: Some("test rejection".to_owned()),
            },
        )
        .unwrap();
    wait_for_outcome(&core.services, rejected_id, &alpha_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Rejected { .. })
    })
    .await;

    let timed_out_id = PresentationId::new();
    core.request(timed_out_id, &alpha_id).await;
    let _timed_out = next_work(&alpha);
    core.services
        .presentations
        .expire_acknowledgements(std::time::Instant::now() + Duration::from_secs(31));
    wait_for_outcome(&core.services, timed_out_id, &alpha_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::TimedOut)
    })
    .await;

    let disconnected_id = PresentationId::new();
    core.request(disconnected_id, &alpha_id).await;
    let _pending = next_work(&alpha);
    alpha
        .shutdown(Some("integration disconnect".to_owned()))
        .unwrap();
    wait_for_outcome(&core.services, disconnected_id, &alpha_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Disconnected)
    })
    .await;

    let reconnected =
        TerminalRuntime::spawn(terminal_configuration(&core.terminal_url, "alpha")).unwrap();
    let new_connection = next_registered(&reconnected);
    assert_ne!(alpha_connection, new_connection);
    let post_reconnect_id = PresentationId::new();
    core.request(post_reconnect_id, &alpha_id).await;
    let first_after_reconnect = next_work(&reconnected);
    assert_eq!(
        first_after_reconnect.presentation().request.id,
        post_reconnect_id
    );
    assert!(first_after_reconnect.delivery().generation.get() > 1);
    reconnected
        .accept_presentation(first_after_reconnect.completion().clone())
        .unwrap();

    reconnected.shutdown(None).unwrap();
    bravo.shutdown(None).unwrap();
    terminal_events.close(None).await.unwrap();
    core.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_endpoint_rejects_incompatible_protocol_and_cleans_up_socket_loss() {
    let core = RunningCore::start().await;
    let incompatible_id = TerminalId::new("incompatible").unwrap();
    let (mut socket, _) = connect_async(&core.terminal_url).await.unwrap();
    socket
        .send(Message::Text(
            serde_json::to_string(&TerminalClientMessage::Register {
                registration: TerminalRegistration {
                    identity: TerminalIdentity {
                        id: incompatible_id.clone(),
                        name: TerminalName::new("Incompatible").unwrap(),
                    },
                    implementation: TerminalImplementationId::new("raw-terminal").unwrap(),
                    protocol_version: ProtocolVersion::new(0, 2),
                    capabilities: TerminalCapabilities::default(),
                },
                implementation_version: None,
                runtime_diagnostics: TerminalRuntimeDiagnostics::default(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let response = socket.next().await.unwrap().unwrap().into_text().unwrap();
    let bts_protocol::CoreTerminalMessage::RegistrationRejected { rejection } =
        serde_json::from_str(&response).unwrap()
    else {
        panic!("expected registration rejection")
    };
    assert!(matches!(
        rejection.reason,
        RegistrationRejectionReason::UnsupportedProtocolVersion { .. }
    ));

    let raw_id = TerminalId::new("socket-loss").unwrap();
    let (mut raw, _) = connect_async(&core.terminal_url).await.unwrap();
    raw.send(Message::Text(
        serde_json::to_string(&TerminalClientMessage::Register {
            registration: TerminalRegistration {
                identity: TerminalIdentity {
                    id: raw_id.clone(),
                    name: TerminalName::new("Socket loss").unwrap(),
                },
                implementation: TerminalImplementationId::new("raw-terminal").unwrap(),
                protocol_version: ProtocolVersion::CURRENT,
                capabilities: TerminalCapabilities::default(),
            },
            implementation_version: None,
            runtime_diagnostics: TerminalRuntimeDiagnostics::default(),
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let response = raw.next().await.unwrap().unwrap().into_text().unwrap();
    let bts_protocol::CoreTerminalMessage::RegistrationAcknowledged { connection_id, .. } =
        serde_json::from_str(&response).unwrap()
    else {
        panic!("expected registration acknowledgement")
    };
    assert!(core.services.terminals.presence(&raw_id).is_some());
    raw.send(Message::Text(
        serde_json::to_string(&TerminalClientMessage::Heartbeat {
            terminal_id: raw_id.clone(),
            connection_id,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let heartbeat = raw.next().await.unwrap().unwrap().into_text().unwrap();
    assert_eq!(
        serde_json::from_str::<bts_protocol::CoreTerminalMessage>(&heartbeat).unwrap(),
        bts_protocol::CoreTerminalMessage::HeartbeatAcknowledged { connection_id }
    );
    drop(raw);
    tokio::time::timeout(Duration::from_secs(5), async {
        while core.services.terminals.presence(&raw_id).is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    core.stop().await;
}
