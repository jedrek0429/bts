//! Authoritative terminal definitions and ephemeral connection presence.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use anyhow::{Context, Result as AnyResult, bail};
use bts_protocol::{
    GroupId, GroupIdentity, GroupName, ProtocolVersion, RegistrationRejection,
    RegistrationRejectionReason, TerminalCapabilities, TerminalConnectionId, TerminalEvent,
    TerminalEventKind, TerminalGroupChange, TerminalId, TerminalIdentity, TerminalImplementationId,
    TerminalImplementationVersion, TerminalMetadataChange, TerminalName, TerminalRegistration,
    TerminalRuntimeDiagnostics, TerminalTag,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use tempfile::NamedTempFile;
use tokio::sync::broadcast;
use tracing::{info, warn};

pub const DEFAULT_TERMINAL_STATE_PATH: &str = "/var/lib/bts/terminals.json";
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub const DEFAULT_PRESENCE_TIMEOUT: Duration = Duration::from_secs(90);
pub const DEFAULT_EXPIRY_INTERVAL: Duration = Duration::from_secs(30);

const TERMINAL_STATE_SCHEMA_VERSION: u16 = 2;
const CHANGE_CHANNEL_CAPACITY: usize = 128;
const MAX_DESCRIPTION_LENGTH: usize = 500;

/// A durable terminal definition. Group and tag membership is retained here but
/// will be administered by the later terminal-management slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDefinition {
    pub identity: TerminalIdentity,
    #[serde(default)]
    pub description: Option<TerminalDescription>,
    pub implementation: TerminalImplementationId,
    pub approved_capabilities: TerminalCapabilities,
    #[serde(default)]
    pub tags: BTreeSet<TerminalTag>,
    #[serde(default)]
    pub groups: BTreeSet<GroupId>,
    #[serde(default)]
    pub first_seen: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_seen: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_reported_protocol_version: Option<ProtocolVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TerminalDescription(String);

impl TerminalDescription {
    pub fn new(value: impl Into<String>) -> Result<Self, DescriptionError> {
        let value = value.into();
        let characters = value.chars().count();
        if (1..=MAX_DESCRIPTION_LENGTH).contains(&characters)
            && value.trim() == value
            && !value.chars().any(char::is_control)
        {
            Ok(Self(value))
        } else {
            Err(DescriptionError { value })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TerminalDescription {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptionError {
    value: String,
}

impl std::fmt::Display for DescriptionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "terminal description must be 1-{MAX_DESCRIPTION_LENGTH} characters, have no surrounding whitespace and contain no control characters: {:?}",
            self.value
        )
    }
}

impl std::error::Error for DescriptionError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalGroup {
    pub identity: GroupIdentity,
    #[serde(default)]
    pub members: BTreeSet<TerminalId>,
}

/// Volatile ownership of one terminal identity by one live connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalPresence {
    pub connection_id: TerminalConnectionId,
    pub remote_address: Option<SocketAddr>,
    pub connected_at: Instant,
    pub last_seen: Instant,
    pub last_seen_at: DateTime<Utc>,
    pub protocol_version: ProtocolVersion,
    pub declared_capabilities: TerminalCapabilities,
    pub implementation_version: Option<TerminalImplementationVersion>,
    pub runtime_diagnostics: TerminalRuntimeDiagnostics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredTerminalPresence {
    pub terminal_id: TerminalId,
    pub connection_id: TerminalConnectionId,
}

/// One atomic view of definitions, groups and live presence for routing.
///
/// Routing must not assemble these collections through separate registry calls:
/// a registration, disconnect or administrative update between calls could
/// otherwise produce a target which never existed at a single point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalRoutingSnapshot {
    pub definitions: BTreeMap<TerminalId, TerminalDefinition>,
    pub groups: BTreeMap<GroupId, TerminalGroup>,
    pub presences: BTreeMap<TerminalId, TerminalPresence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationKind {
    FirstRegistration,
    Reconnected,
    StalePresenceReplaced {
        previous_connection_id: TerminalConnectionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationOutcome {
    pub kind: RegistrationKind,
    pub definition: TerminalDefinition,
}

#[derive(Debug)]
pub enum RegisterError {
    Rejected(RegistrationRejection),
    Persistence(anyhow::Error),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(rejection) => {
                write!(formatter, "terminal registration rejected: {rejection:?}")
            }
            Self::Persistence(error) => {
                write!(formatter, "could not persist terminal definition: {error}")
            }
        }
    }
}

impl std::error::Error for RegisterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rejected(_) => None,
            Self::Persistence(error) => Some(error.as_ref()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceError {
    Offline,
    NotConnectionOwner,
}

impl std::fmt::Display for PresenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline => formatter.write_str("terminal is offline"),
            Self::NotConnectionOwner => {
                formatter.write_str("connection does not own this terminal presence")
            }
        }
    }
}

impl std::error::Error for PresenceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationOutcome {
    Changed,
    Unchanged,
}

#[derive(Debug)]
pub enum TerminalAdminError {
    TerminalNotFound(TerminalId),
    GroupNotFound(GroupId),
    GroupAlreadyExists(GroupId),
    InvalidTag { value: String, detail: String },
    Persistence(anyhow::Error),
}

impl std::fmt::Display for TerminalAdminError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TerminalNotFound(id) => write!(formatter, "terminal {id} does not exist"),
            Self::GroupNotFound(id) => write!(formatter, "terminal group {id} does not exist"),
            Self::GroupAlreadyExists(id) => {
                write!(formatter, "terminal group {id} already exists")
            }
            Self::InvalidTag { value, detail } => {
                write!(formatter, "invalid terminal tag {value:?}: {detail}")
            }
            Self::Persistence(error) => {
                write!(formatter, "could not persist terminal metadata: {error}")
            }
        }
    }
}

impl std::error::Error for TerminalAdminError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Persistence(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedRegistry {
    schema_version: u16,
    terminals: Vec<TerminalDefinition>,
    #[serde(default)]
    groups: Vec<TerminalGroup>,
}

struct RegistryState {
    path: PathBuf,
    stale_after: Duration,
    definitions: HashMap<TerminalId, TerminalDefinition>,
    groups: HashMap<GroupId, TerminalGroup>,
    presences: HashMap<TerminalId, TerminalPresence>,
}

/// A cloneable, concurrency-safe handle to the Core terminal registry.
#[derive(Clone)]
pub struct TerminalRegistry {
    state: Arc<Mutex<RegistryState>>,
    changes: broadcast::Sender<TerminalEvent>,
}

impl TerminalRegistry {
    pub fn load(path: impl Into<PathBuf>, stale_after: Duration) -> AnyResult<Self> {
        let path = path.into();
        let (definitions, groups, migrated) = load_registry(&path)?;
        if migrated {
            persist_registry(
                &path,
                definitions.values().cloned(),
                groups.values().cloned(),
            )?;
        }
        let (changes, _) = broadcast::channel(CHANGE_CHANNEL_CAPACITY);
        Ok(Self {
            state: Arc::new(Mutex::new(RegistryState {
                path,
                stale_after,
                definitions,
                groups,
                // Presence is deliberately never restored after a Core restart.
                presences: HashMap::new(),
            })),
            changes,
        })
    }

    pub fn subscribe_changes(&self) -> broadcast::Receiver<TerminalEvent> {
        self.changes.subscribe()
    }

    pub fn register(
        &self,
        registration: TerminalRegistration,
        connection_id: TerminalConnectionId,
        remote_address: Option<SocketAddr>,
        now: Instant,
    ) -> Result<RegistrationOutcome, RegisterError> {
        self.register_with_metadata(
            registration,
            connection_id,
            remote_address,
            now,
            None,
            TerminalRuntimeDiagnostics::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_with_metadata(
        &self,
        registration: TerminalRegistration,
        connection_id: TerminalConnectionId,
        remote_address: Option<SocketAddr>,
        now: Instant,
        implementation_version: Option<TerminalImplementationVersion>,
        runtime_diagnostics: TerminalRuntimeDiagnostics,
    ) -> Result<RegistrationOutcome, RegisterError> {
        self.register_observed_with_metadata(
            registration,
            connection_id,
            remote_address,
            now,
            Utc::now(),
            implementation_version,
            runtime_diagnostics,
        )
    }

    pub fn register_observed(
        &self,
        registration: TerminalRegistration,
        connection_id: TerminalConnectionId,
        remote_address: Option<SocketAddr>,
        now: Instant,
        observed_at: DateTime<Utc>,
    ) -> Result<RegistrationOutcome, RegisterError> {
        self.register_observed_with_metadata(
            registration,
            connection_id,
            remote_address,
            now,
            observed_at,
            None,
            TerminalRuntimeDiagnostics::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_observed_with_metadata(
        &self,
        registration: TerminalRegistration,
        connection_id: TerminalConnectionId,
        remote_address: Option<SocketAddr>,
        now: Instant,
        observed_at: DateTime<Utc>,
        implementation_version: Option<TerminalImplementationVersion>,
        runtime_diagnostics: TerminalRuntimeDiagnostics,
    ) -> Result<RegistrationOutcome, RegisterError> {
        let terminal_id = registration.identity.id.clone();
        if !ProtocolVersion::CURRENT.is_compatible_with(registration.protocol_version) {
            warn!(
                terminal_id = %terminal_id,
                received = ?registration.protocol_version,
                supported = ?ProtocolVersion::CURRENT,
                "terminal registration rejected due to incompatible protocol"
            );
            return Err(rejection(
                &terminal_id,
                RegistrationRejectionReason::UnsupportedProtocolVersion {
                    received: registration.protocol_version,
                    supported: ProtocolVersion::CURRENT,
                },
            ));
        }

        let mut state = self.lock();
        if let Some(active) = state.presences.get(&terminal_id)
            && !is_stale(active, now, state.stale_after)
        {
            warn!(
                terminal_id = %terminal_id,
                active_connection_id = ?active.connection_id,
                rejected_connection_id = ?connection_id,
                "duplicate terminal identity rejected"
            );
            return Err(rejection(
                &terminal_id,
                RegistrationRejectionReason::DuplicateTerminalId,
            ));
        }

        let (definition, first_registration) = match state.definitions.get(&terminal_id) {
            Some(definition) => {
                if let Err(reason) = validate_existing_definition(definition, &registration) {
                    warn!(
                        terminal_id = %terminal_id,
                        ?reason,
                        "terminal registration rejected against its existing definition"
                    );
                    return Err(rejection(&terminal_id, reason));
                }
                let mut definitions = state.definitions.clone();
                let updated = definitions
                    .get_mut(&terminal_id)
                    .expect("existing terminal definition must remain present");
                if updated.first_seen.is_none() {
                    updated.first_seen = Some(observed_at);
                }
                updated.last_seen = Some(
                    updated
                        .last_seen
                        .map_or(observed_at, |last_seen| last_seen.max(observed_at)),
                );
                updated.last_reported_protocol_version = Some(registration.protocol_version);
                let definition = updated.clone();
                persist_registry(
                    &state.path,
                    definitions.values().cloned(),
                    state.groups.values().cloned(),
                )
                .map_err(RegisterError::Persistence)?;
                state.definitions = definitions;
                (definition, false)
            }
            None => {
                let definition = TerminalDefinition {
                    identity: registration.identity.clone(),
                    description: None,
                    implementation: registration.implementation.clone(),
                    approved_capabilities: registration.capabilities.clone(),
                    tags: BTreeSet::new(),
                    groups: BTreeSet::new(),
                    first_seen: Some(observed_at),
                    last_seen: Some(observed_at),
                    last_reported_protocol_version: Some(registration.protocol_version),
                };
                let mut definitions = state.definitions.clone();
                definitions.insert(terminal_id.clone(), definition.clone());
                persist_registry(
                    &state.path,
                    definitions.values().cloned(),
                    state.groups.values().cloned(),
                )
                .map_err(RegisterError::Persistence)?;
                state.definitions = definitions;
                (definition, true)
            }
        };

        let replaced = state.presences.remove(&terminal_id);
        state.presences.insert(
            terminal_id.clone(),
            TerminalPresence {
                connection_id,
                remote_address,
                connected_at: now,
                last_seen: now,
                last_seen_at: observed_at,
                protocol_version: registration.protocol_version,
                declared_capabilities: registration.capabilities,
                implementation_version,
                runtime_diagnostics,
            },
        );

        let kind = if first_registration {
            info!(terminal_id = %terminal_id, ?connection_id, "terminal registered and connected");
            RegistrationKind::FirstRegistration
        } else if let Some(previous) = replaced {
            warn!(
                terminal_id = %terminal_id,
                previous_connection_id = ?previous.connection_id,
                new_connection_id = ?connection_id,
                "stale terminal presence replaced"
            );
            RegistrationKind::StalePresenceReplaced {
                previous_connection_id: previous.connection_id,
            }
        } else {
            info!(terminal_id = %terminal_id, ?connection_id, "terminal reconnected");
            RegistrationKind::Reconnected
        };

        Ok(RegistrationOutcome { kind, definition })
    }

    /// Refreshes presence after a heartbeat or other authenticated terminal traffic.
    pub fn refresh_presence(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
        now: Instant,
    ) -> Result<(), PresenceError> {
        self.refresh_presence_observed(terminal_id, connection_id, now, Utc::now())
    }

    pub fn refresh_presence_observed(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
        now: Instant,
        observed_at: DateTime<Utc>,
    ) -> Result<(), PresenceError> {
        let mut state = self.lock();
        let presence = state
            .presences
            .get_mut(terminal_id)
            .ok_or(PresenceError::Offline)?;
        if presence.connection_id != connection_id {
            return Err(PresenceError::NotConnectionOwner);
        }
        presence.last_seen = presence.last_seen.max(now);
        presence.last_seen_at = presence.last_seen_at.max(observed_at);
        let last_seen_at = presence.last_seen_at;
        if let Some(definition) = state.definitions.get_mut(terminal_id) {
            definition.last_seen = Some(last_seen_at);
        }
        Ok(())
    }

    pub fn heartbeat(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
        now: Instant,
    ) -> Result<(), PresenceError> {
        self.refresh_presence(terminal_id, connection_id, now)
    }

    pub fn heartbeat_observed(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
        now: Instant,
        observed_at: DateTime<Utc>,
    ) -> Result<(), PresenceError> {
        self.refresh_presence_observed(terminal_id, connection_id, now, observed_at)
    }

    pub fn disconnect(
        &self,
        terminal_id: &TerminalId,
        connection_id: TerminalConnectionId,
    ) -> Result<(), PresenceError> {
        let mut state = self.lock();
        let presence = state
            .presences
            .get(terminal_id)
            .ok_or(PresenceError::Offline)?;
        if presence.connection_id != connection_id {
            return Err(PresenceError::NotConnectionOwner);
        }
        if let Err(error) = checkpoint_registry(&state) {
            warn!(terminal_id = %terminal_id, %error, "could not checkpoint terminal last-seen metadata");
        }
        state.presences.remove(terminal_id);
        info!(terminal_id = %terminal_id, ?connection_id, "terminal disconnected");
        Ok(())
    }

    /// Removes presences whose last activity is strictly older than the timeout.
    pub fn expire_stale(&self, now: Instant) -> Vec<ExpiredTerminalPresence> {
        let mut state = self.lock();
        let stale_after = state.stale_after;
        let mut expired = state
            .presences
            .iter()
            .filter(|(_, presence)| is_stale(presence, now, stale_after))
            .map(|(terminal_id, presence)| ExpiredTerminalPresence {
                terminal_id: terminal_id.clone(),
                connection_id: presence.connection_id,
            })
            .collect::<Vec<_>>();
        expired.sort_by(|left, right| left.terminal_id.as_str().cmp(right.terminal_id.as_str()));
        if !expired.is_empty()
            && let Err(error) = checkpoint_registry(&state)
        {
            warn!(%error, "could not checkpoint timed-out terminal metadata");
        }
        for expired_presence in &expired {
            if let Some(presence) = state.presences.remove(&expired_presence.terminal_id) {
                warn!(
                    terminal_id = %expired_presence.terminal_id,
                    connection_id = ?presence.connection_id,
                    "terminal presence timed out"
                );
            }
        }
        expired
    }

    pub fn rename_terminal(
        &self,
        terminal_id: &TerminalId,
        name: TerminalName,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let mut state = self.lock();
        let definition = state
            .definitions
            .get(terminal_id)
            .ok_or_else(|| TerminalAdminError::TerminalNotFound(terminal_id.clone()))?;
        if definition.identity.name == name {
            return Ok(MutationOutcome::Unchanged);
        }
        let mut definitions = state.definitions.clone();
        definitions
            .get_mut(terminal_id)
            .expect("existing terminal definition must remain present")
            .identity
            .name = name.clone();
        self.commit_admin_change(
            &mut state,
            definitions,
            None,
            TerminalEventKind::MetadataChanged {
                terminal_id: terminal_id.clone(),
                change: TerminalMetadataChange::Renamed { name },
            },
        )
    }

    pub fn set_terminal_description(
        &self,
        terminal_id: &TerminalId,
        description: Option<TerminalDescription>,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let mut state = self.lock();
        let definition = state
            .definitions
            .get(terminal_id)
            .ok_or_else(|| TerminalAdminError::TerminalNotFound(terminal_id.clone()))?;
        if definition.description == description {
            return Ok(MutationOutcome::Unchanged);
        }
        let mut definitions = state.definitions.clone();
        definitions
            .get_mut(terminal_id)
            .expect("existing terminal definition must remain present")
            .description = description.clone();
        self.commit_admin_change(
            &mut state,
            definitions,
            None,
            TerminalEventKind::MetadataChanged {
                terminal_id: terminal_id.clone(),
                change: TerminalMetadataChange::DescriptionChanged {
                    description: description.map(|value| value.0),
                },
            },
        )
    }

    pub fn add_terminal_tag(
        &self,
        terminal_id: &TerminalId,
        tag: &str,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let tag = normalise_tag(tag)?;
        let mut state = self.lock();
        let definition = state
            .definitions
            .get(terminal_id)
            .ok_or_else(|| TerminalAdminError::TerminalNotFound(terminal_id.clone()))?;
        if definition.tags.contains(&tag) {
            return Ok(MutationOutcome::Unchanged);
        }
        let mut definitions = state.definitions.clone();
        definitions
            .get_mut(terminal_id)
            .expect("existing terminal definition must remain present")
            .tags
            .insert(tag.clone());
        self.commit_admin_change(
            &mut state,
            definitions,
            None,
            TerminalEventKind::MetadataChanged {
                terminal_id: terminal_id.clone(),
                change: TerminalMetadataChange::TagAdded { tag },
            },
        )
    }

    pub fn remove_terminal_tag(
        &self,
        terminal_id: &TerminalId,
        tag: &str,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let tag = normalise_tag(tag)?;
        let mut state = self.lock();
        let definition = state
            .definitions
            .get(terminal_id)
            .ok_or_else(|| TerminalAdminError::TerminalNotFound(terminal_id.clone()))?;
        if !definition.tags.contains(&tag) {
            return Ok(MutationOutcome::Unchanged);
        }
        let mut definitions = state.definitions.clone();
        definitions
            .get_mut(terminal_id)
            .expect("existing terminal definition must remain present")
            .tags
            .remove(&tag);
        self.commit_admin_change(
            &mut state,
            definitions,
            None,
            TerminalEventKind::MetadataChanged {
                terminal_id: terminal_id.clone(),
                change: TerminalMetadataChange::TagRemoved { tag },
            },
        )
    }

    pub fn create_group(
        &self,
        identity: GroupIdentity,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let mut state = self.lock();
        if state.groups.contains_key(&identity.id) {
            return Err(TerminalAdminError::GroupAlreadyExists(identity.id));
        }
        let mut groups = state.groups.clone();
        groups.insert(
            identity.id.clone(),
            TerminalGroup {
                identity: identity.clone(),
                members: BTreeSet::new(),
            },
        );
        let definitions = state.definitions.clone();
        self.commit_admin_change(
            &mut state,
            definitions,
            Some(groups),
            TerminalEventKind::GroupChanged {
                group_id: identity.id,
                change: TerminalGroupChange::Created {
                    name: identity.name,
                },
            },
        )
    }

    pub fn rename_group(
        &self,
        group_id: &GroupId,
        name: GroupName,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let mut state = self.lock();
        let group = state
            .groups
            .get(group_id)
            .ok_or_else(|| TerminalAdminError::GroupNotFound(group_id.clone()))?;
        if group.identity.name == name {
            return Ok(MutationOutcome::Unchanged);
        }
        let mut groups = state.groups.clone();
        groups
            .get_mut(group_id)
            .expect("existing terminal group must remain present")
            .identity
            .name = name.clone();
        let definitions = state.definitions.clone();
        self.commit_admin_change(
            &mut state,
            definitions,
            Some(groups),
            TerminalEventKind::GroupChanged {
                group_id: group_id.clone(),
                change: TerminalGroupChange::Renamed { name },
            },
        )
    }

    pub fn delete_group(&self, group_id: &GroupId) -> Result<MutationOutcome, TerminalAdminError> {
        let mut state = self.lock();
        if !state.groups.contains_key(group_id) {
            return Err(TerminalAdminError::GroupNotFound(group_id.clone()));
        }
        let mut groups = state.groups.clone();
        groups.remove(group_id);
        let mut definitions = state.definitions.clone();
        for definition in definitions.values_mut() {
            definition.groups.remove(group_id);
        }
        self.commit_admin_change(
            &mut state,
            definitions,
            Some(groups),
            TerminalEventKind::GroupChanged {
                group_id: group_id.clone(),
                change: TerminalGroupChange::Deleted,
            },
        )
    }

    pub fn add_group_member(
        &self,
        group_id: &GroupId,
        terminal_id: &TerminalId,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let mut state = self.lock();
        let group = state
            .groups
            .get(group_id)
            .ok_or_else(|| TerminalAdminError::GroupNotFound(group_id.clone()))?;
        if !state.definitions.contains_key(terminal_id) {
            return Err(TerminalAdminError::TerminalNotFound(terminal_id.clone()));
        }
        if group.members.contains(terminal_id) {
            return Ok(MutationOutcome::Unchanged);
        }
        let mut groups = state.groups.clone();
        groups
            .get_mut(group_id)
            .expect("existing terminal group must remain present")
            .members
            .insert(terminal_id.clone());
        let mut definitions = state.definitions.clone();
        definitions
            .get_mut(terminal_id)
            .expect("existing terminal definition must remain present")
            .groups
            .insert(group_id.clone());
        self.commit_admin_change(
            &mut state,
            definitions,
            Some(groups),
            TerminalEventKind::GroupChanged {
                group_id: group_id.clone(),
                change: TerminalGroupChange::MemberAdded {
                    terminal_id: terminal_id.clone(),
                },
            },
        )
    }

    pub fn remove_group_member(
        &self,
        group_id: &GroupId,
        terminal_id: &TerminalId,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let mut state = self.lock();
        let group = state
            .groups
            .get(group_id)
            .ok_or_else(|| TerminalAdminError::GroupNotFound(group_id.clone()))?;
        if !state.definitions.contains_key(terminal_id) {
            return Err(TerminalAdminError::TerminalNotFound(terminal_id.clone()));
        }
        if !group.members.contains(terminal_id) {
            return Ok(MutationOutcome::Unchanged);
        }
        let mut groups = state.groups.clone();
        groups
            .get_mut(group_id)
            .expect("existing terminal group must remain present")
            .members
            .remove(terminal_id);
        let mut definitions = state.definitions.clone();
        definitions
            .get_mut(terminal_id)
            .expect("existing terminal definition must remain present")
            .groups
            .remove(group_id);
        self.commit_admin_change(
            &mut state,
            definitions,
            Some(groups),
            TerminalEventKind::GroupChanged {
                group_id: group_id.clone(),
                change: TerminalGroupChange::MemberRemoved {
                    terminal_id: terminal_id.clone(),
                },
            },
        )
    }

    pub fn definition(&self, terminal_id: &TerminalId) -> Option<TerminalDefinition> {
        self.lock().definitions.get(terminal_id).cloned()
    }

    pub fn definitions(&self) -> Vec<TerminalDefinition> {
        let mut definitions = self
            .lock()
            .definitions
            .values()
            .cloned()
            .collect::<Vec<_>>();
        definitions
            .sort_by(|left, right| left.identity.id.as_str().cmp(right.identity.id.as_str()));
        definitions
    }

    pub fn group(&self, group_id: &GroupId) -> Option<TerminalGroup> {
        self.lock().groups.get(group_id).cloned()
    }

    pub fn groups(&self) -> Vec<TerminalGroup> {
        let mut groups = self.lock().groups.values().cloned().collect::<Vec<_>>();
        groups.sort_by(|left, right| left.identity.id.as_str().cmp(right.identity.id.as_str()));
        groups
    }

    pub fn presence(&self, terminal_id: &TerminalId) -> Option<TerminalPresence> {
        self.lock().presences.get(terminal_id).cloned()
    }

    pub fn routing_snapshot(&self, now: Instant) -> TerminalRoutingSnapshot {
        let state = self.lock();
        TerminalRoutingSnapshot {
            definitions: state
                .definitions
                .iter()
                .map(|(terminal_id, definition)| (terminal_id.clone(), definition.clone()))
                .collect(),
            groups: state
                .groups
                .iter()
                .map(|(group_id, group)| (group_id.clone(), group.clone()))
                .collect(),
            presences: state
                .presences
                .iter()
                .filter(|(_, presence)| !is_stale(presence, now, state.stale_after))
                .map(|(terminal_id, presence)| (terminal_id.clone(), presence.clone()))
                .collect(),
        }
    }

    fn commit_admin_change(
        &self,
        state: &mut RegistryState,
        definitions: HashMap<TerminalId, TerminalDefinition>,
        groups: Option<HashMap<GroupId, TerminalGroup>>,
        event: TerminalEventKind,
    ) -> Result<MutationOutcome, TerminalAdminError> {
        let groups = groups.unwrap_or_else(|| state.groups.clone());
        persist_registry(
            &state.path,
            definitions.values().cloned(),
            groups.values().cloned(),
        )
        .map_err(TerminalAdminError::Persistence)?;
        state.definitions = definitions;
        state.groups = groups;
        let _ = self.changes.send(TerminalEvent::new(event));
        Ok(MutationOutcome::Changed)
    }

    fn lock(&self) -> MutexGuard<'_, RegistryState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub fn configured_state_path() -> PathBuf {
    std::env::var_os("BTS_CORE_TERMINAL_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_TERMINAL_STATE_PATH))
}

fn rejection(terminal_id: &TerminalId, reason: RegistrationRejectionReason) -> RegisterError {
    RegisterError::Rejected(RegistrationRejection {
        terminal_id: Some(terminal_id.clone()),
        reason,
    })
}

fn validate_existing_definition(
    definition: &TerminalDefinition,
    registration: &TerminalRegistration,
) -> Result<(), RegistrationRejectionReason> {
    if definition.implementation != registration.implementation {
        return Err(RegistrationRejectionReason::InvalidRegistration {
            detail: "terminal implementation does not match its registered definition".to_owned(),
        });
    }
    if !definition
        .approved_capabilities
        .supports_all(&registration.capabilities)
    {
        return Err(RegistrationRejectionReason::InvalidRegistration {
            detail: "terminal declared capabilities which have not been approved".to_owned(),
        });
    }
    Ok(())
}

fn is_stale(presence: &TerminalPresence, now: Instant, stale_after: Duration) -> bool {
    now.checked_duration_since(presence.last_seen)
        .is_some_and(|elapsed| elapsed > stale_after)
}

pub fn normalise_tag(value: &str) -> Result<TerminalTag, TerminalAdminError> {
    let normalised = value.trim().to_ascii_lowercase();
    TerminalTag::new(normalised).map_err(|error| TerminalAdminError::InvalidTag {
        value: value.to_owned(),
        detail: error.to_string(),
    })
}

type LoadedRegistry = (
    HashMap<TerminalId, TerminalDefinition>,
    HashMap<GroupId, TerminalGroup>,
    bool,
);

fn load_registry(path: &Path) -> AnyResult<LoadedRegistry> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((HashMap::new(), HashMap::new(), false));
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("could not read terminal registry state {}", path.display())
            });
        }
    };
    let persisted: PersistedRegistry = serde_json::from_slice(&bytes)
        .with_context(|| format!("terminal registry state {} is malformed", path.display()))?;
    if !matches!(persisted.schema_version, 1 | TERMINAL_STATE_SCHEMA_VERSION) {
        bail!(
            "unsupported terminal registry schema {} in {}; expected 1 or {}",
            persisted.schema_version,
            path.display(),
            TERMINAL_STATE_SCHEMA_VERSION
        );
    }
    let mut definitions = HashMap::with_capacity(persisted.terminals.len());
    for definition in persisted.terminals {
        let terminal_id = definition.identity.id.clone();
        if definitions
            .insert(terminal_id.clone(), definition)
            .is_some()
        {
            bail!(
                "terminal registry state {} contains duplicate terminal {}",
                path.display(),
                terminal_id
            );
        }
    }
    let mut groups = HashMap::with_capacity(persisted.groups.len());
    for group in persisted.groups {
        let group_id = group.identity.id.clone();
        if groups.insert(group_id.clone(), group).is_some() {
            bail!(
                "terminal registry state {} contains duplicate group {}",
                path.display(),
                group_id
            );
        }
    }

    let migrated = persisted.schema_version == 1;
    if migrated {
        for definition in definitions.values() {
            for group_id in &definition.groups {
                groups
                    .entry(group_id.clone())
                    .or_insert_with(|| TerminalGroup {
                        identity: GroupIdentity {
                            id: group_id.clone(),
                            name: GroupName::new(group_id.as_str())
                                .expect("a valid group identifier is a valid group name"),
                        },
                        members: BTreeSet::new(),
                    })
                    .members
                    .insert(definition.identity.id.clone());
            }
        }
    } else {
        validate_memberships(path, &definitions, &groups)?;
    }
    Ok((definitions, groups, migrated))
}

fn validate_memberships(
    path: &Path,
    definitions: &HashMap<TerminalId, TerminalDefinition>,
    groups: &HashMap<GroupId, TerminalGroup>,
) -> AnyResult<()> {
    let mut memberships = HashMap::<TerminalId, BTreeSet<GroupId>>::new();
    for group in groups.values() {
        for terminal_id in &group.members {
            if !definitions.contains_key(terminal_id) {
                bail!(
                    "terminal registry state {} has group {} referencing missing terminal {}",
                    path.display(),
                    group.identity.id,
                    terminal_id
                );
            }
            memberships
                .entry(terminal_id.clone())
                .or_default()
                .insert(group.identity.id.clone());
        }
    }
    for definition in definitions.values() {
        let expected = memberships
            .remove(&definition.identity.id)
            .unwrap_or_default();
        if definition.groups != expected {
            bail!(
                "terminal registry state {} has inconsistent group membership for terminal {}",
                path.display(),
                definition.identity.id
            );
        }
    }
    Ok(())
}

fn checkpoint_registry(state: &RegistryState) -> AnyResult<()> {
    persist_registry(
        &state.path,
        state.definitions.values().cloned(),
        state.groups.values().cloned(),
    )
}

fn persist_registry(
    path: &Path,
    definitions: impl IntoIterator<Item = TerminalDefinition>,
    groups: impl IntoIterator<Item = TerminalGroup>,
) -> AnyResult<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "could not create terminal registry directory {}",
            parent.display()
        )
    })?;

    let mut terminals = definitions.into_iter().collect::<Vec<_>>();
    terminals.sort_by(|left, right| left.identity.id.as_str().cmp(right.identity.id.as_str()));
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    groups.sort_by(|left, right| left.identity.id.as_str().cmp(right.identity.id.as_str()));
    let persisted = PersistedRegistry {
        schema_version: TERMINAL_STATE_SCHEMA_VERSION,
        terminals,
        groups,
    };

    let mut temporary = NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "could not create temporary terminal registry state in {}",
            parent.display()
        )
    })?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), &persisted)
        .context("could not serialise terminal registry state")?;
    temporary
        .write_all(b"\n")
        .context("could not finish terminal registry state")?;
    temporary
        .as_file()
        .sync_all()
        .context("could not sync temporary terminal registry state")?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| {
            format!(
                "could not atomically replace terminal registry state {}",
                path.display()
            )
        })?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| {
            format!(
                "could not sync terminal registry directory {}",
                parent.display()
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, net::Ipv4Addr, sync::Barrier, thread};

    use bts_protocol::{TerminalCapability, TerminalName};
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(90);

    fn terminal_id(value: &str) -> TerminalId {
        TerminalId::new(value).unwrap()
    }

    fn capabilities(values: &[&str]) -> TerminalCapabilities {
        TerminalCapabilities::new(
            values
                .iter()
                .map(|value| TerminalCapability::new(*value).unwrap()),
        )
    }

    fn registration(id: &str, name: &str) -> TerminalRegistration {
        TerminalRegistration {
            identity: TerminalIdentity {
                id: terminal_id(id),
                name: TerminalName::new(name).unwrap(),
            },
            implementation: TerminalImplementationId::new("bts-display").unwrap(),
            protocol_version: ProtocolVersion::CURRENT,
            capabilities: capabilities(&[TerminalCapability::RENDER_TEXT]),
        }
    }

    fn observed(value: &str) -> DateTime<Utc> {
        value.parse().unwrap()
    }

    fn group_identity(id: &str, name: &str) -> GroupIdentity {
        GroupIdentity {
            id: GroupId::new(id).unwrap(),
            name: GroupName::new(name).unwrap(),
        }
    }

    fn registry() -> (tempfile::TempDir, PathBuf, TerminalRegistry) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("terminals.json");
        let registry = TerminalRegistry::load(&path, TIMEOUT).unwrap();
        (directory, path, registry)
    }

    fn rejected_reason(error: RegisterError) -> RegistrationRejectionReason {
        let RegisterError::Rejected(rejection) = error else {
            panic!("expected registration rejection, got {error}");
        };
        rejection.reason
    }

    #[test]
    fn first_registration_persists_one_definition_and_presence() {
        let (_directory, path, registry) = registry();
        let now = Instant::now();
        let connection_id = TerminalConnectionId::new();
        let remote = SocketAddr::from((Ipv4Addr::LOCALHOST, 40123));

        let outcome = registry
            .register(
                registration("hall-display", "Hall Display"),
                connection_id,
                Some(remote),
                now,
            )
            .unwrap();

        assert_eq!(outcome.kind, RegistrationKind::FirstRegistration);
        assert_eq!(registry.definitions().len(), 1);
        let presence = registry.presence(&terminal_id("hall-display")).unwrap();
        assert_eq!(presence.connection_id, connection_id);
        assert_eq!(presence.remote_address, Some(remote));
        assert!(path.is_file());
    }

    #[test]
    fn reconnect_reuses_definition_and_ignores_suggested_name() {
        let (_directory, _path, registry) = registry();
        let now = Instant::now();
        let first_connection = TerminalConnectionId::new();
        registry
            .register(
                registration("hall-display", "Original Name"),
                first_connection,
                None,
                now,
            )
            .unwrap();
        registry
            .disconnect(&terminal_id("hall-display"), first_connection)
            .unwrap();

        let outcome = registry
            .register(
                registration("hall-display", "Replacement Suggestion"),
                TerminalConnectionId::new(),
                None,
                now + Duration::from_secs(1),
            )
            .unwrap();

        assert_eq!(outcome.kind, RegistrationKind::Reconnected);
        assert_eq!(registry.definitions().len(), 1);
        assert_eq!(outcome.definition.identity.name.as_str(), "Original Name");
    }

    #[test]
    fn healthy_duplicate_is_rejected_and_active_owner_remains() {
        let (_directory, _path, registry) = registry();
        let now = Instant::now();
        let active = TerminalConnectionId::new();
        registry
            .register(
                registration("hall-display", "Hall Display"),
                active,
                None,
                now,
            )
            .unwrap();

        let rejected = registry
            .register(
                registration("hall-display", "Other"),
                TerminalConnectionId::new(),
                None,
                now + TIMEOUT,
            )
            .unwrap_err();

        let reason = rejected_reason(rejected);
        assert_eq!(reason, RegistrationRejectionReason::DuplicateTerminalId);
        assert_eq!(
            serde_json::to_value(reason).unwrap(),
            json!({ "reason": "duplicate_terminal_id" })
        );
        assert_eq!(
            registry
                .presence(&terminal_id("hall-display"))
                .unwrap()
                .connection_id,
            active
        );
    }

    #[test]
    fn only_presence_older_than_timeout_may_be_replaced() {
        let (_directory, _path, registry) = registry();
        let now = Instant::now();
        let old_connection = TerminalConnectionId::new();
        registry
            .register(
                registration("hall-display", "Hall Display"),
                old_connection,
                None,
                now,
            )
            .unwrap();
        let new_connection = TerminalConnectionId::new();

        let outcome = registry
            .register(
                registration("hall-display", "Ignored"),
                new_connection,
                None,
                now + TIMEOUT + Duration::from_nanos(1),
            )
            .unwrap();

        assert_eq!(
            outcome.kind,
            RegistrationKind::StalePresenceReplaced {
                previous_connection_id: old_connection
            }
        );
        assert_eq!(
            registry
                .presence(&terminal_id("hall-display"))
                .unwrap()
                .connection_id,
            new_connection
        );
    }

    #[test]
    fn heartbeat_and_disconnect_require_connection_ownership() {
        let (_directory, _path, registry) = registry();
        let now = Instant::now();
        let owner = TerminalConnectionId::new();
        let stranger = TerminalConnectionId::new();
        let id = terminal_id("hall-display");
        registry
            .register(
                registration("hall-display", "Hall Display"),
                owner,
                None,
                now,
            )
            .unwrap();

        assert_eq!(
            registry.heartbeat(&id, stranger, now + Duration::from_secs(30)),
            Err(PresenceError::NotConnectionOwner)
        );
        assert_eq!(registry.presence(&id).unwrap().last_seen, now);
        assert_eq!(
            registry.disconnect(&id, stranger),
            Err(PresenceError::NotConnectionOwner)
        );
        assert!(registry.presence(&id).is_some());

        registry
            .heartbeat(&id, owner, now + Duration::from_secs(30))
            .unwrap();
        assert_eq!(
            registry.presence(&id).unwrap().last_seen,
            now + Duration::from_secs(30)
        );
        registry.disconnect(&id, owner).unwrap();
        assert!(registry.presence(&id).is_none());
        assert!(registry.definition(&id).is_some());
    }

    #[test]
    fn expiry_is_deterministic_and_keeps_definition() {
        let (_directory, _path, registry) = registry();
        let now = Instant::now();
        let id = terminal_id("hall-display");
        let connection_id = TerminalConnectionId::new();
        registry
            .register(
                registration("hall-display", "Hall Display"),
                connection_id,
                None,
                now,
            )
            .unwrap();

        assert!(registry.expire_stale(now + TIMEOUT).is_empty());
        assert_eq!(
            registry.expire_stale(now + TIMEOUT + Duration::from_nanos(1)),
            vec![ExpiredTerminalPresence {
                terminal_id: id.clone(),
                connection_id,
            }]
        );
        assert!(registry.presence(&id).is_none());
        assert!(registry.definition(&id).is_some());
    }

    #[test]
    fn restart_restores_definitions_but_never_presence() {
        let (_directory, path, registry) = registry();
        let now = Instant::now();
        registry
            .register(
                registration("hall-display", "Hall Display"),
                TerminalConnectionId::new(),
                None,
                now,
            )
            .unwrap();

        let restored = TerminalRegistry::load(path, TIMEOUT).unwrap();
        let id = terminal_id("hall-display");
        assert_eq!(
            restored.definition(&id).unwrap().identity.name.as_str(),
            "Hall Display"
        );
        assert!(restored.presence(&id).is_none());
    }

    #[test]
    fn persisted_tags_and_groups_survive_rewrite() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("terminals.json");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&json!({
                "schema_version": 1,
                "terminals": [{
                    "identity": { "id": "hall-display", "name": "Hall Display" },
                    "implementation": "bts-display",
                    "approved_capabilities": ["render_text"],
                    "tags": ["public"],
                    "groups": ["ground-floor"]
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let registry = TerminalRegistry::load(&path, TIMEOUT).unwrap();
        let migrated: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(migrated["schema_version"], 2);
        assert_eq!(migrated["groups"][0]["identity"]["name"], "ground-floor");
        registry
            .register(
                registration("kitchen-display", "Kitchen Display"),
                TerminalConnectionId::new(),
                None,
                Instant::now(),
            )
            .unwrap();

        let restored = TerminalRegistry::load(path, TIMEOUT).unwrap();
        let definition = restored.definition(&terminal_id("hall-display")).unwrap();
        assert_eq!(
            definition.tags,
            BTreeSet::from([TerminalTag::new("public").unwrap()])
        );
        assert_eq!(
            definition.groups,
            BTreeSet::from([GroupId::new("ground-floor").unwrap()])
        );
    }

    #[test]
    fn administrative_name_description_and_seen_times_persist_across_reconnect() {
        for invalid in [
            "".to_owned(),
            " padded".to_owned(),
            "two\nlines".to_owned(),
            "x".repeat(MAX_DESCRIPTION_LENGTH + 1),
        ] {
            assert!(TerminalDescription::new(invalid).is_err());
        }
        let (_directory, path, registry) = registry();
        let id = terminal_id("hall-display");
        let monotonic = Instant::now();
        let first_seen = observed("2026-07-31T10:00:00Z");
        let heartbeat_seen = observed("2026-07-31T10:01:00Z");
        let reconnected = observed("2026-07-31T11:00:00Z");
        let first_connection = TerminalConnectionId::new();
        registry
            .register_observed(
                registration("hall-display", "Suggested Name"),
                first_connection,
                None,
                monotonic,
                first_seen,
            )
            .unwrap();
        assert_eq!(
            registry
                .rename_terminal(&id, TerminalName::new("Administrator Name").unwrap())
                .unwrap(),
            MutationOutcome::Changed
        );
        assert_eq!(
            registry
                .set_terminal_description(
                    &id,
                    Some(TerminalDescription::new("Mounted beside the entrance").unwrap()),
                )
                .unwrap(),
            MutationOutcome::Changed
        );
        registry
            .heartbeat_observed(
                &id,
                first_connection,
                monotonic + Duration::from_secs(60),
                heartbeat_seen,
            )
            .unwrap();
        registry.disconnect(&id, first_connection).unwrap();
        registry
            .register_observed(
                registration("hall-display", "New Terminal Suggestion"),
                TerminalConnectionId::new(),
                None,
                monotonic + Duration::from_secs(3600),
                reconnected,
            )
            .unwrap();

        let restored = TerminalRegistry::load(path, TIMEOUT).unwrap();
        let definition = restored.definition(&id).unwrap();
        assert_eq!(definition.identity.name.as_str(), "Administrator Name");
        assert_eq!(
            definition
                .description
                .as_ref()
                .map(TerminalDescription::as_str),
            Some("Mounted beside the entrance")
        );
        assert_eq!(definition.first_seen, Some(first_seen));
        assert_eq!(definition.last_seen, Some(reconnected));
        assert_eq!(
            definition.last_reported_protocol_version,
            Some(ProtocolVersion::CURRENT)
        );

        assert_eq!(
            restored.set_terminal_description(&id, None).unwrap(),
            MutationOutcome::Changed
        );
        assert!(restored.definition(&id).unwrap().description.is_none());
    }

    #[test]
    fn tag_normalisation_validation_and_mutations_are_idempotent() {
        let (_directory, _path, registry) = registry();
        let id = terminal_id("hall-display");
        registry
            .register(
                registration("hall-display", "Hall Display"),
                TerminalConnectionId::new(),
                None,
                Instant::now(),
            )
            .unwrap();
        let mut changes = registry.subscribe_changes();

        assert_eq!(
            registry.add_terminal_tag(&id, "  Private  ").unwrap(),
            MutationOutcome::Changed
        );
        assert_eq!(
            registry.add_terminal_tag(&id, "PRIVATE").unwrap(),
            MutationOutcome::Unchanged
        );
        assert_eq!(
            registry.definition(&id).unwrap().tags,
            BTreeSet::from([TerminalTag::new("private").unwrap()])
        );
        assert!(matches!(
            changes.try_recv().unwrap().kind,
            TerminalEventKind::MetadataChanged {
                change: TerminalMetadataChange::TagAdded { tag },
                ..
            } if tag.as_str() == "private"
        ));
        assert!(matches!(
            changes.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        assert_eq!(
            registry.remove_terminal_tag(&id, " private ").unwrap(),
            MutationOutcome::Changed
        );
        assert_eq!(
            registry.remove_terminal_tag(&id, "PRIVATE").unwrap(),
            MutationOutcome::Unchanged
        );
        for invalid in ["", "-private", "dining room", "piętro"] {
            assert!(matches!(
                registry.add_terminal_tag(&id, invalid),
                Err(TerminalAdminError::InvalidTag { .. })
            ));
        }
    }

    #[test]
    fn group_lifecycle_uses_stable_identity_and_preserves_terminals() {
        let (_directory, _path, registry) = registry();
        let terminal_id = terminal_id("hall-display");
        registry
            .register(
                registration("hall-display", "Hall Display"),
                TerminalConnectionId::new(),
                None,
                Instant::now(),
            )
            .unwrap();
        let group_id = GroupId::new("downstairs").unwrap();

        assert_eq!(
            registry
                .create_group(group_identity("downstairs", "Downstairs"))
                .unwrap(),
            MutationOutcome::Changed
        );
        assert!(matches!(
            registry.create_group(group_identity("downstairs", "Duplicate")),
            Err(TerminalAdminError::GroupAlreadyExists(id)) if id == group_id
        ));
        assert_eq!(
            registry
                .rename_group(&group_id, GroupName::new("Ground Floor").unwrap())
                .unwrap(),
            MutationOutcome::Changed
        );
        assert_eq!(
            registry
                .rename_group(&group_id, GroupName::new("Ground Floor").unwrap())
                .unwrap(),
            MutationOutcome::Unchanged
        );
        registry.add_group_member(&group_id, &terminal_id).unwrap();
        registry.delete_group(&group_id).unwrap();

        assert!(registry.group(&group_id).is_none());
        let definition = registry.definition(&terminal_id).unwrap();
        assert!(definition.groups.is_empty());
        assert_eq!(definition.identity.name.as_str(), "Hall Display");
        assert!(matches!(
            registry.delete_group(&group_id),
            Err(TerminalAdminError::GroupNotFound(id)) if id == group_id
        ));
    }

    #[test]
    fn membership_is_idempotent_and_supports_offline_terminals() {
        let (_directory, _path, registry) = registry();
        let terminal_id = terminal_id("hall-display");
        let connection_id = TerminalConnectionId::new();
        registry
            .register(
                registration("hall-display", "Hall Display"),
                connection_id,
                None,
                Instant::now(),
            )
            .unwrap();
        registry.disconnect(&terminal_id, connection_id).unwrap();
        let group_id = GroupId::new("public").unwrap();
        registry
            .create_group(group_identity("public", "Public Displays"))
            .unwrap();

        assert_eq!(
            registry.add_group_member(&group_id, &terminal_id).unwrap(),
            MutationOutcome::Changed
        );
        assert_eq!(
            registry.add_group_member(&group_id, &terminal_id).unwrap(),
            MutationOutcome::Unchanged
        );
        assert!(registry.presence(&terminal_id).is_none());
        assert!(
            registry
                .group(&group_id)
                .unwrap()
                .members
                .contains(&terminal_id)
        );
        assert!(
            registry
                .definition(&terminal_id)
                .unwrap()
                .groups
                .contains(&group_id)
        );
        assert_eq!(
            registry
                .remove_group_member(&group_id, &terminal_id)
                .unwrap(),
            MutationOutcome::Changed
        );
        assert_eq!(
            registry
                .remove_group_member(&group_id, &terminal_id)
                .unwrap(),
            MutationOutcome::Unchanged
        );
    }

    #[test]
    fn administrative_operations_return_typed_missing_resource_errors() {
        let (_directory, _path, registry) = registry();
        let missing_terminal = terminal_id("missing");
        let missing_group = GroupId::new("missing").unwrap();
        assert!(matches!(
            registry.rename_terminal(
                &missing_terminal,
                TerminalName::new("Missing Terminal").unwrap()
            ),
            Err(TerminalAdminError::TerminalNotFound(id)) if id == missing_terminal
        ));
        assert!(matches!(
            registry.rename_group(&missing_group, GroupName::new("Missing Group").unwrap()),
            Err(TerminalAdminError::GroupNotFound(id)) if id == missing_group
        ));

        registry
            .create_group(group_identity("known", "Known Group"))
            .unwrap();
        assert!(matches!(
            registry.add_group_member(&GroupId::new("known").unwrap(), &missing_terminal),
            Err(TerminalAdminError::TerminalNotFound(id)) if id == missing_terminal
        ));
    }

    #[test]
    fn metadata_groups_and_membership_survive_restart() {
        let (_directory, path, registry) = registry();
        let terminal_id = terminal_id("hall-display");
        let group_id = GroupId::new("downstairs").unwrap();
        registry
            .register(
                registration("hall-display", "Hall Display"),
                TerminalConnectionId::new(),
                None,
                Instant::now(),
            )
            .unwrap();
        registry
            .set_terminal_description(
                &terminal_id,
                Some(TerminalDescription::new("Primary hallway screen").unwrap()),
            )
            .unwrap();
        registry
            .add_terminal_tag(&terminal_id, "Downstairs")
            .unwrap();
        registry
            .create_group(group_identity("downstairs", "Downstairs Displays"))
            .unwrap();
        registry.add_group_member(&group_id, &terminal_id).unwrap();

        let restored = TerminalRegistry::load(path, TIMEOUT).unwrap();
        let definition = restored.definition(&terminal_id).unwrap();
        assert_eq!(
            definition
                .description
                .as_ref()
                .map(TerminalDescription::as_str),
            Some("Primary hallway screen")
        );
        assert!(
            definition
                .tags
                .contains(&TerminalTag::new("downstairs").unwrap())
        );
        assert!(definition.groups.contains(&group_id));
        assert!(
            restored
                .group(&group_id)
                .unwrap()
                .members
                .contains(&terminal_id)
        );
    }

    #[test]
    fn metadata_and_group_changes_emit_structured_events_only_when_changed() {
        let (_directory, _path, registry) = registry();
        let terminal_id = terminal_id("hall-display");
        registry
            .register(
                registration("hall-display", "Hall Display"),
                TerminalConnectionId::new(),
                None,
                Instant::now(),
            )
            .unwrap();
        let mut changes = registry.subscribe_changes();
        registry
            .rename_terminal(&terminal_id, TerminalName::new("Hallway").unwrap())
            .unwrap();
        registry
            .rename_terminal(&terminal_id, TerminalName::new("Hallway").unwrap())
            .unwrap();
        registry
            .create_group(group_identity("public", "Public"))
            .unwrap();

        assert!(matches!(
            changes.try_recv().unwrap().kind,
            TerminalEventKind::MetadataChanged {
                terminal_id: id,
                change: TerminalMetadataChange::Renamed { name }
            } if id == terminal_id && name.as_str() == "Hallway"
        ));
        assert!(matches!(
            changes.try_recv().unwrap().kind,
            TerminalEventKind::GroupChanged {
                group_id,
                change: TerminalGroupChange::Created { name }
            } if group_id.as_str() == "public" && name.as_str() == "Public"
        ));
        assert!(matches!(
            changes.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn persistence_replacement_is_complete_and_leaves_no_temporary_files() {
        let (directory, path, registry) = registry();
        let now = Instant::now();
        for (index, id) in ["alpha", "beta"].into_iter().enumerate() {
            registry
                .register(
                    registration(id, id),
                    TerminalConnectionId::new(),
                    None,
                    now + Duration::from_secs(index as u64),
                )
                .unwrap();
            TerminalRegistry::load(&path, TIMEOUT).unwrap();
        }
        let entries = fs::read_dir(directory.path()).unwrap().count();
        assert_eq!(entries, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn failed_persistence_does_not_create_an_in_memory_definition() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state-as-directory");
        fs::create_dir(&path).unwrap();
        let registry = TerminalRegistry::load(path.join("missing"), TIMEOUT).unwrap();
        // Replace the expected file with a directory after loading to force atomic rename failure.
        fs::create_dir(path.join("missing")).unwrap();

        let error = registry
            .register(
                registration("hall-display", "Hall Display"),
                TerminalConnectionId::new(),
                None,
                Instant::now(),
            )
            .unwrap_err();

        assert!(matches!(error, RegisterError::Persistence(_)));
        assert!(registry.definitions().is_empty());
    }

    #[test]
    fn malformed_unsupported_and_duplicate_state_are_rejected_without_rewrite() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("terminals.json");
        for malformed in [
            "not json".to_owned(),
            json!({ "schema_version": 99, "terminals": [] }).to_string(),
            json!({
                "schema_version": 1,
                "terminals": [
                    {
                        "identity": { "id": "duplicate", "name": "One" },
                        "implementation": "bts-display",
                        "approved_capabilities": []
                    },
                    {
                        "identity": { "id": "duplicate", "name": "Two" },
                        "implementation": "bts-display",
                        "approved_capabilities": []
                    }
                ]
            })
            .to_string(),
            json!({
                "schema_version": 2,
                "terminals": [],
                "groups": [
                    { "identity": { "id": "duplicate", "name": "One" }, "members": [] },
                    { "identity": { "id": "duplicate", "name": "Two" }, "members": [] }
                ]
            })
            .to_string(),
            json!({
                "schema_version": 2,
                "terminals": [],
                "groups": [{
                    "identity": { "id": "public", "name": "Public" },
                    "members": ["missing-terminal"]
                }]
            })
            .to_string(),
            json!({
                "schema_version": 2,
                "terminals": [{
                    "identity": { "id": "hall-display", "name": "Hall Display" },
                    "description": " invalid description ",
                    "implementation": "bts-display",
                    "approved_capabilities": [],
                    "groups": []
                }],
                "groups": []
            })
            .to_string(),
            json!({
                "schema_version": 2,
                "terminals": [{
                    "identity": { "id": "hall-display", "name": "Hall Display" },
                    "implementation": "bts-display",
                    "approved_capabilities": [],
                    "groups": ["public"]
                }],
                "groups": []
            })
            .to_string(),
        ] {
            fs::write(&path, &malformed).unwrap();
            assert!(TerminalRegistry::load(&path, TIMEOUT).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), malformed);
        }
    }

    #[test]
    fn incompatible_protocol_implementation_and_capability_changes_are_rejected() {
        let (_directory, _path, registry) = registry();
        let now = Instant::now();
        let first = TerminalConnectionId::new();
        registry
            .register(
                registration("hall-display", "Hall Display"),
                first,
                None,
                now,
            )
            .unwrap();
        registry
            .disconnect(&terminal_id("hall-display"), first)
            .unwrap();

        let mut incompatible = registration("hall-display", "Ignored");
        incompatible.protocol_version = ProtocolVersion::new(0, 4);
        assert!(matches!(
            rejected_reason(
                registry
                    .register(incompatible, TerminalConnectionId::new(), None, now)
                    .unwrap_err()
            ),
            RegistrationRejectionReason::UnsupportedProtocolVersion { .. }
        ));

        let mut wrong_implementation = registration("hall-display", "Ignored");
        wrong_implementation.implementation =
            TerminalImplementationId::new("other-display").unwrap();
        assert!(matches!(
            rejected_reason(
                registry
                    .register(wrong_implementation, TerminalConnectionId::new(), None, now)
                    .unwrap_err()
            ),
            RegistrationRejectionReason::InvalidRegistration { .. }
        ));

        let mut expanded = registration("hall-display", "Ignored");
        expanded.capabilities = capabilities(&[
            TerminalCapability::RENDER_TEXT,
            TerminalCapability::PLAY_AUDIO,
        ]);
        assert!(matches!(
            rejected_reason(
                registry
                    .register(expanded, TerminalConnectionId::new(), None, now)
                    .unwrap_err()
            ),
            RegistrationRejectionReason::InvalidRegistration { .. }
        ));
    }

    #[test]
    fn concurrent_claims_produce_one_owner_and_one_duplicate_rejection() {
        let (_directory, _path, registry) = registry();
        let barrier = Arc::new(Barrier::new(3));
        let now = Instant::now();
        let mut handles = Vec::new();
        for _ in 0..2 {
            let registry = registry.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                registry.register(
                    registration("hall-display", "Hall Display"),
                    TerminalConnectionId::new(),
                    None,
                    now,
                )
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .into_iter()
                .filter_map(Result::err)
                .filter(|error| matches!(
                    error,
                    RegisterError::Rejected(RegistrationRejection {
                        reason: RegistrationRejectionReason::DuplicateTerminalId,
                        ..
                    })
                ))
                .count(),
            1
        );
        assert_eq!(registry.definitions().len(), 1);
    }

    #[test]
    fn concurrent_group_creation_has_one_winner_and_one_typed_conflict() {
        let (_directory, _path, registry) = registry();
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for name in ["First Name", "Second Name"] {
            let registry = registry.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                registry.create_group(group_identity("shared", name))
            }));
        }
        barrier.wait();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .into_iter()
                .filter(|result| matches!(
                    result,
                    Err(TerminalAdminError::GroupAlreadyExists(id)) if id.as_str() == "shared"
                ))
                .count(),
            1
        );
        assert_eq!(registry.groups().len(), 1);
    }
}
