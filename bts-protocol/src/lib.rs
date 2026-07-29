use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A complete BTS event as distributed by bts-core.
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

/// The useful part submitted by a BTS client.
///
/// bts-core adds the unique ID and timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewEvent {
    pub source: String,

    #[serde(flatten)]
    pub kind: EventKind,
}

/// Every event type understood by BTS.
///
/// `tag = "type"` produces JSON such as:
///
/// {
///   "type": "display.set",
///   "display": "..."
/// }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    SystemStarted {
        component: String,
    },

    DisplaySet {
        display: DisplayState,
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
}

/// State retained by bts-core.
///
/// Events describe what happened. State describes what is true now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BtsState {
    pub display: DisplayState,
}

impl Default for BtsState {
    fn default() -> Self {
        Self {
            display: DisplayState::Message {
                title: "Bansleben Telephone Services".to_owned(),
                body: "BTS Core is online".to_owned(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Messages sent from bts-core to WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "message", rename_all = "snake_case")]
pub enum ServerMessage {
    Snapshot { state: BtsState },

    Event { event: Event, state: BtsState },
}
