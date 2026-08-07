//! Stable terminal identity, capability and registration contracts.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};

const MAX_IDENTIFIER_LENGTH: usize = 64;
const MAX_NAME_LENGTH: usize = 100;
const MAX_IMPLEMENTATION_VERSION_LENGTH: usize = 64;
const MAX_RUNTIME_DIAGNOSTICS: usize = 32;
const MAX_DIAGNOSTIC_VALUE_LENGTH: usize = 256;

/// An invalid machine-readable protocol identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifierError {
    kind: &'static str,
    value: String,
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} must be 1-{MAX_IDENTIFIER_LENGTH} characters of lower-case ASCII letters, digits, '.', '_' or '-', and start and end with a letter or digit: {:?}",
            self.kind, self.value
        )
    }
}

impl Error for IdentifierError {}

pub(crate) fn validate_identifier(kind: &'static str, value: &str) -> Result<(), IdentifierError> {
    let valid_length = !value.is_empty() && value.len() <= MAX_IDENTIFIER_LENGTH;
    let valid_edges = value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric);
    let valid_characters = value.bytes().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, b'.' | b'_' | b'-')
    });

    if valid_length && valid_edges && valid_characters {
        Ok(())
    } else {
        Err(IdentifierError {
            kind,
            value: value.to_owned(),
        })
    }
}

macro_rules! identifier {
    ($name:ident, $kind:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, $crate::IdentifierError> {
                let value = value.into();
                $crate::terminal::validate_identifier($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, formatter)
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::IdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = $crate::IdentifierError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

pub(crate) use identifier;

identifier!(TerminalId, "terminal identifier");
identifier!(GroupId, "group identifier");
identifier!(
    TerminalImplementationId,
    "terminal implementation identifier"
);
identifier!(CapabilityId, "capability identifier");

/// An invalid user-facing protocol name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameError {
    kind: &'static str,
    value: String,
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} must be 1-{MAX_NAME_LENGTH} characters, have no surrounding whitespace and contain no control characters: {:?}",
            self.kind, self.value
        )
    }
}

impl Error for NameError {}

fn validate_name(kind: &'static str, value: &str) -> Result<(), NameError> {
    let character_count = value.chars().count();
    if (1..=MAX_NAME_LENGTH).contains(&character_count)
        && value.trim() == value
        && !value.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(NameError {
            kind,
            value: value.to_owned(),
        })
    }
}

/// A terminal's user-facing name. This may change without changing its identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TerminalName(String);

impl TerminalName {
    pub fn new(value: impl Into<String>) -> Result<Self, NameError> {
        let value = value.into();
        validate_name("terminal name", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TerminalName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for TerminalName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalIdentity {
    pub id: TerminalId,
    pub name: TerminalName,
}

/// A group's user-facing name, separate from its stable identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct GroupName(String);

impl GroupName {
    pub fn new(value: impl Into<String>) -> Result<Self, NameError> {
        let value = value.into();
        validate_name("group name", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for GroupName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for GroupName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupIdentity {
    pub id: GroupId,
    pub name: GroupName,
}

/// A terminal protocol version, independent of implementation versioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    /// The terminal contract shipped by the BTS 0.3.x release line.
    pub const CURRENT: Self = Self { major: 0, minor: 3 };

    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Stable versions are compatible within a major version. Before 1.0,
    /// each minor version is an independent compatibility line.
    pub const fn is_compatible_with(self, other: Self) -> bool {
        self.major == other.major && (self.major != 0 || self.minor == other.minor)
    }
}

/// An open, functional capability identifier.
///
/// Unknown valid identifiers must be retained and ignored by consumers which do
/// not understand them. Hardware and operating-system details are not routing
/// capabilities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalCapability(CapabilityId);

impl TerminalCapability {
    pub const RENDER_TEXT: &'static str = "render_text";
    pub const RENDER_IMAGES: &'static str = "render_images";
    pub const RENDER_VIDEO: &'static str = "render_video";
    pub const PLAY_AUDIO: &'static str = "play_audio";
    pub const CAPTURE_AUDIO: &'static str = "capture_audio";
    pub const RECEIVE_TOUCH: &'static str = "receive_touch";
    pub const RECEIVE_KEYBOARD: &'static str = "receive_keyboard";
    pub const RECEIVE_POINTER: &'static str = "receive_pointer";

    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        CapabilityId::new(value).map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalCapabilities(BTreeSet<TerminalCapability>);

impl TerminalCapabilities {
    pub fn new(capabilities: impl IntoIterator<Item = TerminalCapability>) -> Self {
        Self(capabilities.into_iter().collect())
    }

    pub fn contains(&self, capability: &TerminalCapability) -> bool {
        self.0.contains(capability)
    }

    pub fn supports_all(&self, required: &Self) -> bool {
        required.0.is_subset(&self.0)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TerminalCapability> {
        self.0.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A terminal implementation's semantic version, independent of the terminal
/// protocol compatibility version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TerminalImplementationVersion(Version);

impl TerminalImplementationVersion {
    pub fn new(version: Version) -> Result<Self, ImplementationVersionError> {
        if version.to_string().len() <= MAX_IMPLEMENTATION_VERSION_LENGTH {
            Ok(Self(version))
        } else {
            Err(ImplementationVersionError)
        }
    }

    pub const fn as_version(&self) -> &Version {
        &self.0
    }
}

impl<'de> Deserialize<'de> for TerminalImplementationVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() > MAX_IMPLEMENTATION_VERSION_LENGTH {
            return Err(serde::de::Error::custom(ImplementationVersionError));
        }
        let version = Version::parse(&value).map_err(serde::de::Error::custom)?;
        Self::new(version).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImplementationVersionError;

impl fmt::Display for ImplementationVersionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "terminal implementation version must contain at most {MAX_IMPLEMENTATION_VERSION_LENGTH} bytes"
        )
    }
}

impl Error for ImplementationVersionError {}

/// Bounded, informational runtime metadata. These values are never routing
/// capabilities and Core retains them only with ephemeral presence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct TerminalRuntimeDiagnostics(BTreeMap<String, String>);

impl TerminalRuntimeDiagnostics {
    pub fn new(
        values: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, RuntimeDiagnosticsError> {
        let values = values.into_iter().collect::<BTreeMap<_, _>>();
        if values.len() > MAX_RUNTIME_DIAGNOSTICS {
            return Err(RuntimeDiagnosticsError::TooMany);
        }
        for (key, value) in &values {
            validate_identifier("runtime diagnostic key", key)
                .map_err(|_| RuntimeDiagnosticsError::InvalidKey(key.clone()))?;
            if value.chars().count() > MAX_DIAGNOSTIC_VALUE_LENGTH
                || value.chars().any(char::is_control)
            {
                return Err(RuntimeDiagnosticsError::InvalidValue(key.clone()));
            }
        }
        Ok(Self(values))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

impl<'de> Deserialize<'de> for TerminalRuntimeDiagnostics {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = BTreeMap::<String, String>::deserialize(deserializer)?;
        Self::new(values).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDiagnosticsError {
    TooMany,
    InvalidKey(String),
    InvalidValue(String),
}

impl fmt::Display for RuntimeDiagnosticsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooMany => write!(
                formatter,
                "at most {MAX_RUNTIME_DIAGNOSTICS} runtime diagnostics may be reported"
            ),
            Self::InvalidKey(key) => write!(formatter, "invalid runtime diagnostic key {key:?}"),
            Self::InvalidValue(key) => write!(
                formatter,
                "runtime diagnostic {key:?} must contain at most {MAX_DIAGNOSTIC_VALUE_LENGTH} non-control characters"
            ),
        }
    }
}

impl Error for RuntimeDiagnosticsError {}

/// The validated fields a terminal supplies when opening a connection.
///
/// Additional wire fields are ignored for forwards compatibility. Routing may
/// use only typed protocol fields, never arbitrary extension metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalRegistration {
    pub identity: TerminalIdentity,
    pub implementation: TerminalImplementationId,
    pub protocol_version: ProtocolVersion,
    #[serde(default)]
    pub capabilities: TerminalCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationRejection {
    pub terminal_id: Option<TerminalId>,
    pub reason: RegistrationRejectionReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RegistrationRejectionReason {
    UnsupportedProtocolVersion {
        received: ProtocolVersion,
        supported: ProtocolVersion,
    },
    DuplicateTerminalId,
    /// Retained for compatibility with the issue #27 foundation contract.
    IdentityAlreadyConnected,
    InvalidRegistration {
        detail: String,
    },
}
