use std::collections::BTreeSet;

use bts_protocol::{
    AdministrativeApiCompatibility, AdministrativeError, AdministrativeErrorCategory,
    AdministrativeErrorCode, AdministrativeErrorResponse, AdministrativeResourceKind, ApiDiscovery,
    GroupId, GroupName, GroupResource, MutationResponse, ResourceCandidate, TerminalCapabilities,
    TerminalDescription, TerminalId, TerminalImplementationId, TerminalName, TerminalReference,
    TerminalResource, TerminalTag, UpdateGroupMembersRequest,
    core::{
        CORE_ADMIN_BASE_PATH, CORE_ADMIN_GROUP_MEMBERS_PATH, CORE_ADMIN_GROUP_NAME_PATH,
        CORE_ADMIN_GROUP_PATH, CORE_ADMIN_GROUPS_PATH, CORE_ADMIN_STATE_PATH,
        CORE_ADMIN_STATUS_PATH, CORE_ADMIN_TERMINAL_DESCRIPTION_PATH,
        CORE_ADMIN_TERMINAL_NAME_PATH, CORE_ADMIN_TERMINAL_PATH, CORE_ADMIN_TERMINAL_TAGS_PATH,
        CORE_ADMIN_TERMINALS_PATH, CORE_API_DISCOVERY_PATH, CORE_API_VERSION,
    },
};
use semver::Version;

#[test]
fn administrative_paths_are_resource_oriented_and_share_the_version_source() {
    assert_eq!(CORE_API_DISCOVERY_PATH, "/api");
    assert_eq!(CORE_ADMIN_BASE_PATH, "/api/v1/admin");
    assert_eq!(CORE_ADMIN_STATUS_PATH, "/api/v1/admin/status");
    assert_eq!(CORE_ADMIN_STATE_PATH, "/api/v1/admin/state");
    assert_eq!(CORE_ADMIN_TERMINALS_PATH, "/api/v1/admin/terminals");
    assert_eq!(
        CORE_ADMIN_TERMINAL_PATH,
        "/api/v1/admin/terminals/{terminal}"
    );
    assert_eq!(
        CORE_ADMIN_TERMINAL_NAME_PATH,
        "/api/v1/admin/terminals/{terminal}/name"
    );
    assert_eq!(
        CORE_ADMIN_TERMINAL_DESCRIPTION_PATH,
        "/api/v1/admin/terminals/{terminal}/description"
    );
    assert_eq!(
        CORE_ADMIN_TERMINAL_TAGS_PATH,
        "/api/v1/admin/terminals/{terminal}/tags"
    );
    assert_eq!(CORE_ADMIN_GROUPS_PATH, "/api/v1/admin/groups");
    assert_eq!(CORE_ADMIN_GROUP_PATH, "/api/v1/admin/groups/{group}");
    assert_eq!(
        CORE_ADMIN_GROUP_NAME_PATH,
        "/api/v1/admin/groups/{group}/name"
    );
    assert_eq!(
        CORE_ADMIN_GROUP_MEMBERS_PATH,
        "/api/v1/admin/groups/{group}/members"
    );
    assert_eq!(CORE_API_VERSION, 1);
}

#[test]
fn discovery_and_structured_errors_have_stable_machine_shapes() {
    let discovery = ApiDiscovery {
        product: "bts-core".to_owned(),
        product_version: Version::new(0, 3, 0),
        administrative_api: AdministrativeApiCompatibility {
            current: CORE_API_VERSION,
            supported: BTreeSet::from([CORE_API_VERSION]),
            base_path: CORE_ADMIN_BASE_PATH.to_owned(),
        },
    };
    assert_eq!(
        serde_json::to_value(discovery).unwrap(),
        serde_json::json!({
            "product": "bts-core",
            "product_version": "0.3.0",
            "administrative_api": {
                "current": 1,
                "supported": [1],
                "base_path": "/api/v1/admin"
            }
        })
    );

    let response = AdministrativeErrorResponse {
        error: AdministrativeError {
            category: AdministrativeErrorCategory::AmbiguousReference,
            code: AdministrativeErrorCode::new(
                AdministrativeErrorCode::AMBIGUOUS_TERMINAL_REFERENCE,
            )
            .unwrap(),
            message: "Terminal reference matches more than one name".to_owned(),
            resource: Some(AdministrativeResourceKind::Terminal),
            reference: Some("Kitchen".to_owned()),
            candidates: vec![ResourceCandidate {
                kind: AdministrativeResourceKind::Terminal,
                id: "kitchen-east".to_owned(),
                name: "Kitchen".to_owned(),
            }],
        },
    };
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["error"]["category"], "ambiguous_reference");
    assert_eq!(value["error"]["code"], "ambiguous_terminal_reference");
    assert_eq!(value["error"]["candidates"][0]["id"], "kitchen-east");
    assert_eq!(
        serde_json::from_value::<AdministrativeErrorResponse>(value).unwrap(),
        response
    );
}

#[test]
fn references_are_bounded_raw_id_or_name_values_and_members_are_deterministic() {
    assert!(TerminalReference::new("").is_err());
    assert!(TerminalReference::new(" Bedroom").is_err());
    assert!(TerminalReference::new("Bedroom\n").is_err());
    assert_eq!(
        TerminalReference::new("Bedroom").unwrap().as_str(),
        "Bedroom"
    );
    assert_eq!(
        serde_json::to_value(UpdateGroupMembersRequest {
            add: BTreeSet::from([
                TerminalReference::new("dining-display").unwrap(),
                TerminalReference::new("bedroom-display").unwrap(),
            ]),
            remove: BTreeSet::new(),
        })
        .unwrap(),
        serde_json::json!({
            "add": ["bedroom-display", "dining-display"],
            "remove": []
        })
    );
}

#[test]
fn terminal_and_group_resources_keep_durable_and_ephemeral_state_distinct() {
    let terminal = TerminalResource {
        id: TerminalId::new("bedroom-display").unwrap(),
        name: TerminalName::new("Bedroom").unwrap(),
        description: Some(TerminalDescription::new("Upstairs display").unwrap()),
        implementation: TerminalImplementationId::new("bts-display").unwrap(),
        approved_capabilities: TerminalCapabilities::default(),
        tags: BTreeSet::from([TerminalTag::new("private").unwrap()]),
        groups: BTreeSet::from([GroupId::new("all-displays").unwrap()]),
        first_seen: None,
        last_seen: None,
        presence: None,
        presentation: None,
    };
    let value = serde_json::to_value(MutationResponse {
        changed: false,
        resource: terminal,
    })
    .unwrap();
    assert_eq!(value["changed"], false);
    assert_eq!(value["resource"]["id"], "bedroom-display");
    assert_eq!(value["resource"]["description"], "Upstairs display");
    assert_eq!(value["resource"]["tags"], serde_json::json!(["private"]));
    assert!(value["resource"].get("presence").is_none());
    assert!(value["resource"].get("first_seen").is_none());

    let group = GroupResource {
        id: GroupId::new("all-displays").unwrap(),
        name: GroupName::new("All displays").unwrap(),
        members: BTreeSet::from([
            TerminalId::new("dining-display").unwrap(),
            TerminalId::new("bedroom-display").unwrap(),
        ]),
    };
    assert_eq!(
        serde_json::to_value(group).unwrap()["members"],
        serde_json::json!(["bedroom-display", "dining-display"])
    );
}
