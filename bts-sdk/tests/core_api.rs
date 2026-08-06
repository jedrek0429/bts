use std::{collections::BTreeSet, future::pending, sync::Arc, time::Duration};

use axum::{Json, Router, http::HeaderMap, routing::get};
use bts_protocol::{
    AdministrativeError, AdministrativeErrorCategory, AdministrativeErrorCode,
    AdministrativeErrorResponse, ApiDiscovery, CoreOperationalStatus, CoreStateResource,
    CoreStatusResource,
    core::{CORE_ADMIN_STATE_PATH, CORE_ADMIN_STATUS_PATH, CORE_API_DISCOVERY_PATH},
};
use bts_sdk::{CoreApi, CoreApiConfiguration, SdkError};
use chrono::Utc;
use semver::Version;
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};

struct Fixture {
    base_url: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn spawn(app: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_receiver.await;
                })
                .await
                .unwrap();
        });
        Self {
            base_url: format!("http://{address}/"),
            shutdown: Some(shutdown),
            task,
        }
    }

    fn api(&self) -> CoreApi {
        CoreApi::new(CoreApiConfiguration::new(&self.base_url).unwrap()).unwrap()
    }

    async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        self.task.await.unwrap();
    }
}

fn discovery(current: u16, supported: BTreeSet<u16>) -> ApiDiscovery {
    ApiDiscovery {
        product: "bts-core".to_owned(),
        product_version: Version::new(0, 3, 0),
        administrative_api: bts_protocol::AdministrativeApiCompatibility {
            current,
            supported,
            base_path: "/api/v1/admin".to_owned(),
        },
    }
}

fn compatible_app() -> Router {
    Router::new()
        .route(
            CORE_API_DISCOVERY_PATH,
            get(|| async { Json(discovery(1, BTreeSet::from([1]))) }),
        )
        .route(
            CORE_ADMIN_STATUS_PATH,
            get(|| async {
                Json(CoreStatusResource {
                    status: CoreOperationalStatus::Ready,
                    product_version: Version::new(0, 3, 0),
                    administrative_api_version: 1,
                    started_at: Utc::now(),
                })
            }),
        )
        .route(
            CORE_ADMIN_STATE_PATH,
            get(|| async {
                Json(CoreStateResource {
                    captured_at: Utc::now(),
                    state: bts_protocol::BtsState::default(),
                    terminals: bts_protocol::TerminalStateSummary {
                        registered: 2,
                        online: 1,
                        groups: 1,
                    },
                })
            }),
        )
}

#[test]
fn configuration_validates_urls_and_timeout_without_global_state() {
    for invalid in [
        "not a URL",
        "ftp://core.example/",
        "http://",
        "http://user:secret@core.example/",
        "http://core.example/prefix",
        "http://core.example/?query=yes",
        "http://core.example/#fragment",
    ] {
        assert!(CoreApiConfiguration::new(invalid).is_err(), "{invalid}");
    }

    let configuration = CoreApiConfiguration::new("https://core.example/")
        .unwrap()
        .with_request_timeout(Duration::from_secs(3))
        .unwrap();
    assert_eq!(configuration.base_url().as_str(), "https://core.example/");
    assert_eq!(configuration.request_timeout(), Duration::from_secs(3));
    assert!(
        CoreApiConfiguration::new("http://core.example/")
            .unwrap()
            .with_request_timeout(Duration::ZERO)
            .is_err()
    );
}

#[tokio::test]
async fn discovers_and_decodes_typed_status_and_state() {
    let fixture = Fixture::spawn(compatible_app()).await;
    let api = fixture.api();

    assert_eq!(api.discover().await.unwrap().administrative_api.current, 1);
    assert_eq!(
        api.status().await.unwrap().status,
        CoreOperationalStatus::Ready
    );
    let state = api.state().await.unwrap();
    assert_eq!(state.terminals.registered, 2);
    assert_eq!(state.terminals.online, 1);
    fixture.stop().await;
}

#[tokio::test]
async fn sends_sdk_metadata_and_ignores_unknown_response_fields() {
    let observed = Arc::new(Mutex::new(None));
    let handler_observed = observed.clone();
    let app = Router::new().route(
        CORE_API_DISCOVERY_PATH,
        get(move |headers: HeaderMap| {
            let observed = handler_observed.clone();
            async move {
                *observed.lock().await = Some(headers);
                Json(json!({
                    "product": "bts-core",
                    "product_version": "0.3.0",
                    "administrative_api": {
                        "current": 1,
                        "supported": [1],
                        "base_path": "/api/v1/admin",
                        "future_field": true
                    },
                    "future_top_level_field": {"value": 42}
                }))
            }
        }),
    );
    let fixture = Fixture::spawn(app).await;
    fixture.api().discover().await.unwrap();

    let headers = observed.lock().await.take().unwrap();
    assert!(
        headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("bts-sdk/")
    );
    assert_eq!(headers["x-bts-sdk-version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(headers["x-bts-administrative-api-version"], "1");
    fixture.stop().await;
}

#[tokio::test]
async fn rejects_incompatible_versions_before_accessing_versioned_resources() {
    let app = Router::new().route(
        CORE_API_DISCOVERY_PATH,
        get(|| async { Json(discovery(2, BTreeSet::from([2]))) }),
    );
    let fixture = Fixture::spawn(app).await;
    let error = fixture.api().discover_compatible().await.unwrap_err();
    assert!(matches!(
        error,
        SdkError::IncompatibleApi {
            core_current: 2,
            ..
        }
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn joins_resources_to_the_validated_advertised_base_path() {
    let app = Router::new()
        .route(
            CORE_API_DISCOVERY_PATH,
            get(|| async {
                let mut value = discovery(1, BTreeSet::from([1]));
                value.administrative_api.base_path = "/alternate/admin".to_owned();
                Json(value)
            }),
        )
        .route(
            "/alternate/admin/status",
            get(|| async {
                Json(CoreStatusResource {
                    status: CoreOperationalStatus::Ready,
                    product_version: Version::new(0, 3, 0),
                    administrative_api_version: 1,
                    started_at: Utc::now(),
                })
            }),
        );
    let fixture = Fixture::spawn(app).await;
    assert_eq!(
        fixture.api().status().await.unwrap().status,
        CoreOperationalStatus::Ready
    );
    fixture.stop().await;
}

#[tokio::test]
async fn classifies_every_structured_administrative_error() {
    let cases = [
        (AdministrativeErrorCategory::InvalidInput, "invalid_request"),
        (AdministrativeErrorCategory::NotFound, "terminal_not_found"),
        (
            AdministrativeErrorCategory::AmbiguousReference,
            "ambiguous_terminal_reference",
        ),
        (AdministrativeErrorCategory::Conflict, "terminal_online"),
        (AdministrativeErrorCategory::Rejected, "mutation_rejected"),
        (
            AdministrativeErrorCategory::IncompatibleApi,
            "unsupported_administrative_api",
        ),
        (AdministrativeErrorCategory::ServerFailure, "internal"),
    ];

    for (category, code) in cases {
        let body = AdministrativeErrorResponse {
            error: AdministrativeError {
                category,
                code: AdministrativeErrorCode::new(code).unwrap(),
                message: "fixture failure".to_owned(),
                resource: None,
                reference: None,
                candidates: Vec::new(),
            },
        };
        let app = Router::new()
            .route(
                CORE_API_DISCOVERY_PATH,
                get(|| async { Json(discovery(1, BTreeSet::from([1]))) }),
            )
            .route(
                CORE_ADMIN_STATUS_PATH,
                get(move || {
                    let body = body.clone();
                    async move { (axum::http::StatusCode::BAD_REQUEST, Json(body)) }
                }),
            );
        let fixture = Fixture::spawn(app).await;
        let error = fixture.api().status().await.unwrap_err();
        assert_eq!(error.administrative_error().unwrap().category, category);
        match category {
            AdministrativeErrorCategory::InvalidInput => {
                assert!(matches!(error, SdkError::InvalidRequest(_)))
            }
            AdministrativeErrorCategory::NotFound => {
                assert!(matches!(error, SdkError::NotFound(_)))
            }
            AdministrativeErrorCategory::AmbiguousReference => {
                assert!(matches!(error, SdkError::AmbiguousReference(_)))
            }
            AdministrativeErrorCategory::Conflict => {
                assert!(matches!(error, SdkError::Conflict(_)))
            }
            AdministrativeErrorCategory::Rejected => {
                assert!(matches!(error, SdkError::Rejected(_)))
            }
            AdministrativeErrorCategory::IncompatibleApi => {
                assert!(matches!(error, SdkError::IncompatibleApiResponse(_)))
            }
            AdministrativeErrorCategory::ServerFailure => {
                assert!(matches!(error, SdkError::ServerFailure(_)))
            }
        }
        fixture.stop().await;
    }
}

#[tokio::test]
async fn distinguishes_malformed_timeout_and_unavailable_core_failures() {
    let malformed = Fixture::spawn(Router::new().route(
        CORE_API_DISCOVERY_PATH,
        get(|| async { (axum::http::StatusCode::OK, "not JSON") }),
    ))
    .await;
    assert!(matches!(
        malformed.api().discover().await.unwrap_err(),
        SdkError::MalformedResponse {
            status: Some(200),
            ..
        }
    ));
    malformed.stop().await;

    let timeout = Fixture::spawn(Router::new().route(
        CORE_API_DISCOVERY_PATH,
        get(|| async { pending::<Json<Value>>().await }),
    ))
    .await;
    let api = CoreApi::new(
        CoreApiConfiguration::new(&timeout.base_url)
            .unwrap()
            .with_request_timeout(Duration::from_millis(25))
            .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        api.discover().await.unwrap_err(),
        SdkError::Timeout { .. }
    ));
    timeout.stop().await;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let api =
        CoreApi::new(CoreApiConfiguration::new(format!("http://{address}/")).unwrap()).unwrap();
    assert!(matches!(
        api.discover().await.unwrap_err(),
        SdkError::Transport(_)
    ));
}
