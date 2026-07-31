//! Event envelope and server-stream contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::addons::v1::{ActionRequest, AddonId, AddonManifest};
use crate::{BtsState, DisplayCommand, VoiceInputRequest, VoiceInputResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub source: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl Event {
    pub fn new(source: impl Into<String>, kind: EventKind) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            source: source.into(),
            kind,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEvent {
    pub source: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    SystemStarted {
        component: String,
    },
    AddonRegistered {
        manifest: AddonManifest,
    },
    AddonStopped {
        addon_id: AddonId,
    },
    ActionRequested {
        request: ActionRequest,
    },
    DisplayRequested {
        command: DisplayCommand,
    },
    PhoneCallStarted {
        channel_id: String,
        caller: Option<String>,
    },
    PhoneDtmfReceived {
        channel_id: String,
        digit: String,
    },
    PhoneCallEnded {
        channel_id: String,
    },
    VoiceInputRequested {
        request: VoiceInputRequest,
    },
    VoiceInputCompleted {
        result: VoiceInputResult,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot { state: BtsState },
    Event { event: Box<Event>, state: BtsState },
}
