//! Stable contracts for BTS Addon API v1.
//!
//! Addons receive BTS events and can communicate with BTS Core through
//! [`AddonContext`]. Component-specific implementation details are deliberately
//! not exposed through this API.

use std::fmt;

use anyhow::{Context, Result};
use async_trait::async_trait;
use bts_protocol::{Action, BtsState, Event, EventKind, NewEvent};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// The version of the addon contract defined by this crate.
pub const ADDON_API_VERSION: u16 = 1;

/// A stable, machine-readable addon identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AddonId(String);

impl AddonId {
    /// Creates an addon identifier.
    ///
    /// Identifiers should use lower-case ASCII words separated by hyphens.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AddonId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Semantic version metadata for an addon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl AddonVersion {
    /// Creates version metadata from semantic-version components.
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for AddonVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// An action handled by an addon.
///
/// Unlike [`Action`], this contains no request-specific data and is therefore
/// suitable for capability discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Clock,
    Weather,
    Message,
    Blank,
}

impl From<&Action> for ActionKind {
    fn from(action: &Action) -> Self {
        match action {
            Action::Clock => Self::Clock,
            Action::Weather => Self::Weather,
            Action::Message { .. } => Self::Message,
            Action::Blank => Self::Blank,
        }
    }
}

/// A facility an addon needs in order to operate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonCapability {
    /// Publish events to BTS Core.
    PublishEvents,
    /// Read the current state from BTS Core.
    ReadState,
    /// Contact an HTTP service outside BTS.
    ExternalHttp,
}

/// Identity and capability declaration for one addon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonManifest {
    /// Addon API version expected by the implementation.
    pub api_version: u16,
    pub id: AddonId,
    pub name: String,
    pub version: AddonVersion,
    pub actions: Vec<ActionKind>,
    pub capabilities: Vec<AddonCapability>,
}

/// Restricted access to services supplied by BTS Core.
#[derive(Clone)]
pub struct AddonContext {
    http: Client,
    core_http_url: String,
}

impl AddonContext {
    /// Creates a Core context using the supplied HTTP base URL.
    pub fn new(core_http_url: impl Into<String>) -> Self {
        Self {
            http: Client::new(),
            core_http_url: core_http_url.into(),
        }
    }

    /// Publishes an event attributed to the calling addon.
    pub async fn publish(&self, source: &AddonId, kind: EventKind) -> Result<()> {
        let endpoint = format!("{}/api/v1/events", self.core_http_url.trim_end_matches('/'));

        self.http
            .post(endpoint)
            .json(&NewEvent {
                source: source.to_string(),
                kind,
            })
            .send()
            .await
            .context("failed to submit addon event to BTS Core")?
            .error_for_status()
            .context("BTS Core rejected addon event")?;

        Ok(())
    }

    /// Retrieves the current state retained by BTS Core.
    pub async fn state(&self) -> Result<BtsState> {
        let endpoint = format!("{}/api/v1/state", self.core_http_url.trim_end_matches('/'));

        self.http
            .get(endpoint)
            .send()
            .await
            .context("failed to request BTS state")?
            .error_for_status()
            .context("BTS Core rejected state request")?
            .json::<BtsState>()
            .await
            .context("failed to decode BTS state")
    }
}

/// Common lifecycle and event contract implemented by every BTS addon.
#[async_trait]
pub trait Addon: Send + Sync {
    /// Describes the addon and the facilities it uses.
    fn manifest(&self) -> AddonManifest;

    /// Starts addon-owned resources. The default implementation has no work.
    async fn start(&self, _context: &AddonContext) -> Result<()> {
        Ok(())
    }

    /// Handles one event received from BTS Core.
    async fn handle_event(&self, context: &AddonContext, event: &Event) -> Result<()>;

    /// Stops addon-owned resources. The default implementation has no work.
    async fn stop(&self, _context: &AddonContext) -> Result<()> {
        Ok(())
    }
}

/// A failure attributed to one addon without terminating the addon host.
#[derive(Debug)]
pub struct AddonFailure {
    pub addon_id: AddonId,
    pub operation: &'static str,
    pub error: anyhow::Error,
}

impl fmt::Display for AddonFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} failed: {}",
            self.addon_id, self.operation, self.error
        )
    }
}

impl std::error::Error for AddonFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_round_trips_through_json() {
        let manifest = AddonManifest {
            api_version: ADDON_API_VERSION,
            id: AddonId::new("weather"),
            name: "Weather Service".to_owned(),
            version: AddonVersion::new(1, 2, 3),
            actions: vec![ActionKind::Weather],
            capabilities: vec![
                AddonCapability::PublishEvents,
                AddonCapability::ExternalHttp,
            ],
        };

        let json = serde_json::to_string(&manifest).expect("manifest should serialise");
        let decoded = serde_json::from_str(&json).expect("manifest should deserialise");

        assert_eq!(manifest, decoded);
        assert!(json.contains("\"external_http\""));
    }

    #[test]
    fn action_kind_discards_request_data() {
        let action = Action::Message {
            title: "Notice".to_owned(),
            body: "Hello".to_owned(),
        };

        assert_eq!(ActionKind::from(&action), ActionKind::Message);
    }
}
