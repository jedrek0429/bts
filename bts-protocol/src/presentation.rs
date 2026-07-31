//! Terminal lifecycle and presentation delivery messages.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DisplayState, RegistrationRejection, ResolvedTarget, TerminalCapabilities, TerminalId,
    TerminalRegistration, TerminalTarget,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresentationId(Uuid);

impl PresentationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for PresentationId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalConnectionId(Uuid);

impl TerminalConnectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for TerminalConnectionId {
    fn default() -> Self {
        Self::new()
    }
}

/// A presentation before Core resolves its target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationRequest {
    pub id: PresentationId,
    pub target: TerminalTarget,
    #[serde(default)]
    pub required_capabilities: TerminalCapabilities,
    pub display: DisplayState,
}

/// A presentation after Core has selected its concrete recipients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresentationDispatch {
    pub request: PresentationRequest,
    pub resolved_target: ResolvedTarget,
}

impl PresentationDispatch {
    pub fn new(
        request: PresentationRequest,
        resolved_target: ResolvedTarget,
    ) -> Result<Self, PresentationDispatchError> {
        if request.target == resolved_target.requested {
            Ok(Self {
                request,
                resolved_target,
            })
        } else {
            Err(PresentationDispatchError)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationDispatchError;

impl std::fmt::Display for PresentationDispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a presentation dispatch must resolve its request target")
    }
}

impl std::error::Error for PresentationDispatchError {}

impl<'de> Deserialize<'de> for PresentationDispatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WirePresentationDispatch {
            request: PresentationRequest,
            resolved_target: ResolvedTarget,
        }

        let dispatch = WirePresentationDispatch::deserialize(deserializer)?;
        Self::new(dispatch.request, dispatch.resolved_target).map_err(serde::de::Error::custom)
    }
}

crate::terminal::identifier!(PresentationRejectionCode, "presentation rejection code");

impl PresentationRejectionCode {
    pub const UNSUPPORTED_CAPABILITIES: &'static str = "unsupported_capabilities";
    pub const INVALID_PRESENTATION: &'static str = "invalid_presentation";
    pub const BUSY: &'static str = "busy";
    pub const INTERNAL_ERROR: &'static str = "internal_error";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationRejection {
    pub code: PresentationRejectionCode,
    pub detail: Option<String>,
}

/// Messages sent by a terminal to Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum TerminalClientMessage {
    Register {
        registration: TerminalRegistration,
    },
    Heartbeat {
        terminal_id: TerminalId,
        connection_id: TerminalConnectionId,
    },
    Disconnect {
        terminal_id: TerminalId,
        connection_id: TerminalConnectionId,
        reason: Option<String>,
    },
    PresentationAccepted {
        terminal_id: TerminalId,
        connection_id: TerminalConnectionId,
        presentation_id: PresentationId,
    },
    PresentationRejected {
        terminal_id: TerminalId,
        connection_id: TerminalConnectionId,
        presentation_id: PresentationId,
        rejection: PresentationRejection,
    },
}

/// Messages sent by Core to a terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum CoreTerminalMessage {
    RegistrationAcknowledged {
        terminal_id: TerminalId,
        connection_id: TerminalConnectionId,
        protocol_version: crate::ProtocolVersion,
        heartbeat_interval_seconds: u32,
    },
    RegistrationRejected {
        rejection: RegistrationRejection,
    },
    HeartbeatAcknowledged {
        connection_id: TerminalConnectionId,
    },
    PresentationDispatch {
        presentation: Box<PresentationDispatch>,
    },
}
