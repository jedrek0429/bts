use std::{
    collections::{BTreeSet, HashMap, HashSet},
    future::Future,
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use crate::presentations::{
    DEFAULT_ACKNOWLEDGEMENT_EXPIRY_INTERVAL, DEFAULT_ACKNOWLEDGEMENT_TIMEOUT, PresentationManager,
    PresentationOwner,
};
use crate::terminals::{
    DEFAULT_EXPIRY_INTERVAL, DEFAULT_PRESENCE_TIMEOUT, MutationOutcome, TerminalAdminError,
    TerminalDefinition, TerminalGroup, TerminalPresence, TerminalRegistry,
};
use anyhow::Context;
use axum::{
    Json, Router,
    body::Body,
    extract::{
        ConnectInfo, Path as AxumPath, State,
        rejection::JsonRejection,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use bts_protocol::addons::v1::{API_VERSION, ActionId, AddonCapability, AddonId, AddonManifest};
use bts_protocol::core::{
    CORE_ADDONS_PATH, CORE_ADMIN_GROUP_MEMBERS_PATH, CORE_ADMIN_GROUP_NAME_PATH,
    CORE_ADMIN_GROUP_PATH, CORE_ADMIN_GROUPS_PATH, CORE_ADMIN_STATE_PATH, CORE_ADMIN_STATUS_PATH,
    CORE_ADMIN_TERMINAL_DESCRIPTION_PATH, CORE_ADMIN_TERMINAL_NAME_PATH, CORE_ADMIN_TERMINAL_PATH,
    CORE_ADMIN_TERMINAL_TAGS_PATH, CORE_ADMIN_TERMINALS_PATH, CORE_API_DISCOVERY_PATH,
    CORE_API_VERSION, CORE_ASSET_PATH, CORE_ASSETS_PATH, CORE_EVENTS_PATH,
    CORE_EVENTS_WEBSOCKET_PATH, CORE_STATE_PATH, CORE_TELEPHONY_TARGETS_PATH,
    CORE_TERMINAL_EVENTS_WEBSOCKET_PATH, CORE_TERMINALS_WEBSOCKET_PATH,
};
use bts_protocol::{
    AdministrativeApiCompatibility, AdministrativeError, AdministrativeErrorCategory,
    AdministrativeErrorCode, AdministrativeErrorResponse, AdministrativeResourceKind, ApiDiscovery,
    AssetId, AssetRef, AssetUpload, BtsState, CoreOperationalStatus, CoreStateResource,
    CoreStatusResource, CreateGroupRequest, DeletionResponse, DisplayCommand, DtmfMenuKey, Event,
    EventKind, GroupId, GroupListResource, GroupReference, GroupResource, MutationResponse,
    NewEvent, PresentationRequest, RenameGroupRequest, RenameTerminalRequest, ResourceCandidate,
    ServerMessage, SetTerminalDescriptionRequest, TerminalListResource, TerminalPresenceResource,
    TerminalPresentationResource, TerminalReference, TerminalResource, TerminalStateSummary,
    UpdateGroupMembersRequest, UpdateTerminalTagsRequest,
};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{RwLock, broadcast, oneshot, watch};
use tracing::{error, info, warn};

use crate::terminal_transport::TerminalTransport;

const EVENT_CHANNEL_CAPACITY: usize = 128;

#[derive(Clone)]
struct AppState {
    started_at: DateTime<Utc>,
    current: Arc<RwLock<BtsState>>,
    registry: Arc<RwLock<AddonRegistry>>,
    terminals: TerminalRegistry,
    presentations: PresentationManager,
    assets: Arc<RwLock<HashMap<AssetId, StoredAsset>>>,
    events: broadcast::Sender<ServerMessage>,
    terminal_transport: TerminalTransport,
}

struct StoredAsset {
    content_type: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct AddonRegistry {
    manifests: HashMap<AddonId, AddonManifest>,
    actions: HashMap<ActionId, AddonId>,
    digits: HashMap<DtmfMenuKey, AddonId>,
}

#[derive(Debug, Clone)]
pub struct CoreConfiguration {
    pub terminal_state_path: PathBuf,
    pub presence_timeout: Duration,
    pub acknowledgement_timeout: Duration,
    pub presence_expiry_interval: Duration,
    pub acknowledgement_expiry_interval: Duration,
}

impl CoreConfiguration {
    pub fn production(terminal_state_path: PathBuf) -> Self {
        Self {
            terminal_state_path,
            presence_timeout: DEFAULT_PRESENCE_TIMEOUT,
            acknowledgement_timeout: DEFAULT_ACKNOWLEDGEMENT_TIMEOUT,
            presence_expiry_interval: DEFAULT_EXPIRY_INTERVAL,
            acknowledgement_expiry_interval: DEFAULT_ACKNOWLEDGEMENT_EXPIRY_INTERVAL,
        }
    }
}

#[derive(Clone)]
pub struct CoreServices {
    pub terminals: TerminalRegistry,
    pub presentations: PresentationManager,
}

pub struct CoreServer {
    configuration: CoreConfiguration,
    services: CoreServices,
}

impl CoreServer {
    pub fn new(configuration: CoreConfiguration) -> anyhow::Result<Self> {
        let terminals = TerminalRegistry::load(
            &configuration.terminal_state_path,
            configuration.presence_timeout,
        )
        .context("failed to load the terminal registry")?;
        let presentations =
            PresentationManager::new(terminals.clone(), configuration.acknowledgement_timeout);
        Ok(Self {
            configuration,
            services: CoreServices {
                terminals,
                presentations,
            },
        })
    }

    pub fn services(&self) -> CoreServices {
        self.services.clone()
    }

    pub async fn serve<F>(
        self,
        listener: tokio::net::TcpListener,
        ready: Option<oneshot::Sender<SocketAddr>>,
        shutdown: F,
    ) -> anyhow::Result<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let address = listener
            .local_addr()
            .context("failed to read Core listener address")?;
        let (shutdown_sender, shutdown_receiver) = watch::channel(false);
        let transport = TerminalTransport::new(
            self.services.terminals.clone(),
            self.services.presentations.clone(),
            shutdown_receiver.clone(),
            uuid::Uuid::new_v4(),
        );
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let state = AppState {
            started_at: Utc::now(),
            current: Arc::new(RwLock::new(BtsState::default())),
            registry: Arc::new(RwLock::new(AddonRegistry::default())),
            terminals: self.services.terminals,
            presentations: self.services.presentations,
            assets: Arc::new(RwLock::new(HashMap::new())),
            events,
            terminal_transport: transport.clone(),
        };
        let terminal_expiry_task = spawn_terminal_expiry(
            state.terminals.clone(),
            transport,
            self.configuration.presence_expiry_interval,
            shutdown_receiver.clone(),
        );
        let presentation_expiry_task = spawn_presentation_expiry(
            state.presentations.clone(),
            self.configuration.acknowledgement_expiry_interval,
            shutdown_receiver,
        );
        let app = router(state);
        if let Some(ready) = ready {
            let _ = ready.send(address);
        }
        info!(%address, "BTS Core started");
        let graceful_shutdown = async move {
            shutdown.await;
            let _ = shutdown_sender.send(true);
        };
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(graceful_shutdown)
        .await
        .context("BTS Core HTTP server failed");
        terminal_expiry_task
            .await
            .context("terminal expiry task failed")?;
        presentation_expiry_task
            .await
            .context("presentation expiry task failed")?;
        info!("BTS Core stopped");
        result
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(CORE_API_DISCOVERY_PATH, get(api_discovery))
        .route(CORE_ADMIN_STATUS_PATH, get(administrative_status))
        .route(CORE_ADMIN_STATE_PATH, get(administrative_state))
        .route(CORE_ADMIN_TERMINALS_PATH, get(list_terminals))
        .route(
            CORE_ADMIN_TERMINAL_PATH,
            get(get_terminal).delete(forget_terminal),
        )
        .route(
            CORE_ADMIN_TERMINAL_NAME_PATH,
            axum::routing::put(rename_terminal),
        )
        .route(
            CORE_ADMIN_TERMINAL_DESCRIPTION_PATH,
            axum::routing::put(set_terminal_description),
        )
        .route(
            CORE_ADMIN_TERMINAL_TAGS_PATH,
            axum::routing::patch(update_terminal_tags),
        )
        .route(CORE_ADMIN_GROUPS_PATH, get(list_groups).post(create_group))
        .route(CORE_ADMIN_GROUP_PATH, get(get_group).delete(delete_group))
        .route(CORE_ADMIN_GROUP_NAME_PATH, axum::routing::put(rename_group))
        .route(
            CORE_ADMIN_GROUP_MEMBERS_PATH,
            axum::routing::patch(update_group_members),
        )
        .route("/api/v1/admin/{*path}", any(administrative_not_found))
        .route(CORE_STATE_PATH, get(get_state))
        .route(CORE_ADDONS_PATH, get(get_addons))
        .route(CORE_TELEPHONY_TARGETS_PATH, get(get_telephony_targets))
        .route(CORE_ASSETS_PATH, post(upload_asset))
        .route(CORE_ASSET_PATH, get(get_asset))
        .route(CORE_EVENTS_PATH, post(submit_event))
        .route(CORE_EVENTS_WEBSOCKET_PATH, any(websocket_handler))
        .route(
            CORE_TERMINALS_WEBSOCKET_PATH,
            any(terminal_websocket_handler),
        )
        .route(
            CORE_TERMINAL_EVENTS_WEBSOCKET_PATH,
            any(terminal_events_websocket_handler),
        )
        .with_state(state)
}

fn spawn_presentation_expiry(
    manager: PresentationManager,
    expiry_interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(expiry_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = interval.tick() => {
                    manager.expire_acknowledgements(std::time::Instant::now());
                }
            }
        }
    })
}

fn spawn_terminal_expiry(
    registry: TerminalRegistry,
    transport: TerminalTransport,
    expiry_interval: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(expiry_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                _ = interval.tick() => {
                    transport.expire_connections(registry.expire_stale(std::time::Instant::now()));
                }
            }
        }
    })
}

async fn health() -> &'static str {
    "BTS Core is online\n"
}

fn product_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .expect("the workspace package version is valid SemVer")
}

async fn api_discovery() -> Json<ApiDiscovery> {
    Json(ApiDiscovery {
        product: "bts-core".to_owned(),
        product_version: product_version(),
        administrative_api: AdministrativeApiCompatibility {
            current: CORE_API_VERSION,
            supported: BTreeSet::from([CORE_API_VERSION]),
            base_path: bts_protocol::core::CORE_ADMIN_BASE_PATH.to_owned(),
        },
    })
}

async fn administrative_status(State(state): State<AppState>) -> Json<CoreStatusResource> {
    Json(CoreStatusResource {
        status: CoreOperationalStatus::Ready,
        product_version: product_version(),
        administrative_api_version: CORE_API_VERSION,
        started_at: state.started_at,
    })
}

async fn administrative_state(State(state): State<AppState>) -> Json<CoreStateResource> {
    let captured_at = Utc::now();
    let current = state.current.read().await.clone();
    let terminals = state.terminals.routing_snapshot(std::time::Instant::now());
    Json(CoreStateResource {
        captured_at,
        state: current,
        terminals: TerminalStateSummary {
            registered: terminals.definitions.len(),
            online: terminals.presences.len(),
            groups: terminals.groups.len(),
        },
    })
}

async fn list_terminals(State(state): State<AppState>) -> Json<TerminalListResource> {
    let snapshot = state.terminals.routing_snapshot(std::time::Instant::now());
    Json(TerminalListResource {
        terminals: snapshot
            .definitions
            .values()
            .map(|definition| {
                let presentation = state.presentations.terminal_state(&definition.identity.id);
                terminal_resource(
                    definition,
                    snapshot.presences.get(&definition.identity.id),
                    presentation.as_ref(),
                )
            })
            .collect(),
    })
}

async fn get_terminal(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
) -> Response {
    let snapshot = state.terminals.routing_snapshot(std::time::Instant::now());
    let terminal_id = match resolve_terminal(&snapshot.definitions, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    Json(terminal_resource(
        snapshot
            .definitions
            .get(&terminal_id)
            .expect("resolved terminal must exist"),
        snapshot.presences.get(&terminal_id),
        state.presentations.terminal_state(&terminal_id).as_ref(),
    ))
    .into_response()
}

async fn rename_terminal(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
    payload: Result<Json<RenameTerminalRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(error) => return invalid_json(error).into_response(),
    };
    let terminal_id = match resolve_terminal_reference(&state.terminals, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let outcome = match state.terminals.rename_terminal(&terminal_id, request.name) {
        Ok(outcome) => outcome,
        Err(error) => return registry_error(error).into_response(),
    };
    terminal_mutation_response(&state, &terminal_id, outcome)
}

async fn set_terminal_description(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
    payload: Result<Json<SetTerminalDescriptionRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(error) => return invalid_json(error).into_response(),
    };
    let terminal_id = match resolve_terminal_reference(&state.terminals, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let outcome = match state
        .terminals
        .set_terminal_description(&terminal_id, request.description)
    {
        Ok(outcome) => outcome,
        Err(error) => return registry_error(error).into_response(),
    };
    terminal_mutation_response(&state, &terminal_id, outcome)
}

async fn update_terminal_tags(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
    payload: Result<Json<UpdateTerminalTagsRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(error) => return invalid_json(error).into_response(),
    };
    if !request.add.is_disjoint(&request.remove) {
        return invalid_request("A terminal tag cannot be added and removed together")
            .into_response();
    }
    let terminal_id = match resolve_terminal_reference(&state.terminals, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let outcome =
        match state
            .terminals
            .update_terminal_tags(&terminal_id, &request.add, &request.remove)
        {
            Ok(outcome) => outcome,
            Err(error) => return registry_error(error).into_response(),
        };
    terminal_mutation_response(&state, &terminal_id, outcome)
}

async fn forget_terminal(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
) -> Response {
    let terminal_id = match resolve_terminal_reference(&state.terminals, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let presentation = state.presentations.terminal_state(&terminal_id);
    let definition = match state.terminals.forget_terminal(&terminal_id) {
        Ok(definition) => definition,
        Err(error) => return registry_error(error).into_response(),
    };
    state.presentations.forget_terminal_state(&terminal_id);
    Json(DeletionResponse {
        deleted: terminal_resource(&definition, None, presentation.as_ref()),
    })
    .into_response()
}

async fn list_groups(State(state): State<AppState>) -> Json<GroupListResource> {
    Json(GroupListResource {
        groups: state
            .terminals
            .groups()
            .iter()
            .map(group_resource)
            .collect(),
    })
}

async fn create_group(
    State(state): State<AppState>,
    payload: Result<Json<CreateGroupRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(error) => return invalid_json(error).into_response(),
    };
    let group_id = request.id.clone();
    if let Err(error) = state.terminals.create_group(bts_protocol::GroupIdentity {
        id: request.id,
        name: request.name,
    }) {
        return registry_error(error).into_response();
    }
    let group = state
        .terminals
        .group(&group_id)
        .expect("created group must exist");
    (StatusCode::CREATED, Json(group_resource(&group))).into_response()
}

async fn get_group(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
) -> Response {
    let snapshot = state.terminals.routing_snapshot(std::time::Instant::now());
    let group_id = match resolve_group(&snapshot.groups, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    Json(group_resource(
        snapshot
            .groups
            .get(&group_id)
            .expect("resolved group must exist"),
    ))
    .into_response()
}

async fn rename_group(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
    payload: Result<Json<RenameGroupRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(error) => return invalid_json(error).into_response(),
    };
    let group_id = match resolve_group_reference(&state.terminals, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let outcome = match state.terminals.rename_group(&group_id, request.name) {
        Ok(outcome) => outcome,
        Err(error) => return registry_error(error).into_response(),
    };
    group_mutation_response(&state, &group_id, outcome)
}

async fn update_group_members(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
    payload: Result<Json<UpdateGroupMembersRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(error) => return invalid_json(error).into_response(),
    };
    if !request.add.is_disjoint(&request.remove) {
        return invalid_request("A terminal cannot be added to and removed from a group together")
            .into_response();
    }
    let group_id = match resolve_group_reference(&state.terminals, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let definitions = state
        .terminals
        .routing_snapshot(std::time::Instant::now())
        .definitions;
    let add = match resolve_terminal_set(&definitions, &request.add) {
        Ok(ids) => ids,
        Err(error) => return error.into_response(),
    };
    let remove = match resolve_terminal_set(&definitions, &request.remove) {
        Ok(ids) => ids,
        Err(error) => return error.into_response(),
    };
    if !add.is_disjoint(&remove) {
        return invalid_request("A terminal cannot be added to and removed from a group together")
            .into_response();
    }
    let outcome = match state
        .terminals
        .update_group_members(&group_id, &add, &remove)
    {
        Ok(outcome) => outcome,
        Err(error) => return registry_error(error).into_response(),
    };
    group_mutation_response(&state, &group_id, outcome)
}

async fn delete_group(
    State(state): State<AppState>,
    AxumPath(reference): AxumPath<String>,
) -> Response {
    let group_id = match resolve_group_reference(&state.terminals, &reference) {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let group = state
        .terminals
        .group(&group_id)
        .expect("resolved group must exist");
    if let Err(error) = state.terminals.delete_group(&group_id) {
        return registry_error(error).into_response();
    }
    Json(DeletionResponse {
        deleted: group_resource(&group),
    })
    .into_response()
}

fn terminal_mutation_response(
    state: &AppState,
    terminal_id: &bts_protocol::TerminalId,
    outcome: MutationOutcome,
) -> Response {
    let definition = state
        .terminals
        .definition(terminal_id)
        .expect("mutated terminal must exist");
    let presence = state.terminals.presence(terminal_id);
    let presentation = state.presentations.terminal_state(terminal_id);
    Json(MutationResponse {
        changed: outcome == MutationOutcome::Changed,
        resource: terminal_resource(&definition, presence.as_ref(), presentation.as_ref()),
    })
    .into_response()
}

fn group_mutation_response(
    state: &AppState,
    group_id: &GroupId,
    outcome: MutationOutcome,
) -> Response {
    let group = state
        .terminals
        .group(group_id)
        .expect("mutated group must exist");
    Json(MutationResponse {
        changed: outcome == MutationOutcome::Changed,
        resource: group_resource(&group),
    })
    .into_response()
}

fn terminal_resource(
    definition: &TerminalDefinition,
    presence: Option<&TerminalPresence>,
    presentation: Option<&crate::presentations::TerminalPresentationState>,
) -> TerminalResource {
    TerminalResource {
        id: definition.identity.id.clone(),
        name: definition.identity.name.clone(),
        description: definition.description.clone(),
        implementation: definition.implementation.clone(),
        approved_capabilities: definition.approved_capabilities.clone(),
        tags: definition.tags.clone(),
        groups: definition.groups.clone(),
        first_seen: definition.first_seen,
        last_seen: definition.last_seen,
        presence: presence.map(|presence| {
            let connected_elapsed = presence
                .last_seen
                .saturating_duration_since(presence.connected_at);
            let connected_at = presence.last_seen_at
                - chrono::Duration::from_std(connected_elapsed)
                    .unwrap_or_else(|_| chrono::Duration::zero());
            TerminalPresenceResource {
                connected_at,
                last_seen_at: presence.last_seen_at,
                protocol_version: presence.protocol_version,
                declared_capabilities: presence.declared_capabilities.clone(),
                implementation_version: presence.implementation_version.clone(),
                runtime_diagnostics: presence.runtime_diagnostics.clone(),
            }
        }),
        presentation: presentation.map(|presentation| TerminalPresentationResource {
            presentation_id: presentation.presentation_id,
            generation: presentation.generation,
            display: presentation.display.clone(),
            source: presentation.owner.source.clone(),
        }),
    }
}

fn group_resource(group: &TerminalGroup) -> GroupResource {
    GroupResource {
        id: group.identity.id.clone(),
        name: group.identity.name.clone(),
        members: group.members.clone(),
    }
}

fn resolve_terminal_reference(
    registry: &TerminalRegistry,
    reference: &str,
) -> Result<bts_protocol::TerminalId, (StatusCode, Json<AdministrativeErrorResponse>)> {
    resolve_terminal(
        &registry
            .routing_snapshot(std::time::Instant::now())
            .definitions,
        reference,
    )
}

fn resolve_terminal(
    definitions: &std::collections::BTreeMap<bts_protocol::TerminalId, TerminalDefinition>,
    reference: &str,
) -> Result<bts_protocol::TerminalId, (StatusCode, Json<AdministrativeErrorResponse>)> {
    if TerminalReference::new(reference).is_err() {
        return Err(invalid_request("The terminal reference is invalid"));
    }
    if let Ok(id) = bts_protocol::TerminalId::new(reference)
        && definitions.contains_key(&id)
    {
        return Ok(id);
    }
    let matches = definitions
        .values()
        .filter(|definition| definition.identity.name.as_str() == reference)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [definition] => Ok(definition.identity.id.clone()),
        [] => Err(reference_error(
            AdministrativeResourceKind::Terminal,
            reference,
            Vec::new(),
        )),
        _ => Err(reference_error(
            AdministrativeResourceKind::Terminal,
            reference,
            matches
                .into_iter()
                .map(|definition| ResourceCandidate {
                    kind: AdministrativeResourceKind::Terminal,
                    id: definition.identity.id.to_string(),
                    name: definition.identity.name.to_string(),
                })
                .collect(),
        )),
    }
}

fn resolve_terminal_set(
    definitions: &std::collections::BTreeMap<bts_protocol::TerminalId, TerminalDefinition>,
    references: &BTreeSet<TerminalReference>,
) -> Result<BTreeSet<bts_protocol::TerminalId>, (StatusCode, Json<AdministrativeErrorResponse>)> {
    references
        .iter()
        .map(|reference| resolve_terminal(definitions, reference.as_str()))
        .collect()
}

fn resolve_group_reference(
    registry: &TerminalRegistry,
    reference: &str,
) -> Result<GroupId, (StatusCode, Json<AdministrativeErrorResponse>)> {
    resolve_group(
        &registry.routing_snapshot(std::time::Instant::now()).groups,
        reference,
    )
}

fn resolve_group(
    groups: &std::collections::BTreeMap<GroupId, TerminalGroup>,
    reference: &str,
) -> Result<GroupId, (StatusCode, Json<AdministrativeErrorResponse>)> {
    if GroupReference::new(reference).is_err() {
        return Err(invalid_request("The terminal group reference is invalid"));
    }
    if let Ok(id) = GroupId::new(reference)
        && groups.contains_key(&id)
    {
        return Ok(id);
    }
    let matches = groups
        .values()
        .filter(|group| group.identity.name.as_str() == reference)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [group] => Ok(group.identity.id.clone()),
        [] => Err(reference_error(
            AdministrativeResourceKind::Group,
            reference,
            Vec::new(),
        )),
        _ => Err(reference_error(
            AdministrativeResourceKind::Group,
            reference,
            matches
                .into_iter()
                .map(|group| ResourceCandidate {
                    kind: AdministrativeResourceKind::Group,
                    id: group.identity.id.to_string(),
                    name: group.identity.name.to_string(),
                })
                .collect(),
        )),
    }
}

fn reference_error(
    kind: AdministrativeResourceKind,
    reference: &str,
    candidates: Vec<ResourceCandidate>,
) -> (StatusCode, Json<AdministrativeErrorResponse>) {
    let ambiguous = !candidates.is_empty();
    let (status, category, code, message) = match (kind, ambiguous) {
        (AdministrativeResourceKind::Terminal, false) => (
            StatusCode::NOT_FOUND,
            AdministrativeErrorCategory::NotFound,
            AdministrativeErrorCode::TERMINAL_NOT_FOUND,
            "No terminal matches the supplied reference",
        ),
        (AdministrativeResourceKind::Group, false) => (
            StatusCode::NOT_FOUND,
            AdministrativeErrorCategory::NotFound,
            AdministrativeErrorCode::GROUP_NOT_FOUND,
            "No terminal group matches the supplied reference",
        ),
        (AdministrativeResourceKind::Terminal, true) => (
            StatusCode::CONFLICT,
            AdministrativeErrorCategory::AmbiguousReference,
            AdministrativeErrorCode::AMBIGUOUS_TERMINAL_REFERENCE,
            "The terminal reference matches more than one display name",
        ),
        (AdministrativeResourceKind::Group, true) => (
            StatusCode::CONFLICT,
            AdministrativeErrorCategory::AmbiguousReference,
            AdministrativeErrorCode::AMBIGUOUS_GROUP_REFERENCE,
            "The terminal group reference matches more than one display name",
        ),
    };
    administrative_error(
        status,
        category,
        code,
        message,
        Some(kind),
        Some(reference.to_owned()),
        candidates,
    )
}

fn invalid_json(error: JsonRejection) -> (StatusCode, Json<AdministrativeErrorResponse>) {
    invalid_request(&format!(
        "The administrative request body is invalid: {error}"
    ))
}

fn invalid_request(message: &str) -> (StatusCode, Json<AdministrativeErrorResponse>) {
    administrative_error(
        StatusCode::BAD_REQUEST,
        AdministrativeErrorCategory::InvalidInput,
        AdministrativeErrorCode::INVALID_REQUEST,
        message,
        None,
        None,
        Vec::new(),
    )
}

fn registry_error(error: TerminalAdminError) -> (StatusCode, Json<AdministrativeErrorResponse>) {
    match error {
        TerminalAdminError::TerminalNotFound(id) => reference_error(
            AdministrativeResourceKind::Terminal,
            id.as_str(),
            Vec::new(),
        ),
        TerminalAdminError::GroupNotFound(id) => {
            reference_error(AdministrativeResourceKind::Group, id.as_str(), Vec::new())
        }
        TerminalAdminError::TerminalOnline(id) => administrative_error(
            StatusCode::CONFLICT,
            AdministrativeErrorCategory::Conflict,
            AdministrativeErrorCode::TERMINAL_ONLINE,
            "An online terminal cannot be forgotten",
            Some(AdministrativeResourceKind::Terminal),
            Some(id.to_string()),
            Vec::new(),
        ),
        TerminalAdminError::GroupAlreadyExists(id) => administrative_error(
            StatusCode::CONFLICT,
            AdministrativeErrorCategory::Conflict,
            AdministrativeErrorCode::GROUP_ALREADY_EXISTS,
            "A terminal group with this stable identifier already exists",
            Some(AdministrativeResourceKind::Group),
            Some(id.to_string()),
            Vec::new(),
        ),
        TerminalAdminError::InvalidTag { value, detail } => administrative_error(
            StatusCode::BAD_REQUEST,
            AdministrativeErrorCategory::InvalidInput,
            AdministrativeErrorCode::INVALID_REQUEST,
            &format!("Invalid terminal tag {value:?}: {detail}"),
            Some(AdministrativeResourceKind::Terminal),
            None,
            Vec::new(),
        ),
        TerminalAdminError::Persistence(error) => administrative_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            AdministrativeErrorCategory::ServerFailure,
            AdministrativeErrorCode::INTERNAL,
            &format!("Core could not persist the administrative change: {error}"),
            None,
            None,
            Vec::new(),
        ),
    }
}

fn administrative_error(
    status: StatusCode,
    category: AdministrativeErrorCategory,
    code: &str,
    message: &str,
    resource: Option<AdministrativeResourceKind>,
    reference: Option<String>,
    mut candidates: Vec<ResourceCandidate>,
) -> (StatusCode, Json<AdministrativeErrorResponse>) {
    candidates.sort_by(|left, right| left.id.cmp(&right.id));
    (
        status,
        Json(AdministrativeErrorResponse {
            error: AdministrativeError {
                category,
                code: AdministrativeErrorCode::new(code)
                    .expect("the static administrative error code is valid"),
                message: message.to_owned(),
                resource,
                reference,
                candidates,
            },
        }),
    )
}

async fn administrative_not_found() -> (StatusCode, Json<AdministrativeErrorResponse>) {
    (
        StatusCode::NOT_FOUND,
        Json(AdministrativeErrorResponse {
            error: AdministrativeError {
                category: AdministrativeErrorCategory::NotFound,
                code: AdministrativeErrorCode::new("administrative_route_not_found")
                    .expect("the static administrative error code is valid"),
                message: "The requested administrative resource was not found".to_owned(),
                resource: None,
                reference: None,
                candidates: Vec::new(),
            },
        }),
    )
}

async fn get_telephony_targets(
    State(state): State<AppState>,
) -> Json<bts_protocol::TelephonyTargets> {
    Json(crate::telephony::target_catalogue(
        &state.terminals.routing_snapshot(std::time::Instant::now()),
    ))
}

async fn get_state(State(state): State<AppState>) -> Json<BtsState> {
    let current = state.current.read().await.clone();
    Json(current)
}

async fn get_addons(State(state): State<AppState>) -> Json<Vec<AddonManifest>> {
    let mut manifests: Vec<_> = state
        .registry
        .read()
        .await
        .manifests
        .values()
        .cloned()
        .collect();
    manifests.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    Json(manifests)
}

async fn upload_asset(
    State(state): State<AppState>,
    Json(upload): Json<AssetUpload>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let registry = state.registry.read().await;
    let manifest = registry.manifests.get(&upload.addon_id).ok_or((
        StatusCode::UNPROCESSABLE_ENTITY,
        format!("addon {} is not registered", upload.addon_id),
    ))?;
    if !manifest.capabilities.contains(&AddonCapability::Assets) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("addon {} did not declare asset access", upload.addon_id),
        ));
    }
    if upload.content_type.trim().is_empty() || upload.bytes.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "asset content type and bytes must not be empty".to_owned(),
        ));
    }
    drop(registry);
    let reference = AssetRef {
        id: AssetId::new(),
        content_type: upload.content_type.clone(),
    };
    state.assets.write().await.insert(
        reference.id,
        StoredAsset {
            content_type: upload.content_type,
            bytes: upload.bytes,
        },
    );
    Ok((StatusCode::CREATED, Json(reference)))
}

async fn get_asset(
    AxumPath(asset_id): AxumPath<uuid::Uuid>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let assets = state.assets.read().await;
    let asset = assets
        .get(&AssetId(asset_id))
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok((
        [(header::CONTENT_TYPE, asset.content_type.clone())],
        Body::from(asset.bytes.clone()),
    ))
}

async fn submit_event(
    State(state): State<AppState>,
    Json(new_event): Json<NewEvent>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let event = Event::new(new_event.source, new_event.kind);

    validate_and_register(&state, &event).await?;

    let now = std::time::Instant::now();
    let plans = match &event.kind {
        EventKind::PresentationRequested { request } => vec![
            state
                .presentations
                .begin_dispatch(
                    request.clone(),
                    PresentationOwner {
                        source: event.source.clone(),
                        addon_id: Some(AddonId::new(event.source.clone())),
                    },
                    now,
                )
                .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?,
        ],
        _ =>
        {
            #[allow(deprecated)]
            state
                .presentations
                .begin_legacy_event(&event, now)
                .map_err(|error| (StatusCode::CONFLICT, error.to_string()))?
        }
    };

    let updated_state = {
        let mut current = state.current.write().await;
        apply_event(&mut current, &event)?;
        current.clone()
    };

    info!(
        event_id = %event.id,
        source = %event.source,
        kind = ?event.kind,
        "event accepted"
    );

    if !matches!(event.kind, EventKind::PresentationRequested { .. }) {
        let message = ServerMessage::Event {
            event: Box::new(event.clone()),
            state: updated_state,
        };
        // The adjacent-version stream never receives the new targeted event.
        let _ = state.events.send(message);
    }
    state.terminal_transport.fan_out(plans);

    Ok((StatusCode::ACCEPTED, Json(event)))
}

async fn validate_and_register(
    state: &AppState,
    event: &Event,
) -> Result<(), (StatusCode, String)> {
    let mut registry = state.registry.write().await;
    match &event.kind {
        EventKind::AddonRegistered { manifest } => registry.register(manifest.clone()),
        EventKind::AddonStopped { addon_id } => {
            registry.unregister(addon_id);
            Ok(())
        }
        EventKind::ActionRequested { request } => {
            if registry.actions.contains_key(&request.action) {
                Ok(())
            } else {
                Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("action {} is not registered", request.action),
                ))
            }
        }
        EventKind::DisplayRequested { command } => registry.validate_display(command),
        EventKind::PresentationRequested { request } => {
            registry.validate_presentation(&event.source, request)
        }
        _ => Ok(()),
    }
}

fn apply_event(state: &mut BtsState, event: &Event) -> Result<(), (StatusCode, String)> {
    match &event.kind {
        EventKind::DisplayRequested { command } => apply_display_command(state, command),
        EventKind::AddonStopped { addon_id } => {
            if state
                .display_lease
                .as_ref()
                .is_some_and(|lease| &lease.owner == addon_id)
            {
                state.display = bts_protocol::DisplayState::Blank;
                state.display_lease = None;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn apply_display_command(
    state: &mut BtsState,
    command: &DisplayCommand,
) -> Result<(), (StatusCode, String)> {
    match command {
        DisplayCommand::Show { lease, display } => {
            if state
                .display_lease
                .as_ref()
                .is_some_and(|active| active.priority > lease.priority)
            {
                return Ok(());
            }
            state.display = display.clone();
            state.display_lease = Some(lease.clone());
        }
        DisplayCommand::Update {
            addon_id,
            lease_id,
            display,
        } => {
            if state
                .display_lease
                .as_ref()
                .is_some_and(|active| &active.owner == addon_id && active.id == *lease_id)
            {
                state.display = display.clone();
            }
        }
        DisplayCommand::Release { addon_id, lease_id } => {
            if state
                .display_lease
                .as_ref()
                .is_some_and(|active| &active.owner == addon_id && active.id == *lease_id)
            {
                state.display = bts_protocol::DisplayState::Blank;
                state.display_lease = None;
            }
        }
        DisplayCommand::ReleaseAll { addon_id } => {
            if state
                .display_lease
                .as_ref()
                .is_some_and(|lease| &lease.owner == addon_id)
            {
                state.display = bts_protocol::DisplayState::Blank;
                state.display_lease = None;
            }
        }
    }
    Ok(())
}

impl AddonRegistry {
    fn register(&mut self, manifest: AddonManifest) -> Result<(), (StatusCode, String)> {
        validate_manifest(&manifest)?;
        if self.manifests.contains_key(&manifest.id) {
            return Err((
                StatusCode::CONFLICT,
                format!("addon {} is already registered", manifest.id),
            ));
        }
        for action in &manifest.actions {
            if let Some(owner) = self.actions.get(&action.id) {
                return Err((
                    StatusCode::CONFLICT,
                    format!("action {} is already registered by {}", action.id, owner),
                ));
            }
        }
        for entry in &manifest.menu {
            if let Some(owner) = self.digits.get(&entry.digit) {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "menu digit {} is already registered by {}",
                        entry.digit, owner
                    ),
                ));
            }
        }
        for action in &manifest.actions {
            self.actions.insert(action.id.clone(), manifest.id.clone());
        }
        for entry in &manifest.menu {
            self.digits.insert(entry.digit, manifest.id.clone());
        }
        self.manifests.insert(manifest.id.clone(), manifest);
        Ok(())
    }

    fn unregister(&mut self, addon_id: &AddonId) {
        if let Some(manifest) = self.manifests.remove(addon_id) {
            for action in manifest.actions {
                self.actions.remove(&action.id);
            }
            for entry in manifest.menu {
                self.digits.remove(&entry.digit);
            }
        }
    }

    fn validate_display(&self, command: &DisplayCommand) -> Result<(), (StatusCode, String)> {
        let (addon_id, display) = match command {
            DisplayCommand::Show { lease, display } => (&lease.owner, Some(display)),
            DisplayCommand::Update {
                addon_id, display, ..
            } => (addon_id, Some(display)),
            DisplayCommand::Release { addon_id, .. } | DisplayCommand::ReleaseAll { addon_id } => {
                (addon_id, None)
            }
        };
        let manifest = self.manifests.get(addon_id).ok_or((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("addon {addon_id} is not registered"),
        ))?;
        if let Some(display) = display
            && (!manifest.capabilities.contains(&AddonCapability::Display)
                || !manifest.screens.contains(&display.kind()))
        {
            return Err((
                StatusCode::FORBIDDEN,
                format!(
                    "addon {addon_id} did not declare the {:?} screen",
                    display.kind()
                ),
            ));
        }
        Ok(())
    }

    fn validate_presentation(
        &self,
        source: &str,
        request: &PresentationRequest,
    ) -> Result<(), (StatusCode, String)> {
        let addon_id = AddonId::new(source);
        let manifest = self.manifests.get(&addon_id).ok_or((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("addon {addon_id} is not registered"),
        ))?;
        if !manifest.capabilities.contains(&AddonCapability::Display)
            || !manifest.screens.contains(&request.display.kind())
        {
            return Err((
                StatusCode::FORBIDDEN,
                format!(
                    "addon {addon_id} did not declare the {:?} screen",
                    request.display.kind()
                ),
            ));
        }
        Ok(())
    }
}

fn validate_manifest(manifest: &AddonManifest) -> Result<(), (StatusCode, String)> {
    if manifest.api_version != API_VERSION {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported addon API version".to_owned(),
        ));
    }
    if manifest.id.as_str().is_empty() || manifest.name.trim().is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "addon identity and name must not be empty".to_owned(),
        ));
    }
    let actions: HashSet<_> = manifest.actions.iter().map(|action| &action.id).collect();
    if actions.len() != manifest.actions.len() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "manifest contains duplicate actions".to_owned(),
        ));
    }
    let digits: HashSet<_> = manifest.menu.iter().map(|entry| entry.digit).collect();
    if digits.len() != manifest.menu.len() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "manifest contains duplicate menu digits".to_owned(),
        ));
    }
    for entry in &manifest.menu {
        if !actions.contains(&entry.action) || entry.prompt.trim().is_empty() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("invalid menu entry for digit {}", entry.digit),
            ));
        }
    }
    Ok(())
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| websocket_connection(socket, state))
}

async fn terminal_websocket_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(remote_address): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        state
            .terminal_transport
            .connection(socket, remote_address)
            .await;
    })
}

async fn terminal_events_websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| terminal_events_connection(socket, state))
}

async fn terminal_events_connection(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut registry_events = state.terminals.subscribe_changes();
    let mut presentation_events = state.presentations.subscribe_changes();
    loop {
        tokio::select! {
            event = registry_events.recv() => match event {
                Ok(event) if send_json(&mut sender, &event).await.is_ok() => {}
                Ok(_) | Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "terminal event client lagged behind registry changes");
                }
            },
            event = presentation_events.recv() => match event {
                Ok(event) if send_json(&mut sender, &event).await.is_ok() => {}
                Ok(_) | Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(skipped, "terminal event client lagged behind delivery changes");
                }
            },
            message = receiver.next() => match message {
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    }
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
    value: &impl serde::Serialize,
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

#[cfg(test)]
mod tests {
    use super::*;
    use bts_protocol::addons::v1::{
        API_VERSION, ActionRegistration, AddonCapability, AddonVersion, MenuEntry,
    };
    use bts_protocol::{DisplayLease, DisplayLeaseId, DisplayState, DtmfMenuKey, ScreenKind};

    fn manifest(id: &str, action: &str, digit: char) -> AddonManifest {
        AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new(id),
            name: id.to_owned(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionRegistration {
                id: ActionId::new(action),
                description: "Test".to_owned(),
            }],
            menu: vec![MenuEntry {
                digit: DtmfMenuKey::new(digit).unwrap(),
                prompt: "sound:test".to_owned(),
                action: ActionId::new(action),
                order: 1,
            }],
            capabilities: vec![AddonCapability::Display],
            screens: vec![ScreenKind::Message],
        }
    }

    #[test]
    fn registry_rejects_duplicate_ids_actions_and_digits() {
        let mut registry = AddonRegistry::default();
        registry.register(manifest("one", "one.run", '1')).unwrap();

        assert!(
            registry
                .register(manifest("one", "two.run", '2'))
                .unwrap_err()
                .1
                .contains("already registered")
        );
        assert!(
            registry
                .register(manifest("two", "one.run", '2'))
                .unwrap_err()
                .1
                .contains("action")
        );
        assert!(
            registry
                .register(manifest("two", "two.run", '1'))
                .unwrap_err()
                .1
                .contains("digit")
        );
    }

    #[test]
    fn manifest_validation_rejects_invalid_menu_entries() {
        let mut invalid = manifest("one", "one.run", '1');
        invalid.menu[0].action = ActionId::new("missing");
        assert!(validate_manifest(&invalid).is_err());
    }

    #[test]
    fn registry_enforces_declared_display_screens() {
        let mut registry = AddonRegistry::default();
        registry.register(manifest("one", "one.run", '1')).unwrap();
        let command = DisplayCommand::Show {
            lease: DisplayLease {
                id: DisplayLeaseId::new(),
                owner: AddonId::new("one"),
                priority: 1,
            },
            display: DisplayState::Clock {
                time: "12:00".into(),
                seconds: "00".into(),
                date: "Today".into(),
            },
        };
        assert_eq!(
            registry.validate_display(&command).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn legacy_state_projection_ignores_stale_or_foreign_updates() {
        let owner = AddonId::new("one");
        let lease_id = DisplayLeaseId::new();
        let mut state = BtsState::default();
        apply_display_command(
            &mut state,
            &DisplayCommand::Show {
                lease: DisplayLease {
                    id: lease_id,
                    owner: owner.clone(),
                    priority: 10,
                },
                display: DisplayState::Message {
                    title: "One".into(),
                    body: "First".into(),
                },
            },
        )
        .unwrap();
        apply_display_command(
            &mut state,
            &DisplayCommand::Update {
                addon_id: AddonId::new("two"),
                lease_id,
                display: DisplayState::Blank,
            },
        )
        .unwrap();
        apply_display_command(
            &mut state,
            &DisplayCommand::Update {
                addon_id: owner,
                lease_id: DisplayLeaseId::new(),
                display: DisplayState::Blank,
            },
        )
        .unwrap();
        assert_eq!(state.display_lease.unwrap().id, lease_id);
        assert!(matches!(
            state.display,
            DisplayState::Message { ref body, .. } if body == "First"
        ));
    }

    #[test]
    fn legacy_state_projection_keeps_the_higher_priority_display() {
        let mut state = BtsState::default();
        apply_display_command(
            &mut state,
            &DisplayCommand::Show {
                lease: DisplayLease {
                    id: DisplayLeaseId::new(),
                    owner: AddonId::new("high"),
                    priority: 20,
                },
                display: DisplayState::Blank,
            },
        )
        .unwrap();
        apply_display_command(
            &mut state,
            &DisplayCommand::Show {
                lease: DisplayLease {
                    id: DisplayLeaseId::new(),
                    owner: AddonId::new("low"),
                    priority: 10,
                },
                display: DisplayState::Blank,
            },
        )
        .unwrap();
        assert_eq!(state.display_lease.unwrap().priority, 20);
    }

    #[test]
    fn stopping_owner_releases_display() {
        let owner = AddonId::new("one");
        let mut state = BtsState {
            display: DisplayState::Message {
                title: "One".into(),
                body: "Owned".into(),
            },
            display_lease: Some(DisplayLease {
                id: DisplayLeaseId::new(),
                owner: owner.clone(),
                priority: 1,
            }),
        };
        let event = Event::new("host", EventKind::AddonStopped { addon_id: owner });
        apply_event(&mut state, &event).unwrap();
        assert!(state.display_lease.is_none());
        assert_eq!(state.display, DisplayState::Blank);
    }
}
