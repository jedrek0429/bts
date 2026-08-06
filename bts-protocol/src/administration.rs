//! Shared wire and domain contracts for Core's administrative API.
//!
//! HTTP transport belongs to `bts-sdk`; command-line policy belongs to
//! `bts-cli`. This module deliberately contains neither.

use std::{collections::BTreeSet, error::Error, fmt};

use chrono::{DateTime, Utc};
use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    BtsState, GroupId, GroupName, IdentifierError, ProtocolVersion, TerminalCapabilities,
    TerminalDescription, TerminalId, TerminalImplementationId, TerminalImplementationVersion,
    TerminalName, TerminalRuntimeDiagnostics, TerminalTag,
};

const MAX_RESOURCE_REFERENCE_LENGTH: usize = 100;

/// Unversioned discovery document returned from `/api`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDiscovery {
    pub product: String,
    pub product_version: Version,
    pub administrative_api: AdministrativeApiCompatibility,
}

/// Exact administrative API versions supported by this Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdministrativeApiCompatibility {
    pub current: u16,
    pub supported: BTreeSet<u16>,
    pub base_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreOperationalStatus {
    Ready,
    Degraded,
}

/// Lightweight process and compatibility status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreStatusResource {
    pub status: CoreOperationalStatus,
    pub product_version: Version,
    pub administrative_api_version: u16,
    pub started_at: DateTime<Utc>,
}

/// Counts associated with the same snapshot as `CoreStateResource::state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStateSummary {
    pub registered: usize,
    pub online: usize,
    pub groups: usize,
}

/// Current Core state plus terminal-registry summary at one observation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreStateResource {
    pub captured_at: DateTime<Utc>,
    pub state: BtsState,
    pub terminals: TerminalStateSummary,
}

macro_rules! resource_reference {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ResourceReferenceError> {
                let value = value.into();
                validate_resource_reference($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

resource_reference!(TerminalReference, "terminal reference");
resource_reference!(GroupReference, "group reference");

fn validate_resource_reference(
    kind: &'static str,
    value: &str,
) -> Result<(), ResourceReferenceError> {
    let count = value.chars().count();
    if (1..=MAX_RESOURCE_REFERENCE_LENGTH).contains(&count)
        && value.trim() == value
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(ResourceReferenceError {
            kind,
            value: value.to_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceReferenceError {
    kind: &'static str,
    value: String,
}

impl fmt::Display for ResourceReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} must be 1-{MAX_RESOURCE_REFERENCE_LENGTH} characters, have no surrounding whitespace and contain no control characters: {:?}",
            self.kind, self.value
        )
    }
}

impl Error for ResourceReferenceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdministrativeResourceKind {
    Terminal,
    Group,
}

/// A possible resource match returned with an ambiguous-reference error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCandidate {
    pub kind: AdministrativeResourceKind,
    pub id: String,
    pub name: String,
}

/// Stable server error categories. Transport and timeout failures are SDK errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdministrativeErrorCategory {
    InvalidInput,
    NotFound,
    AmbiguousReference,
    Conflict,
    Rejected,
    IncompatibleApi,
    ServerFailure,
}

/// Open, validated error code with the v1 codes exposed as constants.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct AdministrativeErrorCode(String);

impl AdministrativeErrorCode {
    pub const INVALID_REQUEST: &'static str = "invalid_request";
    pub const TERMINAL_NOT_FOUND: &'static str = "terminal_not_found";
    pub const GROUP_NOT_FOUND: &'static str = "group_not_found";
    pub const AMBIGUOUS_TERMINAL_REFERENCE: &'static str = "ambiguous_terminal_reference";
    pub const AMBIGUOUS_GROUP_REFERENCE: &'static str = "ambiguous_group_reference";
    pub const TERMINAL_ONLINE: &'static str = "terminal_online";
    pub const GROUP_ALREADY_EXISTS: &'static str = "group_already_exists";
    pub const MUTATION_REJECTED: &'static str = "mutation_rejected";
    pub const UNSUPPORTED_ADMINISTRATIVE_API: &'static str = "unsupported_administrative_api";
    pub const INTERNAL: &'static str = "internal";

    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        crate::terminal::validate_identifier("administrative error code", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AdministrativeErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for AdministrativeErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Machine-readable administrative API failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdministrativeError {
    pub category: AdministrativeErrorCategory,
    pub code: AdministrativeErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<AdministrativeResourceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ResourceCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdministrativeErrorResponse {
    pub error: AdministrativeError,
}

/// Ephemeral information exposed only while the terminal is connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPresenceResource {
    pub connected_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub protocol_version: ProtocolVersion,
    pub declared_capabilities: TerminalCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implementation_version: Option<TerminalImplementationVersion>,
    #[serde(default)]
    pub runtime_diagnostics: TerminalRuntimeDiagnostics,
}

/// Administrative projection of one durable terminal definition and presence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResource {
    pub id: TerminalId,
    pub name: TerminalName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<TerminalDescription>,
    pub implementation: TerminalImplementationId,
    pub approved_capabilities: TerminalCapabilities,
    #[serde(default)]
    pub tags: BTreeSet<TerminalTag>,
    #[serde(default)]
    pub groups: BTreeSet<GroupId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence: Option<TerminalPresenceResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalListResource {
    pub terminals: Vec<TerminalResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupResource {
    pub id: GroupId,
    pub name: GroupName,
    #[serde(default)]
    pub members: BTreeSet<TerminalId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupListResource {
    pub groups: Vec<GroupResource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameTerminalRequest {
    pub name: TerminalName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetTerminalDescriptionRequest {
    pub description: Option<TerminalDescription>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateTerminalTagsRequest {
    #[serde(default)]
    pub add: BTreeSet<TerminalTag>,
    #[serde(default)]
    pub remove: BTreeSet<TerminalTag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateGroupRequest {
    pub id: GroupId,
    pub name: GroupName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameGroupRequest {
    pub name: GroupName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateGroupMembersRequest {
    #[serde(default)]
    pub add: BTreeSet<TerminalReference>,
    #[serde(default)]
    pub remove: BTreeSet<TerminalReference>,
}

/// Result of an idempotent create/update operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationResponse<T> {
    pub changed: bool,
    pub resource: T,
}

/// Result of deleting a terminal or group resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletionResponse<T> {
    pub deleted: T,
}
