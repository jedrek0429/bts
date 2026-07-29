use anyhow::Result;
use bts_protocol::{DisplayState, Event, EventKind};

use crate::AddonContext;

pub(crate) async fn handle(_context: &AddonContext, _event: &Event) -> Result<()> {
    Ok(())
}

pub(crate) async fn show(
    context: &AddonContext,
    title: impl Into<String>,
    body: impl Into<String>,
) -> Result<()> {
    context
        .publish(EventKind::DisplaySet {
            display: DisplayState::Message {
                title: title.into(),
                body: body.into(),
            },
        })
        .await
}
