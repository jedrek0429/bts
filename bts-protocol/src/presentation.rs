//! Terminal lifecycle and presentation delivery messages.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    DisplayState, RegistrationRejection, ResolvedTarget, TerminalCapabilities, TerminalId,
    TerminalRegistration, TerminalTarget,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

/// A Core-assigned, monotonically increasing ordering token for one terminal.
/// Generations are independent between terminals and survive reconnects for the
/// lifetime of the Core process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresentationGeneration(u64);

impl PresentationGeneration {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Default for PresentationId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    /// Connection-specific ordering and validity context for every recipient.
    /// A terminal must select its own entry, reject a different connection,
    /// discard generations older than the greatest one observed and stop
    /// applying the presentation once `valid_for_millis` has elapsed.
    #[serde(default)]
    pub deliveries: BTreeMap<TerminalId, PresentationDeliveryContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationDeliveryContext {
    pub connection_id: TerminalConnectionId,
    pub generation: PresentationGeneration,
    pub valid_for_millis: u64,
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
                deliveries: BTreeMap::new(),
            })
        } else {
            Err(PresentationDispatchError)
        }
    }

    pub fn with_deliveries(
        request: PresentationRequest,
        resolved_target: ResolvedTarget,
        deliveries: BTreeMap<TerminalId, PresentationDeliveryContext>,
    ) -> Result<Self, PresentationDispatchError> {
        if deliveries
            .keys()
            .all(|terminal_id| resolved_target.terminals.contains(terminal_id))
        {
            let mut dispatch = Self::new(request, resolved_target)?;
            dispatch.deliveries = deliveries;
            Ok(dispatch)
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
            #[serde(default)]
            deliveries: BTreeMap<TerminalId, PresentationDeliveryContext>,
        }

        let dispatch = WirePresentationDispatch::deserialize(deserializer)?;
        Self::with_deliveries(
            dispatch.request,
            dispatch.resolved_target,
            dispatch.deliveries,
        )
        .map_err(serde::de::Error::custom)
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

/// The current bounded-delivery outcome for one matched terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PresentationDeliveryOutcome {
    /// Core has dispatched the presentation and is awaiting an acknowledgement.
    Pending,
    Accepted,
    Rejected {
        rejection: PresentationRejection,
    },
    /// The terminal is registered but had no live presence at dispatch time.
    Offline,
    Incompatible {
        missing_capabilities: TerminalCapabilities,
    },
    TimedOut,
    Disconnected,
    /// A newer generation for this terminal made the pending delivery invalid.
    Superseded,
}

/// A stable snapshot of a presentation's target resolution and delivery state.
///
/// An empty `outcomes` map means that no registered terminals matched. Offline
/// terminals remain in `outcomes`, even when the requested scope is `online`,
/// so callers can distinguish no match from an offline match. The optional
/// resolved target follows the requested scope and is absent when that scope
/// selected no terminals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationDeliveryResult {
    pub presentation_id: PresentationId,
    pub requested_target: TerminalTarget,
    pub resolved_target: Option<ResolvedTarget>,
    pub outcomes: BTreeMap<TerminalId, PresentationDeliveryOutcome>,
}

impl PresentationDeliveryResult {
    pub fn is_complete(&self) -> bool {
        self.outcomes
            .values()
            .all(|outcome| !matches!(outcome, PresentationDeliveryOutcome::Pending))
    }

    pub fn accepted_terminals(&self) -> BTreeSet<TerminalId> {
        self.outcomes
            .iter()
            .filter(|(_, outcome)| matches!(outcome, PresentationDeliveryOutcome::Accepted))
            .map(|(terminal_id, _)| terminal_id.clone())
            .collect()
    }
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
