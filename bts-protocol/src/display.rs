//! Declarative display contracts and lease operations.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::addons::AddonId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenKind {
    Clock,
    Weather,
    Message,
    Blank,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "screen", rename_all = "snake_case")]
pub enum DisplayState {
    Clock {
        time: String,
        seconds: String,
        date: String,
    },
    Weather {
        location: String,
        temperature: String,
        condition: String,
        details: Vec<String>,
        updated_at: String,
    },
    Message {
        title: String,
        body: String,
    },
    Blank,
}

impl DisplayState {
    pub fn kind(&self) -> ScreenKind {
        match self {
            Self::Clock { .. } => ScreenKind::Clock,
            Self::Weather { .. } => ScreenKind::Weather,
            Self::Message { .. } => ScreenKind::Message,
            Self::Blank => ScreenKind::Blank,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DisplayLeaseId(pub Uuid);

impl DisplayLeaseId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for DisplayLeaseId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayLease {
    pub id: DisplayLeaseId,
    pub owner: AddonId,
    pub priority: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum DisplayCommand {
    Show {
        lease: DisplayLease,
        display: DisplayState,
    },
    Update {
        addon_id: AddonId,
        lease_id: DisplayLeaseId,
        display: DisplayState,
    },
    Release {
        addon_id: AddonId,
        lease_id: DisplayLeaseId,
    },
    ReleaseAll {
        addon_id: AddonId,
    },
}
