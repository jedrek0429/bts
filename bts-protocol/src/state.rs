//! State retained and distributed by BTS Core.

use serde::{Deserialize, Serialize};

use crate::display::{DisplayLease, DisplayState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtsState {
    pub display: DisplayState,
    pub display_lease: Option<DisplayLease>,
}

impl Default for BtsState {
    fn default() -> Self {
        Self {
            display: DisplayState::Message {
                title: "Bansleben Telephone Services".to_owned(),
                body: "BTS Core is online".to_owned(),
            },
            display_lease: None,
        }
    }
}
