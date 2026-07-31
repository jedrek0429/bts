//! Core-managed binary asset references.

use crate::AddonId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub Uuid);
impl AssetId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRef {
    pub id: AssetId,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetUpload {
    pub addon_id: AddonId,
    pub content_type: String,
    pub bytes: Vec<u8>,
}
