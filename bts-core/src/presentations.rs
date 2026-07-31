//! Core-owned target resolution, bounded delivery and per-terminal presentation state.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use bts_protocol::addons::v1::AddonId;
use bts_protocol::{
    DisplayCommand, DisplayState, Event, EventKind, PresentationDeliveryOutcome,
    PresentationDeliveryResult, PresentationDispatch, PresentationId, PresentationRejection,
    PresentationRequest, ResolvedTarget, TagMatch, TargetScope, TerminalCapabilities,
    TerminalCapability, TerminalConnectionId, TerminalId, TerminalTarget,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::terminals::{TerminalRegistry, TerminalRoutingSnapshot};

pub const DEFAULT_ACKNOWLEDGEMENT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_ACKNOWLEDGEMENT_EXPIRY_INTERVAL: Duration = Duration::from_secs(1);

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
    pub display: DisplayState,
    pub owner: PresentationOwner,
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
pub struct DuplicatePresentationId;

impl std::fmt::Display for DuplicatePresentationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("presentation identifier has already been dispatched")
    }
}

impl std::error::Error for DuplicatePresentationId {}

#[derive(Debug, Clone)]
struct PlannedDelivery {
    connection_id: TerminalConnectionId,
    deadline: Instant,
}

#[derive(Debug, Clone)]
struct DeliveryRecord {
    request: PresentationRequest,
    owner: PresentationOwner,
    result: PresentationDeliveryResult,
    planned: BTreeMap<TerminalId, PlannedDelivery>,
    completion_emitted: bool,
}

#[derive(Debug, Default)]
struct PresentationStore {
    deliveries: BTreeMap<PresentationId, DeliveryRecord>,
    states: BTreeMap<TerminalId, TerminalPresentationState>,
}

/// Cloneable, concurrency-safe Core presentation authority.
#[derive(Clone)]
pub struct PresentationManager {
    registry: TerminalRegistry,
    acknowledgement_timeout: Duration,
    store: Arc<Mutex<PresentationStore>>,
    changes: broadcast::Sender<EventKind>,
}

impl PresentationManager {
    pub fn new(registry: TerminalRegistry, acknowledgement_timeout: Duration) -> Self {
        let (changes, _) = broadcast::channel(DELIVERY_CHANNEL_CAPACITY);
        Self {
            registry,
            acknowledgement_timeout,
            store: Arc::new(Mutex::new(PresentationStore::default())),
            changes,
        }
    }

    pub fn subscribe_changes(&self) -> broadcast::Receiver<EventKind> {
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
        let snapshot = self.registry.routing_snapshot(now);
        let (resolution, matched) = resolve_snapshot(&snapshot, &request.target);
        let deadline = now.checked_add(self.acknowledgement_timeout).unwrap_or(now);
        let mut outcomes = BTreeMap::new();
        let mut planned = BTreeMap::new();
        let mut recipients = Vec::new();

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
                planned.insert(
                    terminal_id.clone(),
                    PlannedDelivery {
                        connection_id: presence.connection_id,
                        deadline,
                    },
                );
                recipients.push(PresentationRecipient {
                    terminal_id: terminal_id.clone(),
                    connection_id: presence.connection_id,
                });
            }
        }

        let result = PresentationDeliveryResult {
            presentation_id: request.id,
            requested_target: request.target.clone(),
            resolved_target: resolution.resolved_target.clone(),
            outcomes,
        };
        let dispatch = resolution.resolved_target.clone().map(|resolved| {
            PresentationDispatch::new(request.clone(), resolved)
                .expect("resolution must preserve the requested target")
        });
        let mut record = DeliveryRecord {
            request,
            owner,
            result: result.clone(),
            planned,
            completion_emitted: false,
        };
        let completion = if record.result.is_complete() {
            record.completion_emitted = true;
            Some(record.result.clone())
        } else {
            None
        };

        {
            let mut store = self.lock();
            if store.deliveries.contains_key(&record.request.id) {
                return Err(DuplicatePresentationId);
            }
            store.deliveries.insert(record.request.id, record);
        }
        if let Some(result) = completion {
            self.emit_completion(result);
        }

        Ok(PresentationPlan {
            dispatch,
            recipients,
            result,
        })
    }

    /// Deprecated adapter for the release-line untargeted display event.
    #[deprecated(
        note = "legacy display events target every online terminal; dispatch an explicit PresentationRequest instead"
    )]
    #[allow(deprecated)]
    pub fn begin_legacy_event(
        &self,
        event: &Event,
        now: Instant,
    ) -> Result<Option<PresentationPlan>, DuplicatePresentationId> {
        let Some(request) = event.legacy_presentation_request() else {
            return Ok(None);
        };
        let addon_id = match &event.kind {
            EventKind::DisplayRequested {
                command: DisplayCommand::Show { lease, .. },
            } => Some(lease.owner.clone()),
            EventKind::DisplayRequested {
                command: DisplayCommand::Update { addon_id, .. },
            } => Some(addon_id.clone()),
            _ => None,
        };
        self.begin_dispatch(
            request,
            PresentationOwner {
                source: event.source.clone(),
                addon_id,
            },
            now,
        )
        .map(Some)
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
        // An acknowledgement is authenticated terminal traffic: refresh the
        // current owner while using the registry's ownership check to reject a
        // stale or foreign connection.
        let owns_current_presence = self
            .registry
            .refresh_presence(terminal_id, connection_id, now)
            .is_ok();
        let mut completion = None;
        let disposition;

        {
            let mut store = self.lock();
            let (acknowledgement, state_update) = {
                let Some(record) = store.deliveries.get_mut(&presentation_id) else {
                    return AcknowledgementDisposition::UnknownPresentation;
                };
                let Some(outcome) = record.result.outcomes.get(terminal_id) else {
                    return AcknowledgementDisposition::UnexpectedTerminal;
                };
                match outcome {
                    PresentationDeliveryOutcome::Accepted
                    | PresentationDeliveryOutcome::Rejected { .. } => {
                        (AcknowledgementDisposition::Duplicate, None)
                    }
                    PresentationDeliveryOutcome::TimedOut
                    | PresentationDeliveryOutcome::Disconnected => {
                        (AcknowledgementDisposition::Late, None)
                    }
                    PresentationDeliveryOutcome::Offline
                    | PresentationDeliveryOutcome::Incompatible { .. } => {
                        (AcknowledgementDisposition::UnexpectedTerminal, None)
                    }
                    PresentationDeliveryOutcome::Pending => {
                        let planned = record.planned.get(terminal_id);
                        if planned.map(|delivery| delivery.connection_id) != Some(connection_id)
                            || !owns_current_presence
                        {
                            (AcknowledgementDisposition::StaleConnection, None)
                        } else if planned.is_some_and(|delivery| delivery.deadline <= now) {
                            record
                                .result
                                .outcomes
                                .insert(terminal_id.clone(), PresentationDeliveryOutcome::TimedOut);
                            (AcknowledgementDisposition::Late, None)
                        } else if let Some(rejection) = rejection {
                            record.result.outcomes.insert(
                                terminal_id.clone(),
                                PresentationDeliveryOutcome::Rejected { rejection },
                            );
                            (AcknowledgementDisposition::Rejected, None)
                        } else {
                            record
                                .result
                                .outcomes
                                .insert(terminal_id.clone(), PresentationDeliveryOutcome::Accepted);
                            (
                                AcknowledgementDisposition::Accepted,
                                Some(TerminalPresentationState {
                                    presentation_id,
                                    display: record.request.display.clone(),
                                    owner: record.owner.clone(),
                                }),
                            )
                        }
                    }
                }
            };
            disposition = acknowledgement;
            if let Some(state) = state_update {
                store.states.insert(terminal_id.clone(), state);
            }

            if let Some(record) = store.deliveries.get_mut(&presentation_id)
                && record.result.is_complete()
                && !record.completion_emitted
            {
                record.completion_emitted = true;
                completion = Some(record.result.clone());
            }
        }

        if let Some(result) = completion {
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
        let mut completed = Vec::new();
        let mut affected = Vec::new();
        {
            let mut store = self.lock();
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
                if record.result.is_complete() && !record.completion_emitted {
                    record.completion_emitted = true;
                    completed.push(record.result.clone());
                }
            }
        }
        for result in completed {
            self.emit_completion(result);
        }
        affected
    }

    /// Settles deadlines at or before `now` without sleeping or awaiting I/O.
    pub fn expire_acknowledgements(&self, now: Instant) -> Vec<PresentationId> {
        let mut completed = Vec::new();
        let mut expired_presentations = BTreeSet::new();
        {
            let mut store = self.lock();
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
                if record.result.is_complete() && !record.completion_emitted {
                    record.completion_emitted = true;
                    completed.push(record.result.clone());
                }
            }
        }
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

    fn emit_completion(&self, result: PresentationDeliveryResult) {
        let _ = self
            .changes
            .send(EventKind::PresentationDeliveryCompleted { result });
    }

    fn lock(&self) -> MutexGuard<'_, PresentationStore> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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
            AcknowledgementDisposition::Late
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
        manager
            .begin_dispatch(presentation.clone(), owner("test"), test.now)
            .unwrap();

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
    #[allow(deprecated)]
    fn legacy_untargeted_events_remain_an_explicit_deprecated_all_adapter() {
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

        let plan = manager
            .begin_legacy_event(&event, test.now)
            .unwrap()
            .unwrap();
        assert_eq!(plan.result.requested_target, TerminalTarget::all());
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
        let EventKind::PresentationDeliveryCompleted { result } = changes.try_recv().unwrap()
        else {
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
