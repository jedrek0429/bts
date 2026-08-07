use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use bts_cli::{
    cli::{
        AddonCommand, Cli, Command, GroupCommand, StateCommand, TerminalCommand, TerminalTagCommand,
    },
    config::{ColourMode, Environment, OutputMode},
    output::OutputStreams,
};
use bts_core::server::{CoreConfiguration, CoreServer};
use bts_protocol::{
    AdministrativeErrorCategory, AdministrativeErrorCode, AdministrativeErrorResponse,
    CoreOperationalStatus, EventKind, GroupId, GroupName, NewEvent, ProtocolVersion,
    TerminalCapabilities, TerminalCapability, TerminalConnectionId, TerminalId, TerminalIdentity,
    TerminalImplementationId, TerminalName, TerminalRegistration, TerminalTag,
    addons::v1::{
        API_VERSION, ActionId, ActionRegistration, AddonCapability, AddonId, AddonManifest,
        AddonVersion,
    },
    core::CORE_API_VERSION,
};
use bts_sdk::{
    AddonReference, CoreApi, CoreApiConfiguration, CreateGroupRequest, GroupReference,
    RenameTerminalRequest, SdkError, SetAddonEnabledRequest, TerminalReference,
    UpdateGroupMembersRequest, UpdateTerminalTagsRequest,
};
use tokio::sync::oneshot;

async fn run_json_cli(base_url: &str, command: Command, yes: bool) -> (u8, serde_json::Value) {
    let cli = Cli {
        core: Some(base_url.to_owned()),
        output: Some(OutputMode::Json),
        timeout: Some("2s".to_owned()),
        quiet: false,
        verbosity: 0,
        colour: Some(ColourMode::Never),
        yes,
        command,
    };
    let mut stdin = std::io::Cursor::new(Vec::<u8>::new());
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = bts_cli::execute(
        cli,
        &Environment::default(),
        OutputStreams {
            stdin: &mut stdin,
            stdout: &mut stdout,
            stderr: &mut stderr,
            stdin_is_terminal: false,
            stdout_is_terminal: false,
            stderr_is_terminal: false,
        },
    )
    .await;
    let bytes = if code == 0 { &stdout } else { &stderr };
    (code, serde_json::from_slice(bytes).unwrap())
}

fn addon_manifest(id: &str, name: &str, action: &str) -> AddonManifest {
    AddonManifest {
        api_version: API_VERSION,
        id: AddonId::new(id),
        name: name.to_owned(),
        version: AddonVersion::new(1, 2, 3),
        actions: vec![ActionRegistration {
            id: ActionId::new(action),
            description: "Run the fixture".to_owned(),
        }],
        menu: Vec::new(),
        capabilities: vec![AddonCapability::Display],
        screens: Vec::new(),
    }
}

async fn publish_event(base_url: &str, kind: EventKind) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base_url}api/v1/events"))
        .json(&NewEvent {
            source: "integration-test".to_owned(),
            kind,
        })
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn sdk_and_cli_observe_real_core_without_creating_terminal_presence() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = CoreConfiguration {
        terminal_state_path: directory.path().join("terminals.json"),
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
    let base_url = format!("http://{address}/");

    let before = services.terminals.routing_snapshot(Instant::now());
    assert!(before.definitions.is_empty());
    assert!(before.presences.is_empty());

    let api = CoreApi::new(CoreApiConfiguration::new(&base_url).unwrap()).unwrap();
    let discovery = api.discover().await.unwrap();
    assert_eq!(discovery.product, "bts-core");
    assert_eq!(discovery.administrative_api.current, CORE_API_VERSION);
    assert!(
        discovery
            .administrative_api
            .supported
            .contains(&CORE_API_VERSION)
    );

    let status = api.status().await.unwrap();
    assert_eq!(status.status, CoreOperationalStatus::Ready);
    assert_eq!(status.administrative_api_version, CORE_API_VERSION);

    let state = api.state().await.unwrap();
    assert_eq!(state.terminals.registered, 0);
    assert_eq!(state.terminals.online, 0);
    assert_eq!(state.terminals.groups, 0);

    for command in [
        Command::Status,
        Command::State {
            command: StateCommand::Show,
        },
    ] {
        let cli = Cli {
            core: Some(base_url.clone()),
            output: Some(OutputMode::Json),
            timeout: Some("2s".to_owned()),
            quiet: false,
            verbosity: 0,
            colour: Some(ColourMode::Always),
            yes: false,
            command,
        };
        let mut stdin = std::io::Cursor::new(Vec::<u8>::new());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = bts_cli::execute(
            cli,
            &Environment::default(),
            OutputStreams {
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
                stdin_is_terminal: false,
                stdout_is_terminal: false,
                stderr_is_terminal: false,
            },
        )
        .await;
        assert_eq!(code, 0);
        assert!(stderr.is_empty());
        assert!(serde_json::from_slice::<serde_json::Value>(&stdout).is_ok());
        assert!(!stdout.windows(2).any(|window| window == b"\x1b["));
    }

    let after = services.terminals.routing_snapshot(Instant::now());
    assert!(after.definitions.is_empty());
    assert!(after.presences.is_empty());
    assert!(after.groups.is_empty());

    let response = reqwest::get(format!("{base_url}api/v1/admin/not-a-resource"))
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let error = response
        .json::<AdministrativeErrorResponse>()
        .await
        .unwrap();
    assert_eq!(error.error.category, AdministrativeErrorCategory::NotFound);

    shutdown_sender.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn terminal_and_group_administration_is_authoritative_and_safe() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = CoreConfiguration {
        terminal_state_path: directory.path().join("terminals.json"),
        presence_timeout: Duration::from_secs(60),
        acknowledgement_timeout: Duration::from_secs(30),
        presence_expiry_interval: Duration::from_secs(3600),
        acknowledgement_expiry_interval: Duration::from_secs(3600),
    };
    let server = CoreServer::new(configuration).unwrap();
    let services = server.services();
    let alpha_id = TerminalId::new("alpha-display").unwrap();
    let beta_id = TerminalId::new("beta-display").unwrap();
    let alpha_connection = TerminalConnectionId::new();
    let beta_connection = TerminalConnectionId::new();
    for (id, name, connection) in [
        (alpha_id.clone(), "Alpha", alpha_connection),
        (beta_id.clone(), "Beta", beta_connection),
    ] {
        services
            .terminals
            .register(
                TerminalRegistration {
                    identity: TerminalIdentity {
                        id,
                        name: TerminalName::new(name).unwrap(),
                    },
                    implementation: TerminalImplementationId::new("bts-display").unwrap(),
                    protocol_version: ProtocolVersion::CURRENT,
                    capabilities: TerminalCapabilities::new([TerminalCapability::new(
                        TerminalCapability::RENDER_TEXT,
                    )
                    .unwrap()]),
                },
                connection,
                None,
                Instant::now(),
            )
            .unwrap();
    }
    services
        .terminals
        .disconnect(&beta_id, beta_connection)
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (ready_sender, ready_receiver) = oneshot::channel();
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let task = tokio::spawn(server.serve(listener, Some(ready_sender), async move {
        let _ = shutdown_receiver.await;
    }));
    let address = ready_receiver.await.unwrap();
    let api =
        CoreApi::new(CoreApiConfiguration::new(format!("http://{address}/")).unwrap()).unwrap();

    let terminals = api.terminals().await.unwrap().terminals;
    assert_eq!(terminals.len(), 2);
    assert!(terminals[0].presence.is_some());
    assert!(terminals[1].presence.is_none());

    let beta = TerminalReference::new(beta_id.to_string()).unwrap();
    let renamed = api
        .rename_terminal(
            &beta,
            &RenameTerminalRequest {
                name: TerminalName::new("Bedroom").unwrap(),
            },
        )
        .await
        .unwrap();
    assert!(renamed.changed);
    assert!(
        !api.rename_terminal(
            &beta,
            &RenameTerminalRequest {
                name: TerminalName::new("Bedroom").unwrap(),
            },
        )
        .await
        .unwrap()
        .changed
    );
    let (code, renamed) = run_json_cli(
        &format!("http://{address}/"),
        Command::Terminal {
            command: TerminalCommand::Rename {
                terminal: beta.clone(),
                name: TerminalName::new("Bedroom").unwrap(),
            },
        },
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(renamed["changed"], false);
    api.rename_terminal(
        &TerminalReference::new(alpha_id.to_string()).unwrap(),
        &RenameTerminalRequest {
            name: TerminalName::new("Bedroom").unwrap(),
        },
    )
    .await
    .unwrap();
    let ambiguous = api
        .terminal(&TerminalReference::new("Bedroom").unwrap())
        .await
        .unwrap_err();
    assert!(matches!(
        &ambiguous,
        SdkError::AmbiguousReference(value)
            if value.code.as_str() == AdministrativeErrorCode::AMBIGUOUS_TERMINAL_REFERENCE
                && value.candidates.len() == 2
    ));
    let (code, error) = run_json_cli(
        &format!("http://{address}/"),
        Command::Terminal {
            command: TerminalCommand::Show {
                terminal: TerminalReference::new("Bedroom").unwrap(),
            },
        },
        false,
    )
    .await;
    assert_eq!(code, 6);
    assert_eq!(error["error"]["code"], "ambiguous_terminal_reference");
    assert!(matches!(
        api.terminal(&TerminalReference::new("Missing terminal").unwrap())
            .await
            .unwrap_err(),
        SdkError::NotFound(_)
    ));
    let tagged = api
        .update_terminal_tags(
            &beta,
            &UpdateTerminalTagsRequest {
                add: BTreeSet::from([TerminalTag::new("private").unwrap()]),
                remove: BTreeSet::new(),
            },
        )
        .await
        .unwrap();
    assert!(
        tagged
            .resource
            .tags
            .contains(&TerminalTag::new("private").unwrap())
    );
    let (code, tagged) = run_json_cli(
        &format!("http://{address}/"),
        Command::Terminal {
            command: TerminalCommand::Tag {
                command: TerminalTagCommand::Add {
                    terminal: beta.clone(),
                    tags: vec![TerminalTag::new("private").unwrap()],
                },
            },
        },
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(tagged["changed"], false);
    let invalid_tags = api
        .update_terminal_tags(
            &beta,
            &UpdateTerminalTagsRequest {
                add: BTreeSet::from([TerminalTag::new("private").unwrap()]),
                remove: BTreeSet::from([TerminalTag::new("private").unwrap()]),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(invalid_tags, SdkError::InvalidRequest(_)));

    let group_id = GroupId::new("all-displays").unwrap();
    api.create_group(&CreateGroupRequest {
        id: group_id.clone(),
        name: GroupName::new("All displays").unwrap(),
    })
    .await
    .unwrap();
    let group = GroupReference::new(group_id.to_string()).unwrap();
    let members = UpdateGroupMembersRequest {
        add: BTreeSet::from([
            TerminalReference::new(alpha_id.to_string()).unwrap(),
            TerminalReference::new(beta_id.to_string()).unwrap(),
        ]),
        remove: BTreeSet::new(),
    };
    let updated = api.update_group_members(&group, &members).await.unwrap();
    assert!(updated.changed);
    assert_eq!(updated.resource.members.len(), 2);
    assert!(
        !api.update_group_members(&group, &members)
            .await
            .unwrap()
            .changed
    );
    let (code, membership) = run_json_cli(
        &format!("http://{address}/"),
        Command::Group {
            command: GroupCommand::Add {
                group: group.clone(),
                terminals: vec![beta.clone()],
            },
        },
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(membership["changed"], false);

    let online = TerminalReference::new(alpha_id.to_string()).unwrap();
    let error = api.forget_terminal(&online).await.unwrap_err();
    assert!(matches!(
        &error,
        SdkError::Conflict(value)
            if value.code.as_str() == AdministrativeErrorCode::TERMINAL_ONLINE
    ));

    let (code, refusal) = run_json_cli(
        &format!("http://{address}/"),
        Command::Terminal {
            command: TerminalCommand::Forget {
                terminal: beta.clone(),
            },
        },
        false,
    )
    .await;
    assert_eq!(code, 2);
    assert_eq!(refusal["error"]["code"], "invalid_usage");
    let (code, deleted) = run_json_cli(
        &format!("http://{address}/"),
        Command::Terminal {
            command: TerminalCommand::Forget {
                terminal: beta.clone(),
            },
        },
        true,
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(deleted["deleted"]["id"], beta_id.as_str());
    assert!(api.group(&group).await.unwrap().members.contains(&alpha_id));
    assert!(!api.group(&group).await.unwrap().members.contains(&beta_id));
    assert_eq!(api.delete_group(&group).await.unwrap().deleted.id, group_id);

    shutdown_sender.send(()).unwrap();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn addon_administration_separates_policy_from_host_registration() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = CoreConfiguration {
        terminal_state_path: directory.path().join("terminals.json"),
        presence_timeout: Duration::from_secs(60),
        acknowledgement_timeout: Duration::from_secs(30),
        presence_expiry_interval: Duration::from_secs(3600),
        acknowledgement_expiry_interval: Duration::from_secs(3600),
    };
    let server = CoreServer::new(configuration).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (ready_sender, ready_receiver) = oneshot::channel();
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let task = tokio::spawn(server.serve(listener, Some(ready_sender), async move {
        let _ = shutdown_receiver.await;
    }));
    let address = ready_receiver.await.unwrap();
    let base_url = format!("http://{address}/");
    let clock = addon_manifest("clock", "Shared name", "clock.show");
    let weather = addon_manifest("weather", "Shared name", "weather.show");
    for manifest in [clock.clone(), weather] {
        assert_eq!(
            publish_event(&base_url, EventKind::AddonRegistered { manifest })
                .await
                .status(),
            reqwest::StatusCode::ACCEPTED
        );
    }

    let api = CoreApi::new(CoreApiConfiguration::new(&base_url).unwrap()).unwrap();
    let addons = api.addons().await.unwrap().addons;
    assert_eq!(addons.len(), 2);
    assert_eq!(addons[0].manifest.id, AddonId::new("clock"));
    assert!(addons.iter().all(|addon| addon.enabled && addon.registered));
    let reference = AddonReference::new("clock").unwrap();
    assert_eq!(
        api.addon(&reference).await.unwrap().manifest.version.patch,
        3
    );
    assert!(matches!(
        api.addon(&AddonReference::new("Shared name").unwrap())
            .await
            .unwrap_err(),
        SdkError::AmbiguousReference(_)
    ));

    let (code, disabled) = run_json_cli(
        &base_url,
        Command::Addon {
            command: AddonCommand::Disable {
                addon: reference.clone(),
            },
        },
        false,
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(disabled["changed"], true);
    assert_eq!(disabled["resource"]["enabled"], false);
    assert_eq!(disabled["resource"]["registered"], true);
    assert_eq!(
        reqwest::get(format!("{base_url}api/v1/addons"))
            .await
            .unwrap()
            .json::<Vec<AddonManifest>>()
            .await
            .unwrap()
            .into_iter()
            .map(|manifest| manifest.id)
            .collect::<Vec<_>>(),
        vec![AddonId::new("weather")]
    );

    assert_eq!(
        publish_event(
            &base_url,
            EventKind::AddonStopped {
                addon_id: clock.id.clone(),
            },
        )
        .await
        .status(),
        reqwest::StatusCode::ACCEPTED
    );
    let offline = api.addon(&reference).await.unwrap();
    assert!(!offline.enabled);
    assert!(!offline.registered);
    let enabled = api
        .set_addon_enabled(&reference, &SetAddonEnabledRequest { enabled: true })
        .await
        .unwrap();
    assert!(enabled.changed);
    assert!(enabled.resource.enabled);
    assert!(!enabled.resource.registered);

    shutdown_sender.send(()).unwrap();
    task.await.unwrap().unwrap();

    let restarted = CoreServer::new(CoreConfiguration::production(
        directory.path().join("terminals.json"),
    ))
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (ready_sender, ready_receiver) = oneshot::channel();
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let task = tokio::spawn(restarted.serve(listener, Some(ready_sender), async move {
        let _ = shutdown_receiver.await;
    }));
    let address = ready_receiver.await.unwrap();
    let base_url = format!("http://{address}/");
    assert!(
        CoreApi::new(CoreApiConfiguration::new(&base_url).unwrap())
            .unwrap()
            .addons()
            .await
            .unwrap()
            .addons
            .is_empty()
    );
    publish_event(&base_url, EventKind::AddonRegistered { manifest: clock }).await;
    let restored = CoreApi::new(CoreApiConfiguration::new(&base_url).unwrap())
        .unwrap()
        .addon(&reference)
        .await
        .unwrap();
    assert!(restored.enabled);
    assert!(restored.registered);
    shutdown_sender.send(()).unwrap();
    task.await.unwrap().unwrap();
}
