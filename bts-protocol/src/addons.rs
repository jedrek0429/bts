//! Addon registration, action and capability contracts.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::display::ScreenKind;

pub const ADDON_API_VERSION: u16 = 1;

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
    pub digit: char,
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
}
