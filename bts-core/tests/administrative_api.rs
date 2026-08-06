use std::time::{Duration, Instant};

use bts_core::server::{CoreConfiguration, CoreServer};
use bts_protocol::{
    AdministrativeErrorCategory, AdministrativeErrorResponse, CoreOperationalStatus,
    core::CORE_API_VERSION,
};
use bts_sdk::{CoreApi, CoreApiConfiguration};
use tokio::sync::oneshot;

#[tokio::test]
async fn sdk_observes_real_core_without_creating_terminal_presence() {
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
