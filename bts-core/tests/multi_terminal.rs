use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use bts_addons::HttpAddonContext;
use bts_core::{
    presentations::TerminalPresentationState,
    server::{CoreConfiguration, CoreServer, CoreServices},
};
use bts_protocol::{
    DisplayState, DtmfMenuKey, Event, EventKind, GroupId, GroupIdentity, GroupName, NewEvent,
    PresentationDeliveryOutcome, PresentationId, PresentationRequest, ScreenKind, TargetScope,
    TelephonyTargets, TerminalCapabilities, TerminalCapability, TerminalDescription, TerminalId,
    TerminalImplementationId, TerminalName, TerminalTarget,
    addons::v1::{
        API_VERSION, ActionId, ActionRegistration, ActionRequest, Addon, AddonCapability,
        AddonContext, AddonId, AddonManifest, AddonVersion, MenuEntry,
    },
    core::{CORE_EVENTS_PATH, CORE_TELEPHONY_TARGETS_PATH, CORE_TERMINALS_WEBSOCKET_PATH},
};
use bts_telephony::session::{CallerIdentity, MenuContext, TelephonySession};
use bts_terminal::{ConnectionState, RuntimeDiagnostics, TerminalConfiguration};
use bts_terminal_simulator::{
    HeadlessTerminal, ResponsePolicy, SimulatorConfiguration, SimulatorEvent,
};
use semver::Version;
use tempfile::TempDir;
use tokio::sync::oneshot;

const EVENT_TIMEOUT: Duration = Duration::from_secs(5);
const ADDON_ID: &str = "integration-addon";
const ACTION_ID: &str = "integration.show";

struct RunningCore {
    state_path: PathBuf,
    address: SocketAddr,
    http_url: String,
    terminal_url: String,
    services: CoreServices,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl RunningCore {
    async fn start(state_path: &Path, address: Option<SocketAddr>) -> Self {
        let configuration = CoreConfiguration {
            terminal_state_path: state_path.to_path_buf(),
            presence_timeout: Duration::from_secs(60),
            acknowledgement_timeout: Duration::from_secs(30),
            presence_expiry_interval: Duration::from_secs(3600),
            acknowledgement_expiry_interval: Duration::from_secs(3600),
        };
        let server = CoreServer::new(configuration).unwrap();
        Self::serve(state_path, server, address).await
    }

    async fn serve(state_path: &Path, server: CoreServer, address: Option<SocketAddr>) -> Self {
        let services = server.services();
        let listener = tokio::net::TcpListener::bind(
            address
                .map(|value| value.to_string())
                .unwrap_or_else(|| "127.0.0.1:0".to_owned()),
        )
        .await
        .unwrap();
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let task = tokio::spawn(server.serve(listener, Some(ready_sender), async move {
            let _ = shutdown_receiver.await;
        }));
        let address = ready_receiver.await.unwrap();
        Self {
            state_path: state_path.to_path_buf(),
            address,
            http_url: format!("http://{address}"),
            terminal_url: format!("ws://{address}{CORE_TERMINALS_WEBSOCKET_PATH}"),
            services,
            shutdown: Some(shutdown_sender),
            task,
        }
    }

    async fn register_addon(&self) {
        self.post(EventKind::AddonRegistered {
            manifest: addon_manifest(),
        })
        .await;
    }

    async fn post(&self, kind: EventKind) {
        reqwest::Client::new()
            .post(format!("{}{}", self.http_url, CORE_EVENTS_PATH))
            .json(&NewEvent {
                source: ADDON_ID.to_owned(),
                kind,
            })
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
    }

    async fn dispatch(
        &self,
        target: TerminalTarget,
        required_capabilities: TerminalCapabilities,
        display: DisplayState,
    ) -> PresentationId {
        let id = PresentationId::new();
        self.post(EventKind::PresentationRequested {
            request: PresentationRequest {
                id,
                target,
                required_capabilities,
                display,
            },
        })
        .await;
        id
    }

    async fn targets(&self) -> TelephonyTargets {
        reqwest::Client::new()
            .get(format!("{}{}", self.http_url, CORE_TELEPHONY_TARGETS_PATH))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn stop(mut self) {
        let _ = self.shutdown.take().unwrap().send(());
        self.task.await.unwrap().unwrap();
    }
}

fn addon_manifest() -> AddonManifest {
    AddonManifest {
        api_version: API_VERSION,
        id: AddonId::new(ADDON_ID),
        name: "Integration addon".to_owned(),
        version: AddonVersion::new(1, 0, 0),
        actions: vec![ActionRegistration {
            id: ActionId::new(ACTION_ID),
            description: "Show an integration presentation".to_owned(),
        }],
        menu: vec![MenuEntry {
            digit: DtmfMenuKey::new('2').unwrap(),
            prompt: "sound:bts/integration".to_owned(),
            action: ActionId::new(ACTION_ID),
            order: 20,
        }],
        capabilities: vec![AddonCapability::Display],
        screens: vec![ScreenKind::Message, ScreenKind::Weather, ScreenKind::Clock],
    }
}

fn capabilities(values: &[&str]) -> TerminalCapabilities {
    TerminalCapabilities::new(
        values
            .iter()
            .map(|value| TerminalCapability::new(*value).unwrap()),
    )
}

fn simulator_configuration(
    core_url: &str,
    id: &str,
    name: &str,
    supported: &[&str],
) -> SimulatorConfiguration {
    let terminal = TerminalConfiguration::new(
        core_url,
        TerminalId::new(id).unwrap(),
        TerminalName::new(name).unwrap(),
        TerminalImplementationId::new("bts-terminal-simulator").unwrap(),
        Version::new(0, 3, 0),
        capabilities(supported),
    )
    .unwrap()
    .with_runtime_diagnostics(
        RuntimeDiagnostics::new([
            ("platform".to_owned(), "integration-test".to_owned()),
            ("runtime".to_owned(), "headless-simulator".to_owned()),
        ])
        .unwrap(),
    );
    SimulatorConfiguration::new(terminal, ResponsePolicy::Accept)
}

fn next_registered(terminal: &mut HeadlessTerminal) -> bts_protocol::TerminalConnectionId {
    loop {
        if let SimulatorEvent::ConnectionStateChanged(ConnectionState::Registered {
            connection_id,
            ..
        }) = terminal.next_event_timeout(EVENT_TIMEOUT).unwrap()
        {
            return connection_id;
        }
    }
}

fn next_accepted(terminal: &mut HeadlessTerminal, expected: PresentationId) -> DisplayState {
    loop {
        if let SimulatorEvent::PresentationAccepted {
            presentation_id,
            display,
        } = terminal.next_event_timeout(EVENT_TIMEOUT).unwrap()
        {
            assert_eq!(presentation_id, expected);
            return display;
        }
    }
}

async fn wait_for_presence(services: &CoreServices, terminal_id: &TerminalId, online: bool) {
    tokio::time::timeout(EVENT_TIMEOUT, async {
        loop {
            if services.terminals.presence(terminal_id).is_some() == online {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_outcome(
    services: &CoreServices,
    presentation_id: PresentationId,
    terminal_id: &TerminalId,
    predicate: impl Fn(&PresentationDeliveryOutcome) -> bool,
) {
    tokio::time::timeout(EVENT_TIMEOUT, async {
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

fn direct_target(id: &TerminalId) -> TerminalTarget {
    TerminalTarget::Terminal {
        id: id.clone(),
        scope: TargetScope::Online,
    }
}

fn weather() -> DisplayState {
    DisplayState::Weather {
        location: "Home".to_owned(),
        temperature: "21 C".to_owned(),
        condition: "Clear".to_owned(),
        details: vec!["Light wind".to_owned()],
        updated_at: "now".to_owned(),
    }
}

fn clock() -> DisplayState {
    DisplayState::Clock {
        time: "12:34".to_owned(),
        seconds: "56".to_owned(),
        date: "Thursday".to_owned(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_terminals_route_independently_reconnect_and_survive_core_restart() {
    let directory = TempDir::new().unwrap();
    let state_path = directory.path().join("core/terminals.json");
    let core = RunningCore::start(&state_path, None).await;
    core.register_addon().await;

    let bedroom_id = TerminalId::new("bedroom-display").unwrap();
    let dining_id = TerminalId::new("dining-display").unwrap();
    let mut bedroom = HeadlessTerminal::spawn(simulator_configuration(
        &core.terminal_url,
        bedroom_id.as_str(),
        "Bedroom",
        &[
            TerminalCapability::RENDER_TEXT,
            TerminalCapability::RENDER_IMAGES,
        ],
    ))
    .unwrap();
    let dining_configuration = simulator_configuration(
        &core.terminal_url,
        dining_id.as_str(),
        "Dining Room",
        &[TerminalCapability::RENDER_TEXT],
    );
    let mut dining = HeadlessTerminal::spawn(dining_configuration.clone()).unwrap();
    let bedroom_connection = next_registered(&mut bedroom);
    let dining_connection = next_registered(&mut dining);

    assert_eq!(core.services.terminals.definitions().len(), 2);
    assert!(core.services.terminals.presence(&bedroom_id).is_some());
    assert!(core.services.terminals.presence(&dining_id).is_some());
    assert_eq!(
        core.targets()
            .await
            .terminals
            .iter()
            .map(|option| option.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Bedroom", "Dining Room"]
    );

    let bedroom_weather = core
        .dispatch(
            direct_target(&bedroom_id),
            capabilities(&[TerminalCapability::RENDER_TEXT]),
            weather(),
        )
        .await;
    assert_eq!(next_accepted(&mut bedroom, bedroom_weather), weather());
    wait_for_outcome(&core.services, bedroom_weather, &bedroom_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Accepted)
    })
    .await;
    let dining_clock = core
        .dispatch(
            direct_target(&dining_id),
            capabilities(&[TerminalCapability::RENDER_TEXT]),
            clock(),
        )
        .await;
    assert_eq!(next_accepted(&mut dining, dining_clock), clock());
    wait_for_outcome(&core.services, dining_clock, &dining_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Accepted)
    })
    .await;
    assert_eq!(
        core.services
            .presentations
            .terminal_state(&bedroom_id)
            .unwrap()
            .display,
        weather()
    );
    assert_eq!(
        core.services
            .presentations
            .terminal_state(&dining_id)
            .unwrap()
            .display,
        clock()
    );

    let group_id = GroupId::new("all-displays").unwrap();
    core.services
        .terminals
        .create_group(GroupIdentity {
            id: group_id.clone(),
            name: GroupName::new("All displays").unwrap(),
        })
        .unwrap();
    core.services
        .terminals
        .add_group_member(&group_id, &bedroom_id)
        .unwrap();
    core.services
        .terminals
        .add_group_member(&group_id, &dining_id)
        .unwrap();
    let group_display = DisplayState::Message {
        title: "Group".to_owned(),
        body: "Both terminals".to_owned(),
    };
    let group_presentation = core
        .dispatch(
            TerminalTarget::Group {
                id: group_id.clone(),
                scope: TargetScope::Online,
            },
            capabilities(&[TerminalCapability::RENDER_TEXT]),
            group_display.clone(),
        )
        .await;
    assert_eq!(
        next_accepted(&mut bedroom, group_presentation),
        group_display
    );
    assert_eq!(
        next_accepted(&mut dining, group_presentation),
        group_display
    );

    let images_only = core
        .dispatch(
            TerminalTarget::all(),
            capabilities(&[TerminalCapability::RENDER_IMAGES]),
            DisplayState::Message {
                title: "Images".to_owned(),
                body: "Capability filtering".to_owned(),
            },
        )
        .await;
    next_accepted(&mut bedroom, images_only);
    wait_for_outcome(&core.services, images_only, &dining_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Incompatible { .. })
    })
    .await;

    dining
        .shutdown(Some("integration offline check".to_owned()))
        .unwrap();
    wait_for_presence(&core.services, &dining_id, false).await;
    let offline = core
        .dispatch(
            TerminalTarget::all(),
            capabilities(&[TerminalCapability::RENDER_TEXT]),
            DisplayState::Message {
                title: "Offline".to_owned(),
                body: "Dining Room is disconnected".to_owned(),
            },
        )
        .await;
    next_accepted(&mut bedroom, offline);
    wait_for_outcome(&core.services, offline, &dining_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Offline)
    })
    .await;

    let mut dining = HeadlessTerminal::spawn(dining_configuration).unwrap();
    let reconnected_dining = next_registered(&mut dining);
    assert_ne!(dining_connection, reconnected_dining);
    assert_eq!(core.services.terminals.definitions().len(), 2);

    core.services
        .terminals
        .set_terminal_description(
            &bedroom_id,
            Some(TerminalDescription::new("Private upstairs display").unwrap()),
        )
        .unwrap();
    core.services
        .terminals
        .add_terminal_tag(&bedroom_id, "bedroom")
        .unwrap();
    core.services
        .terminals
        .add_terminal_tag(&dining_id, "dining-room")
        .unwrap();

    let address = core.address;
    core.stop().await;

    let configuration = CoreConfiguration {
        terminal_state_path: state_path.clone(),
        presence_timeout: Duration::from_secs(60),
        acknowledgement_timeout: Duration::from_secs(30),
        presence_expiry_interval: Duration::from_secs(3600),
        acknowledgement_expiry_interval: Duration::from_secs(3600),
    };
    let restarted_server = CoreServer::new(configuration).unwrap();
    let restored_services = restarted_server.services();
    assert_eq!(restored_services.terminals.definitions().len(), 2);
    assert!(restored_services.terminals.presence(&bedroom_id).is_none());
    assert!(restored_services.terminals.presence(&dining_id).is_none());
    assert_eq!(
        restored_services
            .terminals
            .definition(&bedroom_id)
            .unwrap()
            .description
            .as_ref()
            .map(TerminalDescription::as_str),
        Some("Private upstairs display")
    );
    assert!(
        restored_services
            .terminals
            .group(&group_id)
            .unwrap()
            .members
            .contains(&dining_id)
    );
    assert!(restored_services.presentations.terminal_states().is_empty());

    let restarted = RunningCore::serve(&state_path, restarted_server, Some(address)).await;
    let new_bedroom_connection = next_registered(&mut bedroom);
    let new_dining_connection = next_registered(&mut dining);
    assert_ne!(bedroom_connection, new_bedroom_connection);
    assert_ne!(reconnected_dining, new_dining_connection);
    assert_eq!(restarted.services.terminals.definitions().len(), 2);
    assert_eq!(restarted.state_path, state_path);

    bedroom.shutdown(None).unwrap();
    dining.shutdown(None).unwrap();
    restarted.stop().await;
}

struct PresentationAddon {
    display: DisplayState,
}

#[async_trait]
impl Addon for PresentationAddon {
    fn manifest(&self) -> AddonManifest {
        addon_manifest()
    }

    async fn handle_event(&self, context: &dyn AddonContext, event: &Event) -> anyhow::Result<()> {
        if matches!(event.kind, EventKind::ActionRequested { .. }) {
            context.show(self.display.clone(), 10).await?;
        }
        Ok(())
    }
}

async fn invoke_addon(
    core: &RunningCore,
    data_root: &Path,
    request: ActionRequest,
    display: DisplayState,
) {
    core.post(EventKind::ActionRequested {
        request: request.clone(),
    })
    .await;
    let event = Event::new(
        "bts-telephony",
        EventKind::ActionRequested {
            request: request.clone(),
        },
    );
    let context = HttpAddonContext::new(&core.http_url, AddonId::new(ADDON_ID), data_root)
        .with_selected_target(request.target);
    PresentationAddon { display }
        .handle_event(&context, &event)
        .await
        .unwrap();
}

fn state_display(state: Option<TerminalPresentationState>) -> Option<DisplayState> {
    state.map(|value| value.display)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telephony_changes_target_inside_addon_without_changing_the_previous_screen() {
    let directory = TempDir::new().unwrap();
    let core = RunningCore::start(&directory.path().join("terminals.json"), None).await;
    let addon_data_root = directory.path().join("addons");
    core.register_addon().await;
    let bedroom_id = TerminalId::new("bedroom-display").unwrap();
    let dining_id = TerminalId::new("dining-display").unwrap();
    let mut bedroom = HeadlessTerminal::spawn(simulator_configuration(
        &core.terminal_url,
        bedroom_id.as_str(),
        "Bedroom",
        &[TerminalCapability::RENDER_TEXT],
    ))
    .unwrap();
    let mut dining = HeadlessTerminal::spawn(simulator_configuration(
        &core.terminal_url,
        dining_id.as_str(),
        "Dining Room",
        &[TerminalCapability::RENDER_TEXT],
    ))
    .unwrap();
    next_registered(&mut bedroom);
    next_registered(&mut dining);

    let dining_clock = core
        .dispatch(
            direct_target(&dining_id),
            capabilities(&[TerminalCapability::RENDER_TEXT]),
            clock(),
        )
        .await;
    next_accepted(&mut dining, dining_clock);
    wait_for_outcome(&core.services, dining_clock, &dining_id, |outcome| {
        matches!(outcome, PresentationDeliveryOutcome::Accepted)
    })
    .await;

    let targets = core.targets().await;
    let actions = HashMap::from([("2".to_owned(), ActionId::new(ACTION_ID))]);
    let (mut session, _) = TelephonySession::new(
        CallerIdentity {
            number: Some("201".to_owned()),
            name: Some("Integration caller".to_owned()),
        },
        &targets,
        "sound:bts/main".to_owned(),
    );
    assert!(
        session
            .handle_dtmf("1", &targets, &actions)
            .action
            .is_none()
    );
    assert!(
        session
            .handle_dtmf("#", &targets, &actions)
            .action
            .is_none()
    );
    assert_eq!(session.selected_target, Some(direct_target(&bedroom_id)));

    let bedroom_action = session.handle_dtmf("2", &targets, &actions).action.unwrap();
    invoke_addon(&core, &addon_data_root, bedroom_action, weather()).await;
    let bedroom_presentation = loop {
        if let SimulatorEvent::PresentationAccepted {
            presentation_id,
            display,
        } = bedroom.next_event_timeout(EVENT_TIMEOUT).unwrap()
        {
            assert_eq!(display, weather());
            break presentation_id;
        }
    };
    wait_for_outcome(
        &core.services,
        bedroom_presentation,
        &bedroom_id,
        |outcome| matches!(outcome, PresentationDeliveryOutcome::Accepted),
    )
    .await;

    let before_change = core.services.presentations.terminal_states();
    assert!(
        session
            .handle_dtmf("0", &targets, &actions)
            .action
            .is_none()
    );
    assert!(
        session
            .handle_dtmf("1", &targets, &actions)
            .action
            .is_none()
    );
    assert!(
        session
            .handle_dtmf("2", &targets, &actions)
            .action
            .is_none()
    );
    assert!(
        session
            .handle_dtmf("#", &targets, &actions)
            .action
            .is_none()
    );
    assert_eq!(session.selected_target, Some(direct_target(&dining_id)));
    assert!(matches!(session.current_context, MenuContext::Addon { .. }));
    assert_eq!(core.services.presentations.terminal_states(), before_change);
    assert_eq!(bedroom.current_presentation(), Some(&weather()));
    assert_eq!(dining.current_presentation(), Some(&clock()));

    let dining_action = session.handle_dtmf("2", &targets, &actions).action.unwrap();
    let dining_message = DisplayState::Message {
        title: "Dining Room".to_owned(),
        body: "Selected after configuration".to_owned(),
    };
    invoke_addon(
        &core,
        &addon_data_root,
        dining_action,
        dining_message.clone(),
    )
    .await;
    loop {
        if let SimulatorEvent::PresentationAccepted { display, .. } =
            dining.next_event_timeout(EVENT_TIMEOUT).unwrap()
        {
            assert_eq!(display, dining_message);
            break;
        }
    }
    assert_eq!(
        state_display(core.services.presentations.terminal_state(&bedroom_id)),
        Some(weather())
    );

    session.handle_dtmf("0", &targets, &actions);
    session.handle_dtmf("1", &targets, &actions);
    session.handle_dtmf("1", &targets, &actions);
    let cancelled = session.handle_dtmf("*", &targets, &actions);
    assert!(cancelled.action.is_none());
    assert_eq!(session.selected_target, Some(direct_target(&dining_id)));
    assert!(matches!(session.current_context, MenuContext::Addon { .. }));

    let reserved = HashMap::from([
        ("0".to_owned(), ActionId::new("bad.zero")),
        ("*".to_owned(), ActionId::new("bad.star")),
        ("#".to_owned(), ActionId::new("bad.hash")),
    ]);
    for digit in ["0", "*", "#"] {
        assert!(
            session
                .handle_dtmf(digit, &targets, &reserved)
                .action
                .is_none()
        );
    }

    let valid = NewEvent {
        source: "reserved-test".to_owned(),
        kind: EventKind::AddonRegistered {
            manifest: addon_manifest(),
        },
    };
    for digit in ["0", "*", "#"] {
        let mut wire = serde_json::to_value(&valid).unwrap();
        wire["manifest"]["menu"][0]["digit"] = serde_json::json!(digit);
        let response = reqwest::Client::new()
            .post(format!("{}{}", core.http_url, CORE_EVENTS_PATH))
            .json(&wire)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    }

    bedroom.shutdown(None).unwrap();
    dining.shutdown(None).unwrap();
    core.stop().await;
}
