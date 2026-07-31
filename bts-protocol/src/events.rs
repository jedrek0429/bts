//! Event envelope and server-stream contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::addons::v1::{ActionRequest, AddonId, AddonManifest};
use crate::{
    BtsState, DisplayCommand, GroupId, GroupName, PresentationDeliveryResult, PresentationId,
    PresentationRequest, TerminalCapabilities, TerminalId, TerminalName, TerminalTag,
    TerminalTarget, VoiceInputRequest, VoiceInputResult,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum TerminalMetadataChange {
    Renamed { name: TerminalName },
    DescriptionChanged { description: Option<String> },
    TagAdded { tag: TerminalTag },
    TagRemoved { tag: TerminalTag },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum TerminalGroupChange {
    Created { name: GroupName },
    Renamed { name: GroupName },
    Deleted,
    MemberAdded { terminal_id: TerminalId },
    MemberRemoved { terminal_id: TerminalId },
}

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

    /// Converts a legacy untargeted display update into an immediate
    /// presentation for all online terminals.
    ///
    /// This adapter preserves the existing `display_requested` wire event while
    /// clients migrate to explicit presentation requests. Release operations do
    /// not contain presentation content and therefore return `None`.
    #[deprecated(
        note = "legacy display events are untargeted; send an explicit PresentationRequest instead"
    )]
    pub fn legacy_presentation_request(&self) -> Option<PresentationRequest> {
        let EventKind::DisplayRequested { command } = &self.kind else {
            return None;
        };
        let display = match command {
            DisplayCommand::Show { display, .. } | DisplayCommand::Update { display, .. } => {
                display.clone()
            }
            DisplayCommand::Release { .. } | DisplayCommand::ReleaseAll { .. } => return None,
        };
        Some(PresentationRequest {
            id: PresentationId::from_uuid(self.id),
            target: TerminalTarget::all(),
            required_capabilities: TerminalCapabilities::default(),
            display,
        })
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
    PresentationRequested {
        request: PresentationRequest,
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

/// Terminal administration and delivery changes are deliberately separate from
/// the release-line event stream so a closed adjacent-version `EventKind`
/// consumer never encounters an unknown variant.
pub const TERMINAL_EVENT_STREAM_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalEvent {
    pub stream_version: u16,
    #[serde(flatten)]
    pub kind: TerminalEventKind,
}

impl TerminalEvent {
    pub fn new(kind: TerminalEventKind) -> Self {
        Self {
            stream_version: TERMINAL_EVENT_STREAM_VERSION,
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalEventKind {
    #[serde(rename = "terminal_metadata_changed")]
    MetadataChanged {
        terminal_id: TerminalId,
        change: TerminalMetadataChange,
    },
    #[serde(rename = "terminal_group_changed")]
    GroupChanged {
        group_id: GroupId,
        change: TerminalGroupChange,
    },
    PresentationDeliveryCompleted {
        result: PresentationDeliveryResult,
    },
}
