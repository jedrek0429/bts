use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use bts_cli::{
    cli::{Cli, Command, StateCommand},
    config::{ColourMode, Environment, OutputMode},
    output::OutputStreams,
};
use bts_core::server::{CoreConfiguration, CoreServer};
use bts_protocol::{
    AdministrativeErrorCategory, AdministrativeErrorCode, AdministrativeErrorResponse,
    CoreOperationalStatus, GroupId, GroupName, ProtocolVersion, TerminalCapabilities,
    TerminalCapability, TerminalConnectionId, TerminalId, TerminalIdentity,
    TerminalImplementationId, TerminalName, TerminalRegistration, TerminalTag,
    core::CORE_API_VERSION,
};
use bts_sdk::{
    CoreApi, CoreApiConfiguration, CreateGroupRequest, GroupReference, RenameTerminalRequest,
    SdkError, TerminalReference, UpdateGroupMembersRequest, UpdateTerminalTagsRequest,
};
use tokio::sync::oneshot;

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

    let online = TerminalReference::new(alpha_id.to_string()).unwrap();
    let error = api.forget_terminal(&online).await.unwrap_err();
    assert!(matches!(
        &error,
        SdkError::Conflict(value)
            if value.code.as_str() == AdministrativeErrorCode::TERMINAL_ONLINE
    ));

    let deleted = api.forget_terminal(&beta).await.unwrap();
    assert_eq!(deleted.deleted.id, beta_id);
    assert!(api.group(&group).await.unwrap().members.contains(&alpha_id));
    assert!(!api.group(&group).await.unwrap().members.contains(&beta_id));
    assert_eq!(api.delete_group(&group).await.unwrap().deleted.id, group_id);

    shutdown_sender.send(()).unwrap();
    task.await.unwrap().unwrap();
}
