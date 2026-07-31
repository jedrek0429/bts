//! Generic telephone and voice-input contracts.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
