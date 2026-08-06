//! BTS Addon API version 1.

use crate::{
    AssetRef, BtsState, DisplayLeaseId, DisplayState, DtmfMenuKey, Event, EventKind, ScreenKind,
    TerminalTarget,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

pub use bts_compat::ADDON_API_VERSION as API_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AddonId(pub String);
impl AddonId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for AddonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActionId(pub String);
impl ActionId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for ActionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}
impl AddonVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonCapability {
    Display,
    Assets,
    Configuration,
    DataDirectory,
    ExternalHttp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRegistration {
    pub id: ActionId,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuEntry {
    /// A validated addon key. Platform-reserved controls cannot be represented.
    pub digit: DtmfMenuKey,
    pub prompt: String,
    pub action: ActionId,
    pub order: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddonManifest {
    pub api_version: u16,
    pub id: AddonId,
    pub name: String,
    pub version: AddonVersion,
    pub actions: Vec<ActionRegistration>,
    pub menu: Vec<MenuEntry>,
    pub capabilities: Vec<AddonCapability>,
    pub screens: Vec<ScreenKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRequest {
    pub action: ActionId,
    pub parameters: serde_json::Value,
    /// The mutable target selected by the originating telephony session.
    ///
    /// Non-telephony actions remain valid without session context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TerminalTarget>,
}

/// Transport-neutral services supplied to an addon by its host.
///
/// Implementations may use HTTP, another network transport, or an in-process
/// test double. Addons must not assume that Core shares their filesystem or host.
#[async_trait]
pub trait AddonContext: Send + Sync {
    fn clone_box(&self) -> Box<dyn AddonContext>;
    fn addon_id(&self) -> &AddonId;
    /// Returns the invocation-scoped telephony target, when one was supplied.
    fn selected_target(&self) -> Option<&TerminalTarget> {
        None
    }
    async fn publish(&self, kind: EventKind) -> Result<()>;
    async fn state(&self) -> Result<BtsState>;
    async fn request_action(&self, action: ActionId, parameters: serde_json::Value) -> Result<()>;
    async fn show(&self, display: DisplayState, priority: u8) -> Result<DisplayLeaseId>;
    async fn update(&self, lease_id: DisplayLeaseId, display: DisplayState) -> Result<()>;
    async fn release(&self, lease_id: DisplayLeaseId) -> Result<()>;
    async fn release_all(&self) -> Result<()>;
    async fn upload_asset(&self, content_type: String, bytes: Vec<u8>) -> Result<AssetRef>;
}

/// A BTS addon that can run against any Addon API v1 context implementation.
#[async_trait]
pub trait Addon: Send + Sync {
    fn manifest(&self) -> AddonManifest;
    async fn start(&self, _context: &dyn AddonContext) -> Result<()> {
        Ok(())
    }
    async fn handle_event(&self, context: &dyn AddonContext, event: &Event) -> Result<()>;
    async fn stop(&self, _context: &dyn AddonContext) -> Result<()> {
        Ok(())
    }
}
