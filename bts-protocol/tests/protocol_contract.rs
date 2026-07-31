use std::collections::{BTreeMap, BTreeSet};

use bts_protocol::addons::v1::{ActionId, AddonId, MenuEntry};
use bts_protocol::{
    BtsState, CoreTerminalMessage, DisplayCommand, DisplayLease, DisplayLeaseId, DisplayState,
    DtmfMenuKey, DtmfMenuKeyError, Event, EventKind, GroupId, GroupIdentity, GroupName,
    PresentationDeliveryContext, PresentationDeliveryOutcome, PresentationDeliveryResult,
    PresentationDispatch, PresentationGeneration, PresentationId, PresentationRejection,
    PresentationRejectionCode, PresentationRequest, ProtocolVersion, RegistrationRejection,
    RegistrationRejectionReason, ReservedDtmfAction, ResolvedTarget, RoutingError, ServerMessage,
    TERMINAL_EVENT_STREAM_VERSION, TagMatch, TagQuery, TargetScope, TerminalCapabilities,
    TerminalCapability, TerminalClientMessage, TerminalConnectionId, TerminalEvent,
    TerminalEventKind, TerminalGroupChange, TerminalId, TerminalIdentity, TerminalImplementationId,
    TerminalMetadataChange, TerminalName, TerminalRegistration, TerminalTag, TerminalTarget,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

fn terminal_id(value: &str) -> TerminalId {
    TerminalId::new(value).unwrap()
}

fn capability(value: &str) -> TerminalCapability {
    TerminalCapability::new(value).unwrap()
}

fn round_trip<T>(value: &T) -> T
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
}

#[test]
fn machine_identifiers_have_one_validated_wire_form() {
    for valid in ["bedroom", "dining-room.display_2", "a1"] {
        let id = terminal_id(valid);
        assert_eq!(id.as_str(), valid);
        assert_eq!(serde_json::to_value(&id).unwrap(), json!(valid));
        assert_eq!(
            serde_json::from_value::<TerminalId>(json!(valid)).unwrap(),
            id
        );
    }

    for invalid in ["", "Bedroom", " dining-room", "room/one", "-room", "room-"] {
        assert!(TerminalId::new(invalid).is_err(), "accepted {invalid:?}");
        assert!(serde_json::from_value::<TerminalId>(json!(invalid)).is_err());
    }
    assert!(TerminalId::new("a".repeat(65)).is_err());
}

#[test]
fn terminal_and_group_names_are_separate_from_stable_identity() {
    let terminal = TerminalIdentity {
        id: terminal_id("bedroom-display"),
        name: TerminalName::new("Bedroom Display").unwrap(),
    };
    let group = GroupIdentity {
        id: GroupId::new("ground-floor").unwrap(),
        name: GroupName::new("Ground Floor").unwrap(),
    };

    assert_eq!(round_trip(&terminal), terminal);
    assert_eq!(round_trip(&group), group);
    assert!(TerminalName::new(" Bedroom Display").is_err());
    assert!(GroupName::new("Ground\nFloor").is_err());
}

#[test]
fn protocol_version_compatibility_is_explicit() {
    assert_eq!(ProtocolVersion::CURRENT, ProtocolVersion::new(0, 3));
    assert!(ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion::new(0, 3)));
    assert!(!ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion::new(0, 2)));
    assert!(!ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion::new(0, 4)));
    assert!(ProtocolVersion::new(1, 2).is_compatible_with(ProtocolVersion::new(1, 8)));

    let rejection = RegistrationRejectionReason::UnsupportedProtocolVersion {
        received: ProtocolVersion::new(0, 2),
        supported: ProtocolVersion::CURRENT,
    };
    assert_eq!(
        serde_json::to_value(rejection).unwrap(),
        json!({
            "reason": "unsupported_protocol_version",
            "received": { "major": 0, "minor": 2 },
            "supported": { "major": 0, "minor": 3 }
        })
    );
}

#[test]
fn capabilities_are_functional_and_unknown_values_survive() {
    let known = capability(TerminalCapability::RENDER_TEXT);
    let future = capability("render_hologram");
    let declared = TerminalCapabilities::new([known.clone(), future.clone()]);

    assert!(declared.contains(&known));
    assert!(declared.contains(&future));
    assert_eq!(round_trip(&declared), declared);
    assert!(declared.supports_all(&TerminalCapabilities::new([future])));
}

#[test]
fn registration_ignores_unknown_optional_fields() {
    let registration: TerminalRegistration = serde_json::from_value(json!({
        "identity": { "id": "bedroom-display", "name": "Bedroom Display" },
        "implementation": "bts-display",
        "protocol_version": { "major": 0, "minor": 3 },
        "capabilities": ["render_text", "future_capability"],
        "future_optional_field": { "diagnostic_only": true }
    }))
    .unwrap();

    assert_eq!(registration.protocol_version, ProtocolVersion::CURRENT);
    assert!(
        registration
            .capabilities
            .contains(&capability("future_capability"))
    );
}

#[test]
fn all_target_variants_round_trip_and_default_to_online() {
    let targets = [
        TerminalTarget::Terminal {
            id: terminal_id("bedroom-display"),
            scope: TargetScope::Online,
        },
        TerminalTarget::Group {
            id: GroupId::new("downstairs").unwrap(),
            scope: TargetScope::Registered,
        },
        TerminalTarget::Tags {
            query: TagQuery::new(
                TagMatch::All,
                [
                    TerminalTag::new("public").unwrap(),
                    TerminalTag::new("screen").unwrap(),
                ],
            )
            .unwrap(),
            scope: TargetScope::Online,
        },
        TerminalTarget::all(),
    ];

    for target in targets {
        assert_eq!(round_trip(&target), target);
    }

    let all: TerminalTarget = serde_json::from_value(json!({ "target": "all" })).unwrap();
    assert_eq!(all, TerminalTarget::all());
    assert_eq!(all.scope(), TargetScope::Online);

    let group: TerminalTarget =
        serde_json::from_value(json!({ "target": "group", "id": "downstairs" })).unwrap();
    assert_eq!(group.scope(), TargetScope::Online);
}

#[test]
fn target_queries_and_resolutions_reject_empty_or_invalid_values() {
    assert!(TagQuery::new(TagMatch::Any, []).is_err());
    assert!(serde_json::from_value::<TagQuery>(json!({ "match": "any", "tags": [] })).is_err());
    assert!(
        serde_json::from_value::<TagQuery>(json!({
            "match": "all",
            "tags": ["Not-lower-case"]
        }))
        .is_err()
    );
    assert!(ResolvedTarget::new(TerminalTarget::all(), []).is_err());
    assert!(
        serde_json::from_value::<ResolvedTarget>(json!({
            "requested": { "target": "all" },
            "terminals": []
        }))
        .is_err()
    );
}

#[test]
fn unresolved_and_resolved_targets_have_distinct_wire_contracts() {
    let requested = TerminalTarget::Group {
        id: GroupId::new("downstairs").unwrap(),
        scope: TargetScope::Online,
    };
    let resolved = ResolvedTarget::new(
        requested.clone(),
        [terminal_id("dining-room"), terminal_id("hall-display")],
    )
    .unwrap();

    let wire = serde_json::to_value(&resolved).unwrap();
    assert_eq!(
        wire["requested"],
        json!({ "target": "group", "id": "downstairs" })
    );
    assert_eq!(round_trip(&resolved), resolved);

    let errors = [
        RoutingError::NoMatches { target: requested },
        RoutingError::OfflineTerminals {
            terminals: BTreeSet::from([terminal_id("bedroom-display")]),
        },
        RoutingError::UnsupportedCapabilities {
            terminals: BTreeSet::from([terminal_id("hall-display")]),
            required: TerminalCapabilities::new([capability("render_video")]),
        },
    ];
    for error in errors {
        assert_eq!(round_trip(&error), error);
    }
}

#[test]
fn lifecycle_and_presentation_messages_round_trip() {
    let terminal_id = terminal_id("bedroom-display");
    let connection_id = TerminalConnectionId::from_uuid(Uuid::nil());
    let registration = TerminalRegistration {
        identity: TerminalIdentity {
            id: terminal_id.clone(),
            name: TerminalName::new("Bedroom Display").unwrap(),
        },
        implementation: TerminalImplementationId::new("bts-display").unwrap(),
        protocol_version: ProtocolVersion::CURRENT,
        capabilities: TerminalCapabilities::new([capability("render_text")]),
    };
    let request = PresentationRequest {
        id: PresentationId::from_uuid(Uuid::nil()),
        target: TerminalTarget::all(),
        required_capabilities: TerminalCapabilities::new([capability("render_text")]),
        display: DisplayState::Blank,
    };
    let resolved = ResolvedTarget::new(TerminalTarget::all(), [terminal_id.clone()]).unwrap();

    let client_messages = [
        TerminalClientMessage::Register { registration },
        TerminalClientMessage::Heartbeat {
            terminal_id: terminal_id.clone(),
            connection_id,
        },
        TerminalClientMessage::Disconnect {
            terminal_id: terminal_id.clone(),
            connection_id,
            reason: Some("Service stopping".to_owned()),
        },
        TerminalClientMessage::PresentationAccepted {
            terminal_id: terminal_id.clone(),
            connection_id,
            presentation_id: request.id,
        },
        TerminalClientMessage::PresentationRejected {
            terminal_id: terminal_id.clone(),
            connection_id,
            presentation_id: request.id,
            rejection: PresentationRejection {
                code: PresentationRejectionCode::new("busy").unwrap(),
                detail: Some("Another presentation is being prepared".to_owned()),
            },
        },
    ];
    for message in client_messages {
        assert_eq!(round_trip(&message), message);
    }

    let dispatch = PresentationDispatch::with_deliveries(
        request,
        resolved,
        BTreeMap::from([(
            terminal_id.clone(),
            PresentationDeliveryContext {
                connection_id,
                generation: PresentationGeneration::new(7),
                valid_for_millis: 10_000,
            },
        )]),
    )
    .unwrap();
    assert_eq!(dispatch.deliveries[&terminal_id].generation.get(), 7);
    let server_messages = [
        CoreTerminalMessage::RegistrationAcknowledged {
            terminal_id: terminal_id.clone(),
            connection_id,
            protocol_version: ProtocolVersion::CURRENT,
            heartbeat_interval_seconds: 30,
        },
        CoreTerminalMessage::RegistrationRejected {
            rejection: RegistrationRejection {
                terminal_id: Some(terminal_id),
                reason: RegistrationRejectionReason::IdentityAlreadyConnected,
            },
        },
        CoreTerminalMessage::HeartbeatAcknowledged { connection_id },
        CoreTerminalMessage::PresentationDispatch {
            presentation: Box::new(dispatch),
        },
    ];
    for message in server_messages {
        assert_eq!(round_trip(&message), message);
    }
}

#[test]
fn presentation_dispatch_must_resolve_its_own_request_target() {
    let request = PresentationRequest {
        id: PresentationId::default(),
        target: TerminalTarget::all(),
        required_capabilities: TerminalCapabilities::default(),
        display: DisplayState::Blank,
    };
    let resolved = ResolvedTarget::new(
        TerminalTarget::Terminal {
            id: terminal_id("bedroom-display"),
            scope: TargetScope::Online,
        },
        [terminal_id("bedroom-display")],
    )
    .unwrap();
    assert!(PresentationDispatch::new(request, resolved).is_err());
}

#[test]
fn bounded_delivery_results_have_stable_terminal_specific_wire_outcomes() {
    let requested = TerminalTarget::all();
    let accepted = terminal_id("alpha");
    let incompatible = terminal_id("bravo");
    let offline = terminal_id("charlie");
    let result = PresentationDeliveryResult {
        presentation_id: PresentationId::from_uuid(Uuid::nil()),
        requested_target: requested.clone(),
        resolved_target: Some(
            ResolvedTarget::new(requested, [accepted.clone(), incompatible.clone()]).unwrap(),
        ),
        outcomes: BTreeMap::from([
            (accepted.clone(), PresentationDeliveryOutcome::Accepted),
            (
                incompatible.clone(),
                PresentationDeliveryOutcome::Incompatible {
                    missing_capabilities: TerminalCapabilities::new([capability(
                        TerminalCapability::RENDER_IMAGES,
                    )]),
                },
            ),
            (offline.clone(), PresentationDeliveryOutcome::Offline),
        ]),
    };

    assert!(result.is_complete());
    assert_eq!(result.accepted_terminals(), BTreeSet::from([accepted]));
    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(wire["presentation_id"], json!(Uuid::nil()));
    assert_eq!(
        wire["outcomes"][incompatible.as_str()],
        json!({
            "outcome": "incompatible",
            "missing_capabilities": ["render_images"]
        })
    );
    assert_eq!(
        wire["outcomes"][offline.as_str()],
        json!({ "outcome": "offline" })
    );
    assert_eq!(round_trip(&result), result);

    let event = TerminalEvent::new(TerminalEventKind::PresentationDeliveryCompleted { result });
    assert_eq!(
        serde_json::to_value(round_trip(&event)).unwrap()["stream_version"],
        json!(TERMINAL_EVENT_STREAM_VERSION)
    );
}

#[test]
fn empty_delivery_outcomes_encode_an_explicit_no_registered_match() {
    let result = PresentationDeliveryResult {
        presentation_id: PresentationId::from_uuid(Uuid::nil()),
        requested_target: TerminalTarget::all(),
        resolved_target: None,
        outcomes: BTreeMap::new(),
    };

    assert!(result.is_complete());
    assert!(result.accepted_terminals().is_empty());
    assert_eq!(round_trip(&result), result);
}

#[test]
fn reserved_dtmf_controls_cannot_be_addon_menu_keys() {
    let reserved = [
        ('0', ReservedDtmfAction::SessionConfiguration),
        ('*', ReservedDtmfAction::CancelOrBack),
        ('#', ReservedDtmfAction::ConfirmOrCompleteInput),
    ];

    for (digit, action) in reserved {
        assert_eq!(action.digit(), digit);
        assert_eq!(ReservedDtmfAction::from_digit(digit), Some(action));
        assert_eq!(
            DtmfMenuKey::new(digit),
            Err(DtmfMenuKeyError::Reserved { digit, action })
        );
        assert!(serde_json::from_value::<DtmfMenuKey>(json!(digit.to_string())).is_err());
    }

    let entry = MenuEntry {
        digit: DtmfMenuKey::new('7').unwrap(),
        prompt: "sound:test".to_owned(),
        action: ActionId::new("test.run"),
        order: 1,
    };
    assert_eq!(serde_json::to_value(&entry).unwrap()["digit"], json!("7"));
    assert_eq!(round_trip(&entry), entry);
}

#[test]
#[allow(deprecated)]
fn legacy_display_wire_format_is_unchanged_and_migrates_to_all_online() {
    let lease_id = Uuid::parse_str("747d7218-5aef-44cd-86fa-a9890c809bc6").unwrap();
    let legacy_json = json!({
        "type": "display_requested",
        "command": {
            "operation": "show",
            "lease": {
                "id": lease_id,
                "owner": "legacy-addon",
                "priority": 10
            },
            "display": { "screen": "blank" }
        }
    });
    let legacy_kind: EventKind = serde_json::from_value(legacy_json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&legacy_kind).unwrap(), legacy_json);

    let event_id = Uuid::parse_str("2f48f2c6-dabc-46c5-a4ea-cbb00c842f3f").unwrap();
    let event = Event {
        id: event_id,
        timestamp: chrono::Utc::now(),
        source: "legacy-addon".to_owned(),
        kind: legacy_kind,
    };
    let migrated = event.legacy_presentation_request().unwrap();
    assert_eq!(migrated.id.as_uuid(), &event_id);
    assert_eq!(migrated.target, TerminalTarget::all());
    assert_eq!(
        migrated.required_capabilities,
        TerminalCapabilities::default()
    );
    assert_eq!(migrated.display, DisplayState::Blank);

    let encoded: Value = serde_json::to_value(&event.kind).unwrap();
    assert!(encoded.get("target").is_none());

    let release = Event::new(
        "legacy-addon",
        EventKind::DisplayRequested {
            command: DisplayCommand::Release {
                addon_id: bts_protocol::addons::v1::AddonId::new("legacy-addon"),
                lease_id: DisplayLeaseId(lease_id),
            },
        },
    );
    assert!(release.legacy_presentation_request().is_none());
}

#[test]
fn existing_menu_entry_wire_shape_is_preserved() {
    let legacy: MenuEntry = serde_json::from_value(json!({
        "digit": "4",
        "prompt": "sound:test",
        "action": "test.run",
        "order": 40
    }))
    .unwrap();
    assert_eq!(legacy.digit.digit(), '4');

    assert!(
        serde_json::from_value::<MenuEntry>(json!({
            "digit": "0",
            "prompt": "sound:test",
            "action": "test.run",
            "order": 40
        }))
        .is_err()
    );
}

#[test]
fn terminal_administration_events_have_stable_wire_shapes() {
    let renamed = TerminalEvent::new(TerminalEventKind::MetadataChanged {
        terminal_id: terminal_id("hall-display"),
        change: TerminalMetadataChange::Renamed {
            name: TerminalName::new("Hallway").unwrap(),
        },
    });
    let member_added = TerminalEvent::new(TerminalEventKind::GroupChanged {
        group_id: GroupId::new("downstairs").unwrap(),
        change: TerminalGroupChange::MemberAdded {
            terminal_id: terminal_id("hall-display"),
        },
    });

    let renamed_wire = serde_json::to_value(&renamed).unwrap();
    assert_eq!(
        renamed_wire,
        json!({
            "stream_version": 1,
            "type": "terminal_metadata_changed",
            "terminal_id": "hall-display",
            "change": { "change": "renamed", "name": "Hallway" }
        })
    );
    let member_wire = serde_json::to_value(&member_added).unwrap();
    assert_eq!(
        member_wire,
        json!({
            "stream_version": 1,
            "type": "terminal_group_changed",
            "group_id": "downstairs",
            "change": { "change": "member_added", "terminal_id": "hall-display" }
        })
    );
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<TerminalEvent>(renamed_wire).unwrap())
            .unwrap()["stream_version"],
        json!(TERMINAL_EVENT_STREAM_VERSION)
    );
}

#[test]
fn preceding_release_event_consumers_never_receive_terminal_delivery_variants() {
    #[allow(dead_code)]
    #[derive(Deserialize)]
    struct PreviousEvent {
        id: Uuid,
        timestamp: chrono::DateTime<chrono::Utc>,
        source: String,
        #[serde(flatten)]
        kind: PreviousEventKind,
    }

    #[allow(dead_code)]
    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum PreviousEventKind {
        DisplayRequested { command: DisplayCommand },
    }

    #[allow(dead_code)]
    #[derive(Deserialize)]
    #[serde(tag = "message", rename_all = "snake_case")]
    enum PreviousServerMessage {
        Snapshot {
            state: BtsState,
        },
        Event {
            event: Box<PreviousEvent>,
            state: BtsState,
        },
    }

    let lease = DisplayLease {
        id: DisplayLeaseId::new(),
        owner: AddonId::new("legacy"),
        priority: 10,
    };
    let message = ServerMessage::Event {
        event: Box::new(Event::new(
            "legacy",
            EventKind::DisplayRequested {
                command: DisplayCommand::Show {
                    lease,
                    display: DisplayState::Blank,
                },
            },
        )),
        state: BtsState::default(),
    };
    let wire = serde_json::to_value(message).unwrap();
    serde_json::from_value::<PreviousServerMessage>(wire)
        .expect("the release-line event stream remains adjacent-version compatible");

    let terminal_wire = serde_json::to_value(TerminalEvent::new(
        TerminalEventKind::PresentationDeliveryCompleted {
            result: PresentationDeliveryResult {
                presentation_id: PresentationId::default(),
                requested_target: TerminalTarget::all(),
                resolved_target: None,
                outcomes: BTreeMap::new(),
            },
        },
    ))
    .unwrap();
    assert!(terminal_wire.get("message").is_none());
    assert_eq!(terminal_wire["stream_version"], json!(1));
}
