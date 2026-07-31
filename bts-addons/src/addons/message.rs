use anyhow::Result;
use async_trait::async_trait;
use bts_protocol::addons::v1::{
    API_VERSION, ActionId, ActionRegistration, Addon, AddonCapability, AddonContext, AddonId,
    AddonManifest, MenuEntry,
};
use bts_protocol::{DisplayState, Event, EventKind, ScreenKind};

use super::addon_version;

pub(crate) const ID: &str = "message";
pub(crate) const SHOW: &str = "message.show";
pub(crate) const BLANK: &str = "display.blank";
pub(crate) struct MessageAddon;

#[async_trait]
impl Addon for MessageAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new(ID),
            name: "Message Service".into(),
            version: addon_version(bts_compat::MESSAGE_ADDON_VERSION),
            actions: vec![
                ActionRegistration {
                    id: ActionId::new(SHOW),
                    description: "Show a message".into(),
                },
                ActionRegistration {
                    id: ActionId::new(BLANK),
                    description: "Blank the display".into(),
                },
            ],
            menu: vec![MenuEntry {
                digit: '0',
                prompt: "sound:bts/press-0-clear".into(),
                action: ActionId::new(BLANK),
                order: 90,
            }],
            capabilities: vec![AddonCapability::Display],
            screens: vec![ScreenKind::Message, ScreenKind::Blank],
        }
    }
    async fn handle_event(&self, context: &dyn AddonContext, event: &Event) -> Result<()> {
        let EventKind::ActionRequested { request } = &event.kind else {
            return Ok(());
        };
        match request.action.as_str() {
            SHOW => {
                let title = request
                    .parameters
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("BTS service");
                let body = request
                    .parameters
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                context
                    .show(
                        DisplayState::Message {
                            title: title.into(),
                            body: body.into(),
                        },
                        10,
                    )
                    .await?;
            }
            BLANK => {
                context.show(DisplayState::Blank, 10).await?;
            }
            _ => {}
        }
        Ok(())
    }
}
