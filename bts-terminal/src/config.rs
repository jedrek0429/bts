use std::{error::Error, fmt, time::Duration};

use bts_protocol::{
    ProtocolVersion, TerminalCapabilities, TerminalId, TerminalIdentity, TerminalImplementationId,
    TerminalImplementationVersion, TerminalName, TerminalRegistration,
};
use semver::Version;

pub use bts_protocol::TerminalRuntimeDiagnostics as RuntimeDiagnostics;

/// Deterministic reconnect delays with no jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    initial_delay: Duration,
    maximum_delay: Duration,
}

impl ReconnectPolicy {
    pub fn new(
        initial_delay: Duration,
        maximum_delay: Duration,
    ) -> Result<Self, ConfigurationError> {
        if initial_delay.is_zero() {
            return Err(ConfigurationError::InvalidReconnectPolicy(
                "the initial reconnect delay must be greater than zero".to_owned(),
            ));
        }
        if maximum_delay < initial_delay {
            return Err(ConfigurationError::InvalidReconnectPolicy(
                "the maximum reconnect delay must not be shorter than the initial delay".to_owned(),
            ));
        }
        Ok(Self {
            initial_delay,
            maximum_delay,
        })
    }

    pub const fn initial_delay(&self) -> Duration {
        self.initial_delay
    }

    pub const fn maximum_delay(&self) -> Duration {
        self.maximum_delay
    }

    /// Returns the delay for a one-based consecutive failure count.
    pub fn delay_for_failure(&self, consecutive_failures: u32) -> Duration {
        let exponent = consecutive_failures.saturating_sub(1).min(31);
        let multiplier = 1_u32 << exponent;
        self.initial_delay
            .saturating_mul(multiplier)
            .min(self.maximum_delay)
    }
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            maximum_delay: Duration::from_secs(30),
        }
    }
}

/// Explicit, reusable endpoint configuration.
///
/// Loading values from files, environment variables or installer prompts is a
/// consuming application's responsibility. In particular, this type does not
/// derive identity from a hostname, address or output.
#[derive(Debug, Clone)]
pub struct TerminalConfiguration {
    core_websocket_url: String,
    registration: TerminalRegistration,
    implementation_version: TerminalImplementationVersion,
    runtime_diagnostics: RuntimeDiagnostics,
    reconnect_policy: ReconnectPolicy,
    registration_timeout: Duration,
    maximum_pending_presentations: usize,
}

impl TerminalConfiguration {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        core_websocket_url: impl Into<String>,
        terminal_id: TerminalId,
        suggested_name: TerminalName,
        implementation: TerminalImplementationId,
        implementation_version: Version,
        capabilities: TerminalCapabilities,
    ) -> Result<Self, ConfigurationError> {
        let core_websocket_url = core_websocket_url.into();
        if !(core_websocket_url.starts_with("ws://") || core_websocket_url.starts_with("wss://")) {
            return Err(ConfigurationError::InvalidCoreWebsocketUrl);
        }

        Ok(Self {
            core_websocket_url,
            registration: TerminalRegistration {
                identity: TerminalIdentity {
                    id: terminal_id,
                    name: suggested_name,
                },
                implementation,
                protocol_version: ProtocolVersion::CURRENT,
                capabilities,
            },
            implementation_version: TerminalImplementationVersion::new(implementation_version)
                .map_err(|error| ConfigurationError::InvalidVersion(error.to_string()))?,
            runtime_diagnostics: RuntimeDiagnostics::default(),
            reconnect_policy: ReconnectPolicy::default(),
            registration_timeout: Duration::from_secs(10),
            maximum_pending_presentations: 64,
        })
    }

    pub fn with_runtime_diagnostics(mut self, diagnostics: RuntimeDiagnostics) -> Self {
        self.runtime_diagnostics = diagnostics;
        self
    }

    pub fn with_reconnect_policy(mut self, policy: ReconnectPolicy) -> Self {
        self.reconnect_policy = policy;
        self
    }

    pub fn with_registration_timeout(
        mut self,
        timeout: Duration,
    ) -> Result<Self, ConfigurationError> {
        if timeout.is_zero() {
            return Err(ConfigurationError::InvalidTimeout(
                "registration timeout".to_owned(),
            ));
        }
        self.registration_timeout = timeout;
        Ok(self)
    }

    pub fn with_maximum_pending_presentations(
        mut self,
        maximum: usize,
    ) -> Result<Self, ConfigurationError> {
        if maximum == 0 {
            return Err(ConfigurationError::InvalidPendingLimit);
        }
        self.maximum_pending_presentations = maximum;
        Ok(self)
    }

    pub fn core_websocket_url(&self) -> &str {
        &self.core_websocket_url
    }

    pub fn terminal_id(&self) -> &TerminalId {
        &self.registration.identity.id
    }

    pub fn suggested_name(&self) -> &TerminalName {
        &self.registration.identity.name
    }

    pub fn implementation(&self) -> &TerminalImplementationId {
        &self.registration.implementation
    }

    pub fn implementation_version(&self) -> &Version {
        self.implementation_version.as_version()
    }

    pub fn capabilities(&self) -> &TerminalCapabilities {
        &self.registration.capabilities
    }

    pub fn runtime_diagnostics(&self) -> &RuntimeDiagnostics {
        &self.runtime_diagnostics
    }

    pub fn reconnect_policy(&self) -> ReconnectPolicy {
        self.reconnect_policy
    }

    pub(crate) fn registration(&self) -> &TerminalRegistration {
        &self.registration
    }

    pub(crate) fn registration_timeout(&self) -> Duration {
        self.registration_timeout
    }

    pub(crate) fn maximum_pending_presentations(&self) -> usize {
        self.maximum_pending_presentations
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigurationError {
    InvalidCoreWebsocketUrl,
    InvalidVersion(String),
    InvalidReconnectPolicy(String),
    InvalidTimeout(String),
    InvalidPendingLimit,
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCoreWebsocketUrl => {
                formatter.write_str("the Core WebSocket URL must start with ws:// or wss://")
            }
            Self::InvalidVersion(detail) | Self::InvalidReconnectPolicy(detail) => {
                formatter.write_str(detail)
            }
            Self::InvalidTimeout(name) => write!(formatter, "the {name} must be greater than zero"),
            Self::InvalidPendingLimit => {
                formatter.write_str("the pending presentation limit must be greater than zero")
            }
        }
    }
}

impl Error for ConfigurationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_is_exponential_and_bounded() {
        let policy =
            ReconnectPolicy::new(Duration::from_millis(250), Duration::from_secs(2)).unwrap();
        assert_eq!(policy.delay_for_failure(1), Duration::from_millis(250));
        assert_eq!(policy.delay_for_failure(2), Duration::from_millis(500));
        assert_eq!(policy.delay_for_failure(3), Duration::from_secs(1));
        assert_eq!(policy.delay_for_failure(4), Duration::from_secs(2));
        assert_eq!(policy.delay_for_failure(30), Duration::from_secs(2));
    }

    #[test]
    fn diagnostics_are_bounded_and_validated() {
        let diagnostics = RuntimeDiagnostics::new([
            ("os.name".to_owned(), "Linux".to_owned()),
            ("screen.size".to_owned(), "1280x720".to_owned()),
        ])
        .unwrap();
        assert_eq!(diagnostics.iter().count(), 2);

        assert!(RuntimeDiagnostics::new([("Room Name".to_owned(), "Hall".to_owned())]).is_err());
        assert!(
            RuntimeDiagnostics::new([("os.name".to_owned(), "invalid\nvalue".to_owned())]).is_err()
        );
    }
}
