use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use bts_addons::{
    ADDON_API_VERSION, ActionKind, Addon, AddonCapability, AddonContext, AddonId, AddonManifest,
    AddonVersion,
};
use bts_protocol::{Action, DisplayState, Event, EventKind};
use chrono::Local;
use tokio::{
    sync::Mutex,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tracing::warn;

const UPDATE_INTERVAL: Duration = Duration::from_secs(1);
const ADDON_ID: &str = "clock";

pub(crate) struct ClockAddon {
    update_task: Mutex<Option<JoinHandle<()>>>,
}

impl ClockAddon {
    pub(crate) fn new() -> Self {
        Self {
            update_task: Mutex::new(None),
        }
    }

    async fn show(&self, context: &AddonContext) -> Result<()> {
        self.stop_update_task().await;
        publish_clock(context).await?;

        let context = context.clone();
        let task = tokio::spawn(async move {
            let mut ticker = interval(UPDATE_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

            ticker.tick().await;

            loop {
                ticker.tick().await;

                match clock_is_active(&context).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        warn!(%error, "failed to check active display for clock update");
                        continue;
                    }
                }

                if let Err(error) = publish_clock(&context).await {
                    warn!(%error, "failed to publish clock update");
                }
            }
        });

        *self.update_task.lock().await = Some(task);
        Ok(())
    }

    async fn stop_update_task(&self) {
        if let Some(task) = self.update_task.lock().await.take() {
            task.abort();
        }
    }
}

#[async_trait]
impl Addon for ClockAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: ADDON_API_VERSION,
            id: AddonId::new(ADDON_ID),
            name: "Clock Service".to_owned(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionKind::Clock],
            capabilities: vec![AddonCapability::PublishEvents, AddonCapability::ReadState],
        }
    }

    async fn handle_event(&self, context: &AddonContext, event: &Event) -> Result<()> {
        if matches!(
            &event.kind,
            EventKind::ActionRequested {
                action: Action::Clock
            }
        ) {
            self.show(context).await?;
        }

        Ok(())
    }

    async fn stop(&self, _context: &AddonContext) -> Result<()> {
        self.stop_update_task().await;
        Ok(())
    }
}

async fn publish_clock(context: &AddonContext) -> Result<()> {
    let now = Local::now();

    context
        .publish(
            &AddonId::new(ADDON_ID),
            EventKind::DisplaySet {
                display: DisplayState::Clock {
                    time: now.format("%H:%M").to_string(),
                    seconds: now.format("%S").to_string(),
                    date: now.format("%A, %-d %B %Y").to_string(),
                },
            },
        )
        .await
}

async fn clock_is_active(context: &AddonContext) -> Result<bool> {
    Ok(matches!(
        context.state().await?.display,
        DisplayState::Clock { .. }
    ))
}
