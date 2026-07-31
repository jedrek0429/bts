//! Authoritative terminal definitions and ephemeral connection presence.

use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File},
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use anyhow::{Context, Result as AnyResult, bail};
use bts_protocol::{
    GroupId, ProtocolVersion, RegistrationRejection, RegistrationRejectionReason,
    TerminalCapabilities, TerminalConnectionId, TerminalId, TerminalIdentity,
    TerminalImplementationId, TerminalRegistration, TerminalTag,
};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing::{info, warn};

pub const DEFAULT_TERMINAL_STATE_PATH: &str = "/var/lib/bts/terminals.json";
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
pub const DEFAULT_PRESENCE_TIMEOUT: Duration = Duration::from_secs(90);
pub const DEFAULT_EXPIRY_INTERVAL: Duration = Duration::from_secs(30);

const TERMINAL_STATE_SCHEMA_VERSION: u16 = 1;

/// A durable terminal definition. Group and tag membership is retained here but
/// will be administered by the later terminal-management slice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalDefinition {
    pub identity: TerminalIdentity,
    pub implementation: TerminalImplementationId,
    pub approved_capabilities: TerminalCapabilities,
    #[serde(default)]
    pub tags: BTreeSet<TerminalTag>,
    #[serde(default)]
    pub groups: BTreeSet<GroupId>,
}

/// Volatile ownership of one terminal identity by one live connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalPresence {
    pub connection_id: TerminalConnectionId,
    pub remote_address: Option<SocketAddr>,
    pub connected_at: Instant,
    pub last_seen: Instant,
    pub protocol_version: ProtocolVersion,
    pub declared_capabilities: TerminalCapabilities,
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

#[derive(Debug, Serialize, Deserialize)]
struct PersistedRegistry {
    schema_version: u16,
    terminals: Vec<TerminalDefinition>,
}

struct RegistryState {
    path: PathBuf,
    stale_after: Duration,
    definitions: HashMap<TerminalId, TerminalDefinition>,
    presences: HashMap<TerminalId, TerminalPresence>,
}

/// A cloneable, concurrency-safe handle to the Core terminal registry.
#[derive(Clone)]
pub struct TerminalRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl TerminalRegistry {
    pub fn load(path: impl Into<PathBuf>, stale_after: Duration) -> AnyResult<Self> {
        let path = path.into();
        let definitions = load_definitions(&path)?;
        Ok(Self {
            state: Arc::new(Mutex::new(RegistryState {
                path,
                stale_after,
                definitions,
                // Presence is deliberately never restored after a Core restart.
                presences: HashMap::new(),
            })),
        })
    }

    pub fn register(
        &self,
        registration: TerminalRegistration,
        connection_id: TerminalConnectionId,
        remote_address: Option<SocketAddr>,
        now: Instant,
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
                (definition.clone(), false)
            }
            None => {
                let definition = TerminalDefinition {
                    identity: registration.identity.clone(),
                    implementation: registration.implementation.clone(),
                    approved_capabilities: registration.capabilities.clone(),
                    tags: BTreeSet::new(),
                    groups: BTreeSet::new(),
                };
                persist_with_new_definition(&state, definition.clone())
                    .map_err(RegisterError::Persistence)?;
                state
                    .definitions
                    .insert(terminal_id.clone(), definition.clone());
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
                protocol_version: registration.protocol_version,
                declared_capabilities: registration.capabilities,
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
        let mut state = self.lock();
        let presence = state
            .presences
            .get_mut(terminal_id)
            .ok_or(PresenceError::Offline)?;
        if presence.connection_id != connection_id {
            return Err(PresenceError::NotConnectionOwner);
        }
        presence.last_seen = presence.last_seen.max(now);
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
        state.presences.remove(terminal_id);
        info!(terminal_id = %terminal_id, ?connection_id, "terminal disconnected");
        Ok(())
    }

    /// Removes presences whose last activity is strictly older than the timeout.
    pub fn expire_stale(&self, now: Instant) -> Vec<TerminalId> {
        let mut state = self.lock();
        let stale_after = state.stale_after;
        let mut expired = state
            .presences
            .iter()
            .filter(|(_, presence)| is_stale(presence, now, stale_after))
            .map(|(terminal_id, _)| terminal_id.clone())
            .collect::<Vec<_>>();
        expired.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        for terminal_id in &expired {
            if let Some(presence) = state.presences.remove(terminal_id) {
                warn!(
                    terminal_id = %terminal_id,
                    connection_id = ?presence.connection_id,
                    "terminal presence timed out"
                );
            }
        }
        expired
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

    pub fn presence(&self, terminal_id: &TerminalId) -> Option<TerminalPresence> {
        self.lock().presences.get(terminal_id).cloned()
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

fn load_definitions(path: &Path) -> AnyResult<HashMap<TerminalId, TerminalDefinition>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("could not read terminal registry state {}", path.display())
            });
        }
    };
    let persisted: PersistedRegistry = serde_json::from_slice(&bytes)
        .with_context(|| format!("terminal registry state {} is malformed", path.display()))?;
    if persisted.schema_version != TERMINAL_STATE_SCHEMA_VERSION {
        bail!(
            "unsupported terminal registry schema {} in {}; expected {}",
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
    Ok(definitions)
}

fn persist_with_new_definition(
    state: &RegistryState,
    definition: TerminalDefinition,
) -> AnyResult<()> {
    let mut definitions = state.definitions.values().cloned().collect::<Vec<_>>();
    definitions.push(definition);
    persist_definitions(&state.path, definitions)
}

fn persist_definitions(
    path: &Path,
    definitions: impl IntoIterator<Item = TerminalDefinition>,
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
    let persisted = PersistedRegistry {
        schema_version: TERMINAL_STATE_SCHEMA_VERSION,
        terminals,
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
        registry
            .register(
                registration("hall-display", "Hall Display"),
                TerminalConnectionId::new(),
                None,
                now,
            )
            .unwrap();

        assert!(registry.expire_stale(now + TIMEOUT).is_empty());
        assert_eq!(
            registry.expire_stale(now + TIMEOUT + Duration::from_nanos(1)),
            vec![id.clone()]
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
}
