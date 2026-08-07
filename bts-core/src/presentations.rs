//! Core-owned target resolution, bounded delivery and per-terminal presentation state.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use bts_protocol::addons::v1::AddonId;
use bts_protocol::{
    DisplayCommand, DisplayLease, DisplayState, Event, EventKind, PresentationDeliveryContext,
    PresentationDeliveryOutcome, PresentationDeliveryResult, PresentationDispatch,
    PresentationGeneration, PresentationId, PresentationRejection, PresentationRequest,
    ResolvedTarget, TagMatch, TargetScope, TerminalCapabilities, TerminalCapability,
    TerminalConnectionId, TerminalEvent, TerminalEventKind, TerminalId, TerminalTarget,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::terminals::{TerminalRegistry, TerminalRoutingSnapshot};

pub const DEFAULT_ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_ACKNOWLEDGEMENT_EXPIRY_INTERVAL: Duration = Duration::from_secs(1);
pub const DEFAULT_COMPLETED_DELIVERY_RETENTION: usize = 256;
pub const DEFAULT_EVICTED_DELIVERY_RETENTION: usize = 256;

const DELIVERY_CHANNEL_CAPACITY: usize = 128;

/// The semantic owner retained alongside an accepted presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationOwner {
    pub source: String,
    pub addon_id: Option<AddonId>,
}

/// Core's effective semantic presentation for one terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresentationState {
    pub presentation_id: PresentationId,
    pub generation: PresentationGeneration,
    pub display: DisplayState,
    pub owner: PresentationOwner,
    pub legacy_lease: Option<DisplayLease>,
}

/// A pure target-resolution result. `registered_matches` includes offline
/// definitions while `resolved_target` follows the target's requested scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetResolution {
    pub requested_target: TerminalTarget,
    pub registered_matches: BTreeSet<TerminalId>,
    pub resolved_target: Option<ResolvedTarget>,
}

/// One transport destination selected from the presence snapshot used to plan
/// a dispatch. The connection ID prevents a reconnect from inheriting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationRecipient {
    pub terminal_id: TerminalId,
    pub connection_id: TerminalConnectionId,
    pub generation: PresentationGeneration,
}

/// The immutable dispatch and destinations a terminal transport should send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationPlan {
    pub dispatch: Option<PresentationDispatch>,
    pub recipients: Vec<PresentationRecipient>,
    pub result: PresentationDeliveryResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcknowledgementDisposition {
    Accepted,
    Rejected,
    Duplicate,
    Late,
    StaleConnection,
    UnknownPresentation,
    UnexpectedTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationRetention {
    pub completed: usize,
    pub evicted_tombstones: usize,
}

impl Default for PresentationRetention {
    fn default() -> Self {
        Self {
            completed: DEFAULT_COMPLETED_DELIVERY_RETENTION,
            evicted_tombstones: DEFAULT_EVICTED_DELIVERY_RETENTION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicatePresentationId;

impl std::fmt::Display for DuplicatePresentationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("presentation identifier has already been dispatched")
    }
}

impl std::error::Error for DuplicatePresentationId {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyPresentationError {
    DuplicateLease,
    LeaseNotFound,
    DuplicatePresentationId,
}

impl std::fmt::Display for LegacyPresentationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateLease => "the display lease has already been shown",
            Self::LeaseNotFound => "the display lease is stale or belongs to another addon",
            Self::DuplicatePresentationId => {
                "the generated presentation identifier has already been dispatched"
            }
        })
    }
}

impl std::error::Error for LegacyPresentationError {}

impl From<DuplicatePresentationId> for LegacyPresentationError {
    fn from(_: DuplicatePresentationId) -> Self {
        Self::DuplicatePresentationId
    }
}

#[derive(Debug, Clone)]
struct PlannedDelivery {
    connection_id: TerminalConnectionId,
    generation: PresentationGeneration,
    deadline: Instant,
}

#[derive(Debug, Clone)]
struct DeliveryRecord {
    request: PresentationRequest,
    owner: PresentationOwner,
    result: PresentationDeliveryResult,
    planned: BTreeMap<TerminalId, PlannedDelivery>,
    legacy_lease: Option<DisplayLease>,
    completion_emitted: bool,
}

#[derive(Debug, Clone)]
struct DeliveryTombstone {
    planned_connections: BTreeMap<TerminalId, TerminalConnectionId>,
    outcomes: BTreeMap<TerminalId, PresentationDeliveryOutcome>,
}

#[derive(Debug, Clone)]
struct LegacyLeaseState {
    lease: DisplayLease,
    display: DisplayState,
    order: u64,
}

#[derive(Debug, Clone)]
struct LegacyDispatch {
    terminal_id: TerminalId,
    display: DisplayState,
    lease: Option<DisplayLease>,
}

#[derive(Debug, Default)]
struct PresentationStore {
    deliveries: BTreeMap<PresentationId, DeliveryRecord>,
    completed_order: VecDeque<PresentationId>,
    tombstones: BTreeMap<PresentationId, DeliveryTombstone>,
    tombstone_order: VecDeque<PresentationId>,
    states: BTreeMap<TerminalId, TerminalPresentationState>,
    generations: BTreeMap<TerminalId, u64>,
    legacy_leases: BTreeMap<TerminalId, Vec<LegacyLeaseState>>,
    legacy_order: u64,
}

/// Cloneable, concurrency-safe Core presentation authority.
#[derive(Clone)]
pub struct PresentationManager {
    registry: TerminalRegistry,
    acknowledgement_timeout: Duration,
    retention: PresentationRetention,
    store: Arc<Mutex<PresentationStore>>,
    legacy_gate: Arc<Mutex<()>>,
    changes: broadcast::Sender<TerminalEvent>,
}

impl PresentationManager {
    pub fn new(registry: TerminalRegistry, acknowledgement_timeout: Duration) -> Self {
        Self::with_retention(
            registry,
            acknowledgement_timeout,
            PresentationRetention::default(),
        )
    }

    pub fn with_retention(
        registry: TerminalRegistry,
        acknowledgement_timeout: Duration,
        retention: PresentationRetention,
    ) -> Self {
        let (changes, _) = broadcast::channel(DELIVERY_CHANNEL_CAPACITY);
        Self {
            registry,
            acknowledgement_timeout,
            retention,
            store: Arc::new(Mutex::new(PresentationStore::default())),
            legacy_gate: Arc::new(Mutex::new(())),
            changes,
        }
    }

    pub fn subscribe_changes(&self) -> broadcast::Receiver<TerminalEvent> {
        self.changes.subscribe()
    }

    /// Resolves a selector without dispatching or changing presentation state.
    pub fn resolve_target(&self, target: &TerminalTarget, now: Instant) -> TargetResolution {
        resolve_snapshot(&self.registry.routing_snapshot(now), target).0
    }

    /// Plans a dispatch from one atomic registry snapshot and starts its bounded
    /// acknowledgement window. This method performs no I/O and never waits.
    pub fn begin_dispatch(
        &self,
        request: PresentationRequest,
        owner: PresentationOwner,
        now: Instant,
    ) -> Result<PresentationPlan, DuplicatePresentationId> {
        self.begin_dispatch_owned(request, owner, None, now)
    }

    fn begin_dispatch_owned(
        &self,
        request: PresentationRequest,
        owner: PresentationOwner,
        legacy_lease: Option<DisplayLease>,
        now: Instant,
    ) -> Result<PresentationPlan, DuplicatePresentationId> {
        let snapshot = self.registry.routing_snapshot(now);
        let (resolution, matched) = resolve_snapshot(&snapshot, &request.target);
        let deadline = now.checked_add(self.acknowledgement_timeout).unwrap_or(now);
        let mut outcomes = BTreeMap::new();
        let mut destinations = Vec::new();

        for terminal_id in &resolution.registered_matches {
            let Some(definition) = snapshot.definitions.get(terminal_id) else {
                continue;
            };
            let Some(presence) = snapshot.presences.get(terminal_id) else {
                outcomes.insert(terminal_id.clone(), PresentationDeliveryOutcome::Offline);
                continue;
            };

            let missing = missing_capabilities(
                &request.required_capabilities,
                &definition.approved_capabilities,
                &presence.declared_capabilities,
            );
            if !missing.is_empty() {
                outcomes.insert(
                    terminal_id.clone(),
                    PresentationDeliveryOutcome::Incompatible {
                        missing_capabilities: missing,
                    },
                );
                continue;
            }

            // Registered scope can include offline definitions; every live,
            // compatible match is nevertheless an immediate recipient.
            if matched.contains(terminal_id) {
                outcomes.insert(terminal_id.clone(), PresentationDeliveryOutcome::Pending);
                destinations.push((terminal_id.clone(), presence.connection_id));
            }
        }

        let result = PresentationDeliveryResult {
            presentation_id: request.id,
            requested_target: request.target.clone(),
            resolved_target: resolution.resolved_target.clone(),
            outcomes,
        };
        let (dispatch, recipients, completions) = {
            let mut store = self.lock();
            if store.deliveries.contains_key(&request.id)
                || store.tombstones.contains_key(&request.id)
            {
                return Err(DuplicatePresentationId);
            }

            let mut planned = BTreeMap::new();
            let mut recipients = Vec::new();
            let mut delivery_contexts = BTreeMap::new();
            for (terminal_id, connection_id) in destinations {
                let next = store
                    .generations
                    .get(&terminal_id)
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(1);
                store.generations.insert(terminal_id.clone(), next);
                let generation = PresentationGeneration::new(next);

                for existing in store.deliveries.values_mut() {
                    if matches!(
                        existing.result.outcomes.get(&terminal_id),
                        Some(PresentationDeliveryOutcome::Pending)
                    ) {
                        existing
                            .result
                            .outcomes
                            .insert(terminal_id.clone(), PresentationDeliveryOutcome::Superseded);
                    }
                }

                planned.insert(
                    terminal_id.clone(),
                    PlannedDelivery {
                        connection_id,
                        generation,
                        deadline,
                    },
                );
                recipients.push(PresentationRecipient {
                    terminal_id: terminal_id.clone(),
                    connection_id,
                    generation,
                });
                delivery_contexts.insert(
                    terminal_id,
                    PresentationDeliveryContext {
                        connection_id,
                        generation,
                        valid_for_millis: duration_millis(self.acknowledgement_timeout),
                    },
                );
            }

            let dispatch = resolution.resolved_target.clone().map(|resolved| {
                PresentationDispatch::with_deliveries(request.clone(), resolved, delivery_contexts)
                    .expect("resolution must preserve the requested target and recipients")
            });
            store.deliveries.insert(
                request.id,
                DeliveryRecord {
                    request,
                    owner,
                    result: result.clone(),
                    planned,
                    legacy_lease,
                    completion_emitted: false,
                },
            );
            let completions = settle_completed(&mut store, self.retention);
            (dispatch, recipients, completions)
        };
        for result in completions {
            self.emit_completion(result);
        }

        Ok(PresentationPlan {
            dispatch,
            recipients,
            result,
        })
    }

    /// Deprecated adapter for the complete release-line lease lifecycle. Each
    /// terminal has an independent priority stack; restorations are dispatched
    /// only to terminals whose effective lease changed.
    #[deprecated(
        note = "legacy display events target every online terminal; dispatch an explicit PresentationRequest instead"
    )]
    #[allow(deprecated)]
    pub fn begin_legacy_event(
        &self,
        event: &Event,
        now: Instant,
    ) -> Result<Vec<PresentationPlan>, LegacyPresentationError> {
        let _gate = self
            .legacy_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let snapshot = self.registry.routing_snapshot(now);
        let online = snapshot.presences.keys().cloned().collect::<BTreeSet<_>>();
        let mut actions = Vec::new();

        {
            let mut store = self.lock();
            match &event.kind {
                EventKind::DisplayRequested {
                    command: DisplayCommand::Show { lease, display },
                } => {
                    if store
                        .legacy_leases
                        .values()
                        .flatten()
                        .any(|entry| entry.lease.id == lease.id)
                    {
                        return Err(LegacyPresentationError::DuplicateLease);
                    }
                    for terminal_id in &online {
                        store.legacy_order = store.legacy_order.saturating_add(1);
                        let order = store.legacy_order;
                        let leases = store.legacy_leases.entry(terminal_id.clone()).or_default();
                        let previous = effective_legacy(leases).map(|entry| entry.lease.id);
                        leases.push(LegacyLeaseState {
                            lease: lease.clone(),
                            display: display.clone(),
                            order,
                        });
                        if effective_legacy(leases).map(|entry| entry.lease.id) != previous {
                            actions.push(LegacyDispatch {
                                terminal_id: terminal_id.clone(),
                                display: display.clone(),
                                lease: Some(lease.clone()),
                            });
                        }
                    }
                }
                EventKind::DisplayRequested {
                    command:
                        DisplayCommand::Update {
                            addon_id,
                            lease_id,
                            display,
                        },
                } => {
                    let mut found = false;
                    for (terminal_id, leases) in &mut store.legacy_leases {
                        let effective = effective_legacy(leases).map(|entry| entry.lease.id);
                        if let Some(entry) = leases.iter_mut().find(|entry| {
                            entry.lease.id == *lease_id && entry.lease.owner == *addon_id
                        }) {
                            found = true;
                            entry.display = display.clone();
                            if effective == Some(*lease_id) && online.contains(terminal_id) {
                                actions.push(LegacyDispatch {
                                    terminal_id: terminal_id.clone(),
                                    display: display.clone(),
                                    lease: Some(entry.lease.clone()),
                                });
                            }
                        }
                    }
                    if !found {
                        return Err(LegacyPresentationError::LeaseNotFound);
                    }
                }
                EventKind::DisplayRequested {
                    command: DisplayCommand::Release { addon_id, lease_id },
                } => {
                    let found = release_legacy_leases(
                        &mut store,
                        &online,
                        |entry| entry.lease.id == *lease_id && entry.lease.owner == *addon_id,
                        &mut actions,
                    );
                    if !found {
                        return Err(LegacyPresentationError::LeaseNotFound);
                    }
                }
                EventKind::DisplayRequested {
                    command: DisplayCommand::ReleaseAll { addon_id },
                }
                | EventKind::AddonStopped { addon_id } => {
                    release_legacy_leases(
                        &mut store,
                        &online,
                        |entry| entry.lease.owner == *addon_id,
                        &mut actions,
                    );
                }
                _ => return Ok(Vec::new()),
            }
        }

        let mut plans = Vec::with_capacity(actions.len());
        for action in actions {
            let addon_id = action.lease.as_ref().map(|lease| lease.owner.clone());
            let request = PresentationRequest {
                id: PresentationId::new(),
                target: TerminalTarget::Terminal {
                    id: action.terminal_id,
                    scope: TargetScope::Online,
                },
                required_capabilities: TerminalCapabilities::default(),
                display: action.display,
            };
            plans.push(self.begin_dispatch_owned(
                request,
                PresentationOwner {
                    source: event.source.clone(),
                    addon_id,
                },
                action.lease,
                now,
            )?);
        }
        Ok(plans)
    }

    pub fn acknowledge_accepted(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
        presentation_id: PresentationId,
        now: Instant,
    ) -> AcknowledgementDisposition {
        self.acknowledge(terminal_id, connection_id, presentation_id, None, now)
    }

    pub fn acknowledge_rejected(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
        presentation_id: PresentationId,
        rejection: PresentationRejection,
        now: Instant,
    ) -> AcknowledgementDisposition {
        self.acknowledge(
            terminal_id,
            connection_id,
            presentation_id,
            Some(rejection),
            now,
        )
    }

    fn acknowledge(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
        presentation_id: PresentationId,
        rejection: Option<PresentationRejection>,
        now: Instant,
    ) -> AcknowledgementDisposition {
        let owns_current_presence = self
            .registry
            .refresh_presence(terminal_id, connection_id, now)
            .is_ok();
        let (disposition, completions) = {
            let mut store = self.lock();
            if let Some(tombstone) = store.tombstones.get(&presentation_id) {
                let Some(outcome) = tombstone.outcomes.get(terminal_id) else {
                    return AcknowledgementDisposition::UnexpectedTerminal;
                };
                let Some(planned_connection) = tombstone.planned_connections.get(terminal_id)
                else {
                    return settled_disposition(outcome);
                };
                if planned_connection != &connection_id || !owns_current_presence {
                    return AcknowledgementDisposition::StaleConnection;
                }
                return settled_disposition(outcome);
            }

            let Some(record) = store.deliveries.get_mut(&presentation_id) else {
                return AcknowledgementDisposition::UnknownPresentation;
            };
            let Some(outcome) = record.result.outcomes.get(terminal_id) else {
                return AcknowledgementDisposition::UnexpectedTerminal;
            };
            let Some(planned) = record.planned.get(terminal_id) else {
                return AcknowledgementDisposition::UnexpectedTerminal;
            };
            if planned.connection_id != connection_id || !owns_current_presence {
                return AcknowledgementDisposition::StaleConnection;
            }

            let mut state_update = None;
            let disposition = match outcome {
                PresentationDeliveryOutcome::Pending if planned.deadline <= now => {
                    record
                        .result
                        .outcomes
                        .insert(terminal_id.clone(), PresentationDeliveryOutcome::TimedOut);
                    AcknowledgementDisposition::Late
                }
                PresentationDeliveryOutcome::Pending => {
                    if let Some(rejection) = rejection {
                        record.result.outcomes.insert(
                            terminal_id.clone(),
                            PresentationDeliveryOutcome::Rejected { rejection },
                        );
                        AcknowledgementDisposition::Rejected
                    } else {
                        record
                            .result
                            .outcomes
                            .insert(terminal_id.clone(), PresentationDeliveryOutcome::Accepted);
                        state_update = Some(TerminalPresentationState {
                            presentation_id,
                            generation: planned.generation,
                            display: record.request.display.clone(),
                            owner: record.owner.clone(),
                            legacy_lease: record.legacy_lease.clone(),
                        });
                        AcknowledgementDisposition::Accepted
                    }
                }
                other => settled_disposition(other),
            };

            if let Some(state) = state_update
                && store
                    .states
                    .get(terminal_id)
                    .is_none_or(|current| current.generation <= state.generation)
            {
                if state.legacy_lease.is_none() {
                    store.legacy_leases.remove(terminal_id);
                }
                store.states.insert(terminal_id.clone(), state);
            }
            let completions = settle_completed(&mut store, self.retention);
            (disposition, completions)
        };

        for result in completions {
            self.emit_completion(result);
        }
        disposition
    }

    /// Marks pending sends owned by a disconnected connection. A stale or
    /// foreign disconnect cannot affect a newer connection's deliveries.
    pub fn connection_disconnected(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
    ) -> Vec<PresentationId> {
        let (affected, completed) = {
            let mut store = self.lock();
            let mut affected = Vec::new();
            for (presentation_id, record) in &mut store.deliveries {
                let pending_for_connection = matches!(
                    record.result.outcomes.get(terminal_id),
                    Some(PresentationDeliveryOutcome::Pending)
                ) && record
                    .planned
                    .get(terminal_id)
                    .is_some_and(|delivery| delivery.connection_id == connection_id);
                if !pending_for_connection {
                    continue;
                }
                record.result.outcomes.insert(
                    terminal_id.clone(),
                    PresentationDeliveryOutcome::Disconnected,
                );
                affected.push(*presentation_id);
            }
            let completed = settle_completed(&mut store, self.retention);
            (affected, completed)
        };
        for result in completed {
            self.emit_completion(result);
        }
        affected
    }

    /// Settles deadlines at or before `now` without sleeping or awaiting I/O.
    pub fn expire_acknowledgements(&self, now: Instant) -> Vec<PresentationId> {
        let (expired_presentations, completed) = {
            let mut store = self.lock();
            let mut expired_presentations = BTreeSet::new();
            for (presentation_id, record) in &mut store.deliveries {
                for (terminal_id, planned) in &record.planned {
                    if planned.deadline <= now
                        && matches!(
                            record.result.outcomes.get(terminal_id),
                            Some(PresentationDeliveryOutcome::Pending)
                        )
                    {
                        record
                            .result
                            .outcomes
                            .insert(terminal_id.clone(), PresentationDeliveryOutcome::TimedOut);
                        expired_presentations.insert(*presentation_id);
                    }
                }
            }
            let completed = settle_completed(&mut store, self.retention);
            (expired_presentations, completed)
        };
        for result in completed {
            self.emit_completion(result);
        }
        expired_presentations.into_iter().collect()
    }

    pub fn delivery_result(
        &self,
        presentation_id: PresentationId,
    ) -> Option<PresentationDeliveryResult> {
        self.lock()
            .deliveries
            .get(&presentation_id)
            .map(|record| record.result.clone())
    }

    pub fn terminal_state(&self, terminal_id: &TerminalId) -> Option<TerminalPresentationState> {
        self.lock().states.get(terminal_id).cloned()
    }

    pub fn terminal_states(&self) -> BTreeMap<TerminalId, TerminalPresentationState> {
        self.lock().states.clone()
    }

    /// Removes semantic state when an administrator forgets the definition.
    pub fn forget_terminal_state(
        &self,
        terminal_id: &TerminalId,
    ) -> Option<TerminalPresentationState> {
        self.lock().states.remove(terminal_id)
    }

    pub fn retained_delivery_count(&self) -> usize {
        self.lock().deliveries.len()
    }

    pub fn retained_tombstone_count(&self) -> usize {
        self.lock().tombstones.len()
    }

    fn emit_completion(&self, result: PresentationDeliveryResult) {
        let _ = self.changes.send(TerminalEvent::new(
            TerminalEventKind::PresentationDeliveryCompleted { result },
        ));
    }

    fn lock(&self) -> MutexGuard<'_, PresentationStore> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn settled_disposition(outcome: &PresentationDeliveryOutcome) -> AcknowledgementDisposition {
    match outcome {
        PresentationDeliveryOutcome::Accepted | PresentationDeliveryOutcome::Rejected { .. } => {
            AcknowledgementDisposition::Duplicate
        }
        PresentationDeliveryOutcome::TimedOut
        | PresentationDeliveryOutcome::Disconnected
        | PresentationDeliveryOutcome::Superseded => AcknowledgementDisposition::Late,
        PresentationDeliveryOutcome::Offline
        | PresentationDeliveryOutcome::Incompatible { .. }
        | PresentationDeliveryOutcome::Pending => AcknowledgementDisposition::UnexpectedTerminal,
    }
}

fn settle_completed(
    store: &mut PresentationStore,
    retention: PresentationRetention,
) -> Vec<PresentationDeliveryResult> {
    let completed_ids = store
        .deliveries
        .iter()
        .filter(|(_, record)| record.result.is_complete() && !record.completion_emitted)
        .map(|(presentation_id, _)| *presentation_id)
        .collect::<Vec<_>>();
    let mut completed = Vec::with_capacity(completed_ids.len());
    for presentation_id in completed_ids {
        let record = store
            .deliveries
            .get_mut(&presentation_id)
            .expect("completed delivery remains present while settling");
        record.completion_emitted = true;
        store.completed_order.push_back(presentation_id);
        completed.push(record.result.clone());
    }
    reclaim_completed(store, retention);
    completed
}

fn reclaim_completed(store: &mut PresentationStore, retention: PresentationRetention) {
    while store.completed_order.len() > retention.completed {
        let presentation_id = store
            .completed_order
            .pop_front()
            .expect("completed order length was checked");
        let Some(record) = store.deliveries.remove(&presentation_id) else {
            continue;
        };
        debug_assert!(
            record.result.is_complete(),
            "pending deliveries must not be evicted"
        );
        store.tombstones.insert(
            presentation_id,
            DeliveryTombstone {
                planned_connections: record
                    .planned
                    .into_iter()
                    .map(|(terminal_id, planned)| (terminal_id, planned.connection_id))
                    .collect(),
                outcomes: record.result.outcomes,
            },
        );
        store.tombstone_order.push_back(presentation_id);
    }
    while store.tombstone_order.len() > retention.evicted_tombstones {
        if let Some(presentation_id) = store.tombstone_order.pop_front() {
            store.tombstones.remove(&presentation_id);
        }
    }
}

fn effective_legacy(leases: &[LegacyLeaseState]) -> Option<&LegacyLeaseState> {
    leases
        .iter()
        .max_by_key(|entry| (entry.lease.priority, entry.order))
}

fn release_legacy_leases(
    store: &mut PresentationStore,
    online: &BTreeSet<TerminalId>,
    matches: impl Fn(&LegacyLeaseState) -> bool,
    actions: &mut Vec<LegacyDispatch>,
) -> bool {
    let terminal_ids = store.legacy_leases.keys().cloned().collect::<Vec<_>>();
    let mut found = false;
    let mut empty = Vec::new();
    for terminal_id in terminal_ids {
        let (removed_ids, replacement, effective_removed) = {
            let leases = store
                .legacy_leases
                .get_mut(&terminal_id)
                .expect("terminal ID came from the lease map");
            let previous = effective_legacy(leases).map(|entry| entry.lease.id);
            let removed_ids = leases
                .iter()
                .filter(|entry| matches(entry))
                .map(|entry| entry.lease.id)
                .collect::<Vec<_>>();
            found |= !removed_ids.is_empty();
            leases.retain(|entry| !removed_ids.contains(&entry.lease.id));
            let replacement = effective_legacy(leases).cloned();
            (
                removed_ids.clone(),
                replacement,
                previous.is_some_and(|lease_id| removed_ids.contains(&lease_id)),
            )
        };

        if store
            .legacy_leases
            .get(&terminal_id)
            .is_some_and(Vec::is_empty)
        {
            empty.push(terminal_id.clone());
        }
        if !effective_removed {
            continue;
        }
        if store.states.get(&terminal_id).is_some_and(|state| {
            state
                .legacy_lease
                .as_ref()
                .is_some_and(|lease| removed_ids.contains(&lease.id))
        }) {
            store.states.remove(&terminal_id);
        }
        if online.contains(&terminal_id) {
            actions.push(match replacement {
                Some(entry) => LegacyDispatch {
                    terminal_id,
                    display: entry.display,
                    lease: Some(entry.lease),
                },
                None => LegacyDispatch {
                    terminal_id,
                    display: DisplayState::Blank,
                    lease: None,
                },
            });
        }
    }
    for terminal_id in empty {
        store.legacy_leases.remove(&terminal_id);
    }
    found
}

fn resolve_snapshot(
    snapshot: &TerminalRoutingSnapshot,
    target: &TerminalTarget,
) -> (TargetResolution, BTreeSet<TerminalId>) {
    let registered_matches = snapshot
        .definitions
        .iter()
        .filter(|(terminal_id, definition)| match target {
            TerminalTarget::Terminal { id, .. } => terminal_id == &id,
            TerminalTarget::Group { id, .. } => snapshot
                .groups
                .get(id)
                .is_some_and(|group| group.members.contains(*terminal_id)),
            TerminalTarget::Tags { query, .. } => match query.match_kind {
                TagMatch::All => query.tags.iter().all(|tag| definition.tags.contains(tag)),
                TagMatch::Any => query.tags.iter().any(|tag| definition.tags.contains(tag)),
            },
            TerminalTarget::All { .. } => true,
        })
        .map(|(terminal_id, _)| terminal_id.clone())
        .collect::<BTreeSet<_>>();

    let scoped_matches = match target.scope() {
        TargetScope::Online => registered_matches
            .iter()
            .filter(|terminal_id| snapshot.presences.contains_key(*terminal_id))
            .cloned()
            .collect(),
        TargetScope::Registered => registered_matches.clone(),
    };
    let resolved_target = ResolvedTarget::new(target.clone(), scoped_matches.clone()).ok();
    (
        TargetResolution {
            requested_target: target.clone(),
            registered_matches,
            resolved_target,
        },
        scoped_matches,
    )
}

fn missing_capabilities(
    required: &TerminalCapabilities,
    approved: &TerminalCapabilities,
    declared: &TerminalCapabilities,
) -> TerminalCapabilities {
    TerminalCapabilities::new(
        required
            .iter()
            .filter(|capability| !approved.contains(capability) || !declared.contains(capability))
            .cloned()
            .collect::<Vec<TerminalCapability>>(),
    )
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, sync::Arc};

    use bts_protocol::addons::v1::AddonId;
    use bts_protocol::{
        DisplayCommand, DisplayLease, DisplayLeaseId, Event, GroupId, GroupIdentity, GroupName,
        PresentationDeliveryOutcome, PresentationId, PresentationRejectionCode, ProtocolVersion,
        TagMatch, TagQuery, TerminalCapability, TerminalIdentity, TerminalImplementationId,
        TerminalName, TerminalRegistration, TerminalTag,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::*;

    struct TestRegistry {
        _directory: TempDir,
        registry: TerminalRegistry,
        now: Instant,
    }

    impl TestRegistry {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let registry = TerminalRegistry::load(
                directory.path().join("terminals.json"),
                Duration::from_secs(90),
            )
            .unwrap();
            Self {
                _directory: directory,
                registry,
                now: Instant::now(),
            }
        }

        fn register(
            &self,
            id: &str,
            capabilities: &[&str],
            connection: u128,
        ) -> (TerminalId, TerminalConnectionId) {
            let terminal_id = TerminalId::new(id).unwrap();
            let connection_id = TerminalConnectionId::from_uuid(Uuid::from_u128(connection));
            self.registry
                .register(
                    TerminalRegistration {
                        identity: TerminalIdentity {
                            id: terminal_id.clone(),
                            name: TerminalName::new(format!("{id} display")).unwrap(),
                        },
                        implementation: TerminalImplementationId::new("test-terminal").unwrap(),
                        protocol_version: ProtocolVersion::CURRENT,
                        capabilities: capabilities_from(capabilities),
                    },
                    connection_id,
                    None,
                    self.now,
                )
                .unwrap();
            (terminal_id, connection_id)
        }
    }

    fn capabilities_from(values: &[&str]) -> TerminalCapabilities {
        TerminalCapabilities::new(
            values
                .iter()
                .map(|value| TerminalCapability::new(*value).unwrap()),
        )
    }

    fn owner(addon: &str) -> PresentationOwner {
        PresentationOwner {
            source: format!("addon:{addon}"),
            addon_id: Some(AddonId::new(addon)),
        }
    }

    fn request(
        id: u128,
        target: TerminalTarget,
        required: &[&str],
        display: DisplayState,
    ) -> PresentationRequest {
        PresentationRequest {
            id: PresentationId::from_uuid(Uuid::from_u128(id)),
            target,
            required_capabilities: capabilities_from(required),
            display,
        }
    }

    fn terminal_target(id: &TerminalId, scope: TargetScope) -> TerminalTarget {
        TerminalTarget::Terminal {
            id: id.clone(),
            scope,
        }
    }

    fn terminal_ids(values: impl IntoIterator<Item = TerminalId>) -> BTreeSet<TerminalId> {
        values.into_iter().collect()
    }

    fn acknowledge_plans(manager: &PresentationManager, plans: &[PresentationPlan], now: Instant) {
        for plan in plans {
            for recipient in &plan.recipients {
                assert_eq!(
                    manager.acknowledge_accepted(
                        &recipient.terminal_id,
                        recipient.connection_id,
                        plan.result.presentation_id,
                        now,
                    ),
                    AcknowledgementDisposition::Accepted
                );
            }
        }
    }

    fn legacy_event(source: &str, command: DisplayCommand) -> Event {
        Event::new(source, EventKind::DisplayRequested { command })
    }

    #[test]
    fn resolves_terminal_group_tags_and_all_with_explicit_scope() {
        let test = TestRegistry::new();
        let (alpha, alpha_connection) = test.register("alpha", &["render_text"], 1);
        let (bravo, _) = test.register("bravo", &["render_text"], 2);
        let (charlie, charlie_connection) = test.register("charlie", &["render_text"], 3);

        test.registry.add_terminal_tag(&alpha, "Bedroom").unwrap();
        test.registry.add_terminal_tag(&alpha, "Private").unwrap();
        test.registry
            .add_terminal_tag(&bravo, "Dining-Room")
            .unwrap();
        test.registry.add_terminal_tag(&charlie, "bedroom").unwrap();
        test.registry.add_terminal_tag(&charlie, "private").unwrap();
        let group_id = GroupId::new("downstairs").unwrap();
        test.registry
            .create_group(GroupIdentity {
                id: group_id.clone(),
                name: GroupName::new("Downstairs").unwrap(),
            })
            .unwrap();
        test.registry.add_group_member(&group_id, &alpha).unwrap();
        test.registry.add_group_member(&group_id, &charlie).unwrap();
        test.registry
            .disconnect(&charlie, charlie_connection)
            .unwrap();

        let manager = PresentationManager::new(test.registry.clone(), Duration::from_secs(5));
        let direct =
            manager.resolve_target(&terminal_target(&alpha, TargetScope::Online), test.now);
        assert_eq!(direct.registered_matches, terminal_ids([alpha.clone()]));
        assert_eq!(
            direct.resolved_target.unwrap().terminals,
            terminal_ids([alpha.clone()])
        );

        let offline =
            manager.resolve_target(&terminal_target(&charlie, TargetScope::Online), test.now);
        assert_eq!(offline.registered_matches, terminal_ids([charlie.clone()]));
        assert!(offline.resolved_target.is_none());
        let registered = manager.resolve_target(
            &terminal_target(&charlie, TargetScope::Registered),
            test.now,
        );
        assert_eq!(
            registered.resolved_target.unwrap().terminals,
            terminal_ids([charlie.clone()])
        );

        let group_online = manager.resolve_target(
            &TerminalTarget::Group {
                id: group_id.clone(),
                scope: TargetScope::Online,
            },
            test.now,
        );
        assert_eq!(
            group_online.registered_matches,
            terminal_ids([alpha.clone(), charlie.clone()])
        );
        assert_eq!(
            group_online.resolved_target.unwrap().terminals,
            terminal_ids([alpha.clone()])
        );
        let group_registered = manager.resolve_target(
            &TerminalTarget::Group {
                id: group_id,
                scope: TargetScope::Registered,
            },
            test.now,
        );
        assert_eq!(
            group_registered.resolved_target.unwrap().terminals,
            terminal_ids([alpha.clone(), charlie.clone()])
        );

        let any = manager.resolve_target(
            &TerminalTarget::Tags {
                query: TagQuery::new(
                    TagMatch::Any,
                    [
                        TerminalTag::new("bedroom").unwrap(),
                        TerminalTag::new("dining-room").unwrap(),
                    ],
                )
                .unwrap(),
                scope: TargetScope::Registered,
            },
            test.now,
        );
        assert_eq!(
            any.resolved_target.unwrap().terminals,
            terminal_ids([alpha.clone(), bravo.clone(), charlie.clone()])
        );
        let all = manager.resolve_target(
            &TerminalTarget::Tags {
                query: TagQuery::new(
                    TagMatch::All,
                    [
                        TerminalTag::new("bedroom").unwrap(),
                        TerminalTag::new("private").unwrap(),
                    ],
                )
                .unwrap(),
                scope: TargetScope::Registered,
            },
            test.now,
        );
        assert_eq!(
            all.resolved_target.unwrap().terminals,
            terminal_ids([alpha.clone(), charlie.clone()])
        );

        let all_online = manager
            .resolve_target(&TerminalTarget::all(), test.now)
            .resolved_target
            .unwrap();
        assert_eq!(all_online.terminals, terminal_ids([alpha, bravo]));
        assert_eq!(
            test.registry.presence(&charlie),
            None,
            "offline definition must not regain presence while resolving"
        );
        assert_eq!(
            test.registry
                .presence(&TerminalId::new("alpha").unwrap())
                .unwrap()
                .connection_id,
            alpha_connection
        );
    }

    #[test]
    fn resolution_and_recipient_order_are_stable_and_not_stateful() {
        let test = TestRegistry::new();
        let (zulu, _) = test.register("zulu", &[], 1);
        let (alpha, _) = test.register("alpha", &[], 2);
        let (mike, _) = test.register("mike", &[], 3);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));

        let before = manager.terminal_states();
        let resolution = manager.resolve_target(&TerminalTarget::all(), test.now);
        let plan = manager
            .begin_dispatch(
                request(1, TerminalTarget::all(), &[], DisplayState::Blank),
                owner("clock"),
                test.now,
            )
            .unwrap();
        let ordered = plan
            .recipients
            .iter()
            .map(|recipient| recipient.terminal_id.clone())
            .collect::<Vec<_>>();
        assert_eq!(ordered, vec![alpha, mike, zulu]);
        assert_eq!(resolution.registered_matches.len(), 3);
        assert_eq!(before, manager.terminal_states());
    }

    #[test]
    fn no_match_offline_match_and_capability_mismatch_are_distinct() {
        let test = TestRegistry::new();
        let (text, _) = test.register("text", &["render_text"], 1);
        let (offline, offline_connection) = test.register("offline", &["render_images"], 2);
        let (image, image_connection) = test.register("image", &["render_images"], 3);
        test.registry
            .disconnect(&offline, offline_connection)
            .unwrap();
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));

        let missing = manager
            .begin_dispatch(
                request(
                    1,
                    terminal_target(&TerminalId::new("missing").unwrap(), TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        assert!(missing.result.outcomes.is_empty());
        assert!(missing.result.resolved_target.is_none());

        let offline_plan = manager
            .begin_dispatch(
                request(
                    2,
                    terminal_target(&offline, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        assert_eq!(
            offline_plan.result.outcomes.get(&offline),
            Some(&PresentationDeliveryOutcome::Offline)
        );
        assert!(offline_plan.result.resolved_target.is_none());

        let incompatible = manager
            .begin_dispatch(
                request(
                    3,
                    terminal_target(&text, TargetScope::Online),
                    &["render_images", "render_text"],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        assert!(incompatible.recipients.is_empty());
        assert_eq!(
            incompatible
                .result
                .resolved_target
                .as_ref()
                .unwrap()
                .terminals,
            terminal_ids([text.clone()]),
            "capability filtering must not narrow the resolved target"
        );
        let PresentationDeliveryOutcome::Incompatible {
            missing_capabilities,
        } = incompatible.result.outcomes.get(&text).unwrap()
        else {
            panic!("expected incompatible delivery")
        };
        assert_eq!(missing_capabilities, &capabilities_from(&["render_images"]));

        let mixed = manager
            .begin_dispatch(
                request(
                    4,
                    TerminalTarget::all(),
                    &["render_images"],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        assert_eq!(mixed.recipients[0].terminal_id, image);
        assert_eq!(
            mixed.result.outcomes.get(&text),
            Some(&PresentationDeliveryOutcome::Incompatible {
                missing_capabilities: capabilities_from(&["render_images"]),
            })
        );
        assert_eq!(
            mixed.result.outcomes.get(&offline),
            Some(&PresentationDeliveryOutcome::Offline)
        );
        assert_eq!(
            manager.acknowledge_accepted(
                &image,
                image_connection,
                mixed.result.presentation_id,
                test.now,
            ),
            AcknowledgementDisposition::Accepted
        );
        assert_eq!(
            manager
                .delivery_result(mixed.result.presentation_id)
                .unwrap()
                .accepted_terminals(),
            terminal_ids([image])
        );
    }

    #[test]
    fn routing_treats_unexpired_presence_as_online_and_stale_presence_as_offline() {
        let test = TestRegistry::new();
        let (terminal, _) = test.register("alpha", &[], 1);
        let manager = PresentationManager::new(test.registry.clone(), Duration::from_secs(5));

        assert!(
            manager
                .resolve_target(
                    &terminal_target(&terminal, TargetScope::Online),
                    test.now + Duration::from_secs(90),
                )
                .resolved_target
                .is_some(),
            "the exact registry timeout boundary remains healthy"
        );
        let stale = manager
            .begin_dispatch(
                request(
                    1,
                    terminal_target(&terminal, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now + Duration::from_secs(90) + Duration::from_nanos(1),
            )
            .unwrap();
        assert_eq!(
            stale.result.outcomes.get(&terminal),
            Some(&PresentationDeliveryOutcome::Offline)
        );
        assert!(stale.recipients.is_empty());
        assert!(
            test.registry.presence(&terminal).is_some(),
            "pure routing must not mutate the registry while classifying stale presence"
        );
    }

    #[test]
    fn acknowledgements_are_owned_matched_and_idempotently_classified() {
        let test = TestRegistry::new();
        let (alpha, alpha_connection) = test.register("alpha", &[], 1);
        let (bravo, bravo_connection) = test.register("bravo", &[], 2);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));
        let presentation_id = PresentationId::from_uuid(Uuid::from_u128(10));
        manager
            .begin_dispatch(
                request(
                    10,
                    terminal_target(&alpha, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();

        assert_eq!(
            manager.acknowledge_accepted(
                &alpha,
                TerminalConnectionId::from_uuid(Uuid::from_u128(99)),
                presentation_id,
                test.now,
            ),
            AcknowledgementDisposition::StaleConnection
        );
        assert_eq!(
            manager.acknowledge_accepted(&bravo, bravo_connection, presentation_id, test.now,),
            AcknowledgementDisposition::UnexpectedTerminal
        );
        assert_eq!(
            manager.acknowledge_accepted(
                &alpha,
                alpha_connection,
                PresentationId::from_uuid(Uuid::from_u128(999)),
                test.now,
            ),
            AcknowledgementDisposition::UnknownPresentation
        );
        assert_eq!(
            manager.acknowledge_accepted(&alpha, alpha_connection, presentation_id, test.now,),
            AcknowledgementDisposition::Accepted
        );
        assert_eq!(
            manager.acknowledge_accepted(&alpha, alpha_connection, presentation_id, test.now,),
            AcknowledgementDisposition::Duplicate
        );
        assert_eq!(
            manager.terminal_state(&alpha).unwrap().presentation_id,
            presentation_id
        );
    }

    #[test]
    fn rejection_timeout_and_disconnect_never_update_state() {
        let test = TestRegistry::new();
        let (rejected, rejected_connection) = test.register("rejected", &[], 1);
        let (silent, silent_connection) = test.register("silent", &[], 2);
        let (gone, gone_connection) = test.register("gone", &[], 3);
        let timeout = Duration::from_secs(7);
        let manager = PresentationManager::new(test.registry.clone(), timeout);

        let rejected_id = PresentationId::from_uuid(Uuid::from_u128(1));
        manager
            .begin_dispatch(
                request(
                    1,
                    terminal_target(&rejected, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        assert_eq!(
            manager.acknowledge_rejected(
                &rejected,
                rejected_connection,
                rejected_id,
                PresentationRejection {
                    code: PresentationRejectionCode::new("busy").unwrap(),
                    detail: Some("Rendering another screen".to_owned()),
                },
                test.now,
            ),
            AcknowledgementDisposition::Rejected
        );
        assert!(matches!(
            manager
                .delivery_result(rejected_id)
                .unwrap()
                .outcomes
                .get(&rejected),
            Some(PresentationDeliveryOutcome::Rejected { rejection })
                if rejection.code.as_str() == "busy"
        ));

        let silent_id = PresentationId::from_uuid(Uuid::from_u128(2));
        manager
            .begin_dispatch(
                request(
                    2,
                    terminal_target(&silent, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        assert!(
            manager
                .expire_acknowledgements(test.now + timeout - Duration::from_nanos(1))
                .is_empty()
        );
        assert_eq!(
            manager.expire_acknowledgements(test.now + timeout),
            vec![silent_id]
        );
        assert_eq!(
            manager
                .acknowledge_accepted(&silent, silent_connection, silent_id, test.now + timeout,),
            AcknowledgementDisposition::Late
        );

        let direct_late_id = PresentationId::from_uuid(Uuid::from_u128(4));
        manager
            .begin_dispatch(
                request(
                    4,
                    terminal_target(&silent, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        assert_eq!(
            manager.acknowledge_accepted(
                &silent,
                silent_connection,
                direct_late_id,
                test.now + timeout,
            ),
            AcknowledgementDisposition::Late,
            "the acknowledgement path must enforce the deadline even before the expiry tick"
        );
        assert_eq!(
            manager
                .delivery_result(direct_late_id)
                .unwrap()
                .outcomes
                .get(&silent),
            Some(&PresentationDeliveryOutcome::TimedOut)
        );

        let gone_id = PresentationId::from_uuid(Uuid::from_u128(3));
        manager
            .begin_dispatch(
                request(
                    3,
                    terminal_target(&gone, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();
        test.registry.disconnect(&gone, gone_connection).unwrap();
        assert_eq!(
            manager.connection_disconnected(&gone, gone_connection),
            vec![gone_id]
        );
        assert_eq!(
            manager
                .delivery_result(gone_id)
                .unwrap()
                .outcomes
                .get(&gone),
            Some(&PresentationDeliveryOutcome::Disconnected)
        );
        assert_eq!(
            manager.acknowledge_accepted(&gone, gone_connection, gone_id, test.now),
            AcknowledgementDisposition::StaleConnection
        );

        assert!(manager.terminal_state(&rejected).is_none());
        assert!(manager.terminal_state(&silent).is_none());
        assert!(manager.terminal_state(&gone).is_none());
    }

    #[test]
    fn partial_group_delivery_updates_only_accepted_terminals_and_retains_owners() {
        let test = TestRegistry::new();
        let (alpha, alpha_connection) = test.register("alpha", &[], 1);
        let (bravo, bravo_connection) = test.register("bravo", &[], 2);
        let group_id = GroupId::new("rooms").unwrap();
        test.registry
            .create_group(GroupIdentity {
                id: group_id.clone(),
                name: GroupName::new("Rooms").unwrap(),
            })
            .unwrap();
        test.registry.add_group_member(&group_id, &alpha).unwrap();
        test.registry.add_group_member(&group_id, &bravo).unwrap();
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));

        let initial_bravo = request(
            1,
            terminal_target(&bravo, TargetScope::Online),
            &[],
            DisplayState::Message {
                title: "Clock".to_owned(),
                body: "Independent".to_owned(),
            },
        );
        manager
            .begin_dispatch(initial_bravo.clone(), owner("clock"), test.now)
            .unwrap();
        manager.acknowledge_accepted(&bravo, bravo_connection, initial_bravo.id, test.now);

        let group_request = request(
            2,
            TerminalTarget::Group {
                id: group_id,
                scope: TargetScope::Online,
            },
            &[],
            DisplayState::Weather {
                location: "Home".to_owned(),
                temperature: "20 C".to_owned(),
                condition: "Clear".to_owned(),
                details: vec![],
                updated_at: "now".to_owned(),
            },
        );
        let plan = manager
            .begin_dispatch(group_request.clone(), owner("weather"), test.now)
            .unwrap();
        assert_eq!(plan.recipients.len(), 2);
        manager.acknowledge_accepted(&alpha, alpha_connection, group_request.id, test.now);
        manager.acknowledge_rejected(
            &bravo,
            bravo_connection,
            group_request.id,
            PresentationRejection {
                code: PresentationRejectionCode::new("busy").unwrap(),
                detail: None,
            },
            test.now,
        );

        let alpha_state = manager.terminal_state(&alpha).unwrap();
        assert_eq!(alpha_state.presentation_id, group_request.id);
        assert_eq!(alpha_state.owner, owner("weather"));
        let bravo_state = manager.terminal_state(&bravo).unwrap();
        assert_eq!(bravo_state.presentation_id, initial_bravo.id);
        assert_eq!(bravo_state.owner, owner("clock"));
        let result = manager.delivery_result(group_request.id).unwrap();
        assert_eq!(result.accepted_terminals(), terminal_ids([alpha]));
    }

    #[test]
    fn single_target_isolation_and_target_selection_are_pure() {
        let test = TestRegistry::new();
        let (alpha, alpha_connection) = test.register("alpha", &[], 1);
        let (bravo, bravo_connection) = test.register("bravo", &[], 2);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));

        for (id, terminal, connection, addon) in [
            (1, &alpha, alpha_connection, "weather"),
            (2, &bravo, bravo_connection, "clock"),
        ] {
            let request = request(
                id,
                terminal_target(terminal, TargetScope::Online),
                &[],
                DisplayState::Message {
                    title: addon.to_owned(),
                    body: terminal.to_string(),
                },
            );
            manager
                .begin_dispatch(request.clone(), owner(addon), test.now)
                .unwrap();
            manager.acknowledge_accepted(terminal, connection, request.id, test.now);
        }
        let alpha_before_disconnect = manager.terminal_state(&alpha).unwrap();
        manager
            .registry
            .disconnect(&alpha, alpha_connection)
            .unwrap();
        assert_eq!(
            manager.terminal_state(&alpha),
            Some(alpha_before_disconnect),
            "semantic state survives terminal presence loss"
        );
        let before = manager.terminal_states();

        let selected = TerminalTarget::All {
            scope: TargetScope::Registered,
        };
        let _ = manager.resolve_target(&selected, test.now);
        let _ = manager.resolve_target(&terminal_target(&alpha, TargetScope::Online), test.now);
        assert_eq!(manager.terminal_states(), before);
        assert_ne!(
            manager.terminal_state(&alpha).unwrap().display,
            manager.terminal_state(&bravo).unwrap().display
        );
    }

    #[test]
    fn reconnect_race_cannot_acknowledge_an_old_dispatch() {
        let test = TestRegistry::new();
        let (terminal, old_connection) = test.register("alpha", &[], 1);
        let manager = PresentationManager::new(test.registry.clone(), Duration::from_secs(5));
        let presentation_id = PresentationId::from_uuid(Uuid::from_u128(1));
        manager
            .begin_dispatch(
                request(
                    1,
                    terminal_target(&terminal, TargetScope::Online),
                    &[],
                    DisplayState::Blank,
                ),
                owner("test"),
                test.now,
            )
            .unwrap();

        test.registry.disconnect(&terminal, old_connection).unwrap();
        let (_, new_connection) = test.register("alpha", &[], 2);
        assert_eq!(
            manager.acknowledge_accepted(&terminal, old_connection, presentation_id, test.now,),
            AcknowledgementDisposition::StaleConnection
        );
        assert_eq!(
            manager.acknowledge_accepted(&terminal, new_connection, presentation_id, test.now,),
            AcknowledgementDisposition::StaleConnection
        );
        assert_eq!(
            manager.connection_disconnected(&terminal, old_connection),
            vec![presentation_id]
        );
        assert!(manager.terminal_state(&terminal).is_none());
    }

    #[test]
    fn concurrent_acknowledgements_preserve_per_terminal_state() {
        let test = TestRegistry::new();
        let (alpha, alpha_connection) = test.register("alpha", &[], 1);
        let (bravo, bravo_connection) = test.register("bravo", &[], 2);
        let manager = Arc::new(PresentationManager::new(
            test.registry,
            Duration::from_secs(5),
        ));
        let presentation = request(1, TerminalTarget::all(), &[], DisplayState::Blank);
        let plan = manager
            .begin_dispatch(presentation.clone(), owner("test"), test.now)
            .unwrap();
        assert_eq!(
            plan.recipients
                .iter()
                .map(|recipient| recipient.generation.get())
                .collect::<Vec<_>>(),
            vec![1, 1],
            "different terminals own independent generation counters"
        );

        let handles = [
            (alpha.clone(), alpha_connection),
            (bravo.clone(), bravo_connection),
        ]
        .into_iter()
        .map(|(terminal_id, connection_id)| {
            let manager = manager.clone();
            std::thread::spawn(move || {
                manager.acknowledge_accepted(&terminal_id, connection_id, presentation.id, test.now)
            })
        })
        .collect::<Vec<_>>();
        for handle in handles {
            assert_eq!(handle.join().unwrap(), AcknowledgementDisposition::Accepted);
        }
        assert_eq!(
            manager
                .delivery_result(presentation.id)
                .unwrap()
                .accepted_terminals(),
            terminal_ids([alpha, bravo])
        );
    }

    #[test]
    fn same_terminal_generations_supersede_reversed_completions_without_regression() {
        let test = TestRegistry::new();
        let (terminal, connection) = test.register("alpha", &[], 1);
        let timeout = Duration::from_secs(5);
        let manager = PresentationManager::new(test.registry, timeout);
        let first = request(
            1,
            terminal_target(&terminal, TargetScope::Online),
            &[],
            DisplayState::Message {
                title: "First".into(),
                body: "Delayed".into(),
            },
        );
        let second = request(
            2,
            terminal_target(&terminal, TargetScope::Online),
            &[],
            DisplayState::Message {
                title: "Second".into(),
                body: "Current".into(),
            },
        );

        let first_plan = manager
            .begin_dispatch(first.clone(), owner("one"), test.now)
            .unwrap();
        let second_plan = manager
            .begin_dispatch(second.clone(), owner("two"), test.now)
            .unwrap();
        assert_eq!(first_plan.recipients[0].generation.get(), 1);
        assert_eq!(second_plan.recipients[0].generation.get(), 2);
        let context = second_plan
            .dispatch
            .as_ref()
            .unwrap()
            .deliveries
            .get(&terminal)
            .unwrap();
        assert_eq!(context.connection_id, connection);
        assert_eq!(context.generation.get(), 2);
        assert_eq!(context.valid_for_millis, 5_000);
        assert_eq!(
            manager
                .delivery_result(first.id)
                .unwrap()
                .outcomes
                .get(&terminal),
            Some(&PresentationDeliveryOutcome::Superseded)
        );

        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, second.id, test.now),
            AcknowledgementDisposition::Accepted
        );
        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, first.id, test.now),
            AcknowledgementDisposition::Late
        );
        let state = manager.terminal_state(&terminal).unwrap();
        assert_eq!(state.presentation_id, second.id);
        assert_eq!(state.generation.get(), 2);
        assert_eq!(state.owner, owner("two"));
    }

    #[test]
    fn expiry_is_enforced_at_the_boundary_and_new_work_advances_generation() {
        let test = TestRegistry::new();
        let (terminal, connection) = test.register("alpha", &[], 1);
        let timeout = Duration::from_secs(3);
        let manager = PresentationManager::new(test.registry, timeout);
        let expired = request(
            1,
            terminal_target(&terminal, TargetScope::Online),
            &[],
            DisplayState::Blank,
        );
        let expired_plan = manager
            .begin_dispatch(expired.clone(), owner("clock"), test.now)
            .unwrap();
        assert_eq!(expired_plan.recipients[0].generation.get(), 1);
        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, expired.id, test.now + timeout,),
            AcknowledgementDisposition::Late
        );
        assert!(manager.terminal_state(&terminal).is_none());

        let replacement = request(
            2,
            terminal_target(&terminal, TargetScope::Online),
            &[],
            DisplayState::Blank,
        );
        let replacement_plan = manager
            .begin_dispatch(replacement, owner("clock"), test.now + timeout)
            .unwrap();
        assert_eq!(replacement_plan.recipients[0].generation.get(), 2);
    }

    #[test]
    fn sustained_clock_like_delivery_is_bounded_and_preserves_pending_and_current_state() {
        let test = TestRegistry::new();
        let (clock, clock_connection) = test.register("clock", &[], 1);
        let (pending, _) = test.register("pending", &[], 2);
        let manager = PresentationManager::with_retention(
            test.registry,
            Duration::from_secs(5),
            PresentationRetention {
                completed: 3,
                evicted_tombstones: 2,
            },
        );
        let pending_request = request(
            1_000,
            terminal_target(&pending, TargetScope::Online),
            &[],
            DisplayState::Blank,
        );
        manager
            .begin_dispatch(pending_request.clone(), owner("silent"), test.now)
            .unwrap();

        let mut last = None;
        for second in 1..=20 {
            let presentation = request(
                second,
                terminal_target(&clock, TargetScope::Online),
                &[],
                DisplayState::Clock {
                    time: "12:00".into(),
                    seconds: format!("{second:02}"),
                    date: "Today".into(),
                },
            );
            manager
                .begin_dispatch(presentation.clone(), owner("clock"), test.now)
                .unwrap();
            assert_eq!(
                manager.acknowledge_accepted(&clock, clock_connection, presentation.id, test.now,),
                AcknowledgementDisposition::Accepted
            );
            last = Some(presentation.id);
        }

        assert_eq!(manager.retained_delivery_count(), 4);
        assert_eq!(manager.retained_tombstone_count(), 2);
        assert!(matches!(
            manager
                .delivery_result(pending_request.id)
                .unwrap()
                .outcomes
                .get(&pending),
            Some(PresentationDeliveryOutcome::Pending)
        ));
        assert_eq!(
            manager.terminal_state(&clock).unwrap().presentation_id,
            last.unwrap()
        );
    }

    #[test]
    fn evicted_acknowledgements_remain_duplicate_late_or_stale_within_tombstone_window() {
        let test = TestRegistry::new();
        let (terminal, connection) = test.register("alpha", &[], 1);
        let timeout = Duration::from_secs(2);
        let manager = PresentationManager::with_retention(
            test.registry.clone(),
            timeout,
            PresentationRetention {
                completed: 0,
                evicted_tombstones: 4,
            },
        );
        let accepted = request(
            1,
            terminal_target(&terminal, TargetScope::Online),
            &[],
            DisplayState::Blank,
        );
        manager
            .begin_dispatch(accepted.clone(), owner("clock"), test.now)
            .unwrap();
        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, accepted.id, test.now),
            AcknowledgementDisposition::Accepted
        );
        assert!(manager.delivery_result(accepted.id).is_none());
        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, accepted.id, test.now),
            AcknowledgementDisposition::Duplicate
        );
        assert_eq!(
            manager.begin_dispatch(accepted.clone(), owner("clock"), test.now),
            Err(DuplicatePresentationId)
        );

        let expired = request(
            2,
            terminal_target(&terminal, TargetScope::Online),
            &[],
            DisplayState::Blank,
        );
        manager
            .begin_dispatch(expired.clone(), owner("clock"), test.now)
            .unwrap();
        manager.expire_acknowledgements(test.now + timeout);
        assert!(manager.delivery_result(expired.id).is_none());
        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, expired.id, test.now + timeout,),
            AcknowledgementDisposition::Late
        );

        test.registry.disconnect(&terminal, connection).unwrap();
        let (_, replacement_connection) = test.register("alpha", &[], 9);
        assert_eq!(
            manager.acknowledge_accepted(&terminal, replacement_connection, accepted.id, test.now,),
            AcknowledgementDisposition::StaleConnection
        );
        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, accepted.id, test.now),
            AcknowledgementDisposition::StaleConnection
        );
        assert_eq!(
            manager.terminal_state(&terminal).unwrap().presentation_id,
            accepted.id
        );
    }

    #[test]
    fn acknowledgement_is_unknown_and_identifier_reusable_after_tombstone_reclamation() {
        let test = TestRegistry::new();
        let (terminal, connection) = test.register("alpha", &[], 1);
        let manager = PresentationManager::with_retention(
            test.registry,
            Duration::from_secs(5),
            PresentationRetention {
                completed: 0,
                evicted_tombstones: 1,
            },
        );
        let first = request(
            1,
            terminal_target(&terminal, TargetScope::Online),
            &[],
            DisplayState::Blank,
        );
        for presentation in [
            first.clone(),
            request(
                2,
                terminal_target(&terminal, TargetScope::Online),
                &[],
                DisplayState::Blank,
            ),
        ] {
            manager
                .begin_dispatch(presentation.clone(), owner("clock"), test.now)
                .unwrap();
            manager.acknowledge_accepted(&terminal, connection, presentation.id, test.now);
        }
        assert_eq!(
            manager.acknowledge_accepted(&terminal, connection, first.id, test.now),
            AcknowledgementDisposition::UnknownPresentation
        );
        assert!(
            manager
                .begin_dispatch(first, owner("clock"), test.now)
                .is_ok()
        );
    }

    #[test]
    #[allow(deprecated)]
    fn legacy_priority_restoration_and_mixed_targeted_flow_are_terminal_specific() {
        let test = TestRegistry::new();
        let (alpha, alpha_connection) = test.register("alpha", &[], 1);
        let (bravo, _) = test.register("bravo", &[], 2);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));
        let addon = AddonId::new("legacy");
        let low_id = DisplayLeaseId::new();
        let high_id = DisplayLeaseId::new();
        let low = DisplayState::Message {
            title: "Low".into(),
            body: "Original".into(),
        };
        let low_show = legacy_event(
            "legacy-source",
            DisplayCommand::Show {
                lease: DisplayLease {
                    id: low_id,
                    owner: addon.clone(),
                    priority: 10,
                },
                display: low,
            },
        );
        let plans = manager.begin_legacy_event(&low_show, test.now).unwrap();
        assert_eq!(plans.len(), 2);
        acknowledge_plans(&manager, &plans, test.now);

        let targeted = request(
            500,
            terminal_target(&alpha, TargetScope::Online),
            &[],
            DisplayState::Message {
                title: "Targeted".into(),
                body: "Independent".into(),
            },
        );
        manager
            .begin_dispatch(targeted.clone(), owner("targeted"), test.now)
            .unwrap();
        manager.acknowledge_accepted(&alpha, alpha_connection, targeted.id, test.now);

        let high_show = legacy_event(
            "legacy-source",
            DisplayCommand::Show {
                lease: DisplayLease {
                    id: high_id,
                    owner: addon.clone(),
                    priority: 20,
                },
                display: DisplayState::Message {
                    title: "High".into(),
                    body: "Temporary".into(),
                },
            },
        );
        acknowledge_plans(
            &manager,
            &manager.begin_legacy_event(&high_show, test.now).unwrap(),
            test.now,
        );

        let updated_low = DisplayState::Message {
            title: "Low".into(),
            body: "Updated while hidden".into(),
        };
        let update = legacy_event(
            "legacy-source",
            DisplayCommand::Update {
                addon_id: addon.clone(),
                lease_id: low_id,
                display: updated_low.clone(),
            },
        );
        assert!(
            manager
                .begin_legacy_event(&update, test.now)
                .unwrap()
                .is_empty()
        );

        let release_high = legacy_event(
            "legacy-source",
            DisplayCommand::Release {
                addon_id: addon.clone(),
                lease_id: high_id,
            },
        );
        let restoration = manager.begin_legacy_event(&release_high, test.now).unwrap();
        assert_eq!(restoration.len(), 2);
        acknowledge_plans(&manager, &restoration, test.now);
        assert_eq!(
            manager.terminal_state(&alpha).unwrap().display,
            DisplayState::Blank
        );
        let bravo_state = manager.terminal_state(&bravo).unwrap();
        assert_eq!(bravo_state.display, updated_low);
        assert_eq!(bravo_state.legacy_lease.unwrap().id, low_id);

        let foreign_release = legacy_event(
            "foreign-source",
            DisplayCommand::Release {
                addon_id: AddonId::new("foreign"),
                lease_id: low_id,
            },
        );
        assert_eq!(
            manager.begin_legacy_event(&foreign_release, test.now),
            Err(LegacyPresentationError::LeaseNotFound)
        );
        let release_low = legacy_event(
            "legacy-source",
            DisplayCommand::Release {
                addon_id: addon,
                lease_id: low_id,
            },
        );
        let released = manager.begin_legacy_event(&release_low, test.now).unwrap();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].recipients[0].terminal_id, bravo);
        acknowledge_plans(&manager, &released, test.now);
    }

    #[test]
    #[allow(deprecated)]
    fn legacy_release_all_and_addon_shutdown_restore_then_clear_each_terminal() {
        let test = TestRegistry::new();
        let (terminal, _) = test.register("alpha", &[], 1);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));
        let base_owner = AddonId::new("base");
        let overlay_owner = AddonId::new("overlay");
        for (owner_id, priority, title) in [
            (base_owner.clone(), 10, "Base"),
            (overlay_owner.clone(), 20, "Overlay"),
        ] {
            let show = legacy_event(
                "host",
                DisplayCommand::Show {
                    lease: DisplayLease {
                        id: DisplayLeaseId::new(),
                        owner: owner_id,
                        priority,
                    },
                    display: DisplayState::Message {
                        title: title.into(),
                        body: "Active".into(),
                    },
                },
            );
            acknowledge_plans(
                &manager,
                &manager.begin_legacy_event(&show, test.now).unwrap(),
                test.now,
            );
        }

        let release_overlay = legacy_event(
            "host",
            DisplayCommand::ReleaseAll {
                addon_id: overlay_owner,
            },
        );
        acknowledge_plans(
            &manager,
            &manager
                .begin_legacy_event(&release_overlay, test.now)
                .unwrap(),
            test.now,
        );
        assert!(matches!(
            manager.terminal_state(&terminal).unwrap().display,
            DisplayState::Message { ref title, .. } if title == "Base"
        ));

        let stopped = Event::new(
            "host",
            EventKind::AddonStopped {
                addon_id: base_owner,
            },
        );
        acknowledge_plans(
            &manager,
            &manager.begin_legacy_event(&stopped, test.now).unwrap(),
            test.now,
        );
        let cleared = manager.terminal_state(&terminal).unwrap();
        assert_eq!(cleared.display, DisplayState::Blank);
        assert!(cleared.legacy_lease.is_none());
    }

    #[test]
    #[allow(deprecated)]
    fn legacy_events_are_split_into_terminal_owned_dispatches() {
        let test = TestRegistry::new();
        let (terminal, _) = test.register("alpha", &[], 1);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));
        let addon_id = AddonId::new("legacy-addon");
        let event = Event::new(
            "legacy-source",
            EventKind::DisplayRequested {
                command: DisplayCommand::Show {
                    lease: DisplayLease {
                        id: DisplayLeaseId::new(),
                        owner: addon_id.clone(),
                        priority: 10,
                    },
                    display: DisplayState::Blank,
                },
            },
        );

        let plans = manager.begin_legacy_event(&event, test.now).unwrap();
        let plan = &plans[0];
        assert_eq!(
            plan.result.requested_target,
            terminal_target(&terminal, TargetScope::Online)
        );
        assert_eq!(plan.recipients[0].terminal_id, terminal);
        let record = manager
            .lock()
            .deliveries
            .get(&plan.result.presentation_id)
            .unwrap()
            .clone();
        assert_eq!(
            record.owner,
            PresentationOwner {
                source: "legacy-source".to_owned(),
                addon_id: Some(addon_id),
            }
        );
    }

    #[test]
    fn completion_event_is_emitted_once_after_every_recipient_settles() {
        let test = TestRegistry::new();
        let (alpha, alpha_connection) = test.register("alpha", &[], 1);
        let (bravo, bravo_connection) = test.register("bravo", &[], 2);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));
        let mut changes = manager.subscribe_changes();
        let presentation = request(1, TerminalTarget::all(), &[], DisplayState::Blank);
        manager
            .begin_dispatch(presentation.clone(), owner("test"), test.now)
            .unwrap();

        manager.acknowledge_accepted(&alpha, alpha_connection, presentation.id, test.now);
        assert!(matches!(
            changes.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        manager.acknowledge_accepted(&bravo, bravo_connection, presentation.id, test.now);
        let completion = changes.try_recv().unwrap();
        let TerminalEventKind::PresentationDeliveryCompleted { result } = completion.kind else {
            panic!("expected delivery completion event")
        };
        assert_eq!(result.accepted_terminals(), terminal_ids([alpha, bravo]));
        assert!(matches!(
            changes.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn duplicate_presentation_identifiers_are_rejected_without_replacing_history() {
        let test = TestRegistry::new();
        test.register("alpha", &[], 1);
        let manager = PresentationManager::new(test.registry, Duration::from_secs(5));
        let first = request(1, TerminalTarget::all(), &[], DisplayState::Blank);
        manager
            .begin_dispatch(first.clone(), owner("one"), test.now)
            .unwrap();
        assert_eq!(
            manager.begin_dispatch(first.clone(), owner("two"), test.now),
            Err(DuplicatePresentationId)
        );
        assert_eq!(
            manager.delivery_result(first.id).unwrap().requested_target,
            TerminalTarget::all()
        );
    }
}
