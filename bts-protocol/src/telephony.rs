//! Generic telephone and voice-input contracts.

use crate::TerminalTarget;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One temporary target offered to a caller during a control session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelephonyTargetOption {
    pub target: TerminalTarget,
    pub name: String,
}

/// The online targets which may be selected for a telephony session.
///
/// Ordering is authoritative only for the lifetime of this response. Callers
/// must retain the target itself rather than its temporary menu position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelephonyTargets {
    pub terminals: Vec<TelephonyTargetOption>,
    pub groups: Vec<TelephonyTargetOption>,
    pub all: Option<TelephonyTargetOption>,
}

impl TelephonyTargets {
    pub fn options(&self) -> impl Iterator<Item = &TelephonyTargetOption> {
        self.terminals
            .iter()
            .chain(self.groups.iter())
            .chain(self.all.iter())
    }

    pub fn contains(&self, target: &TerminalTarget) -> bool {
        self.options().any(|option| &option.target == target)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceInputRequest {
    pub request_id: Uuid,
    pub channel_id: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceInputResult {
    pub request_id: Uuid,
    pub transcript: Option<String>,
    pub error: Option<String>,
}
