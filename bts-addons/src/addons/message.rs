use anyhow::Result;
use async_trait::async_trait;
use bts_addons::{
    ADDON_API_VERSION, ActionKind, Addon, AddonCapability, AddonContext, AddonId, AddonManifest,
    AddonVersion,
};
use bts_protocol::{Action, DisplayState, Event, EventKind};

const ADDON_ID: &str = "message";

pub(crate) struct MessageAddon;

#[async_trait]
impl Addon for MessageAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: ADDON_API_VERSION,
            id: AddonId::new(ADDON_ID),
            name: "Message Service".to_owned(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionKind::Message, ActionKind::Blank],
            capabilities: vec![AddonCapability::PublishEvents],
        }
    }

    async fn handle_event(&self, context: &AddonContext, event: &Event) -> Result<()> {
        let EventKind::ActionRequested { action } = &event.kind else {
            return Ok(());
        };

        match action {
            Action::Message { title, body } => show(context, title, body).await,
            Action::Blank => {
                context
                    .publish(
                        &AddonId::new(ADDON_ID),
                        EventKind::DisplaySet {
                            display: DisplayState::Blank,
                        },
                    )
                    .await
            }
            Action::Clock | Action::Weather => Ok(()),
        }
    }
}

pub(crate) async fn show(
    context: &AddonContext,
    title: impl Into<String>,
    body: impl Into<String>,
) -> Result<()> {
    context
        .publish(
            &AddonId::new(ADDON_ID),
            EventKind::DisplaySet {
                display: DisplayState::Message {
                    title: title.into(),
                    body: body.into(),
                },
            },
        )
        .await
}
