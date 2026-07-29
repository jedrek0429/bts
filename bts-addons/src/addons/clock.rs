use anyhow::Result;
use bts_protocol::{DisplayState, Event, EventKind};
use chrono::Local;

use crate::{AddonContext, addons::requested_digit};

const DIGIT: &str = "2";

pub(crate) async fn handle(context: &AddonContext, event: &Event) -> Result<()> {
    if requested_digit(event) != Some(DIGIT) {
        return Ok(());
    }

    let now = Local::now();

    context
        .publish(EventKind::DisplaySet {
            display: DisplayState::Clock {
                time: now.format("%H:%M").to_string(),
                seconds: now.format("%S").to_string(),
                date: now.format("%A, %-d %B %Y").to_string(),
            },
        })
        .await
}
