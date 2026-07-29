use std::time::Duration;

use anyhow::Result;
use bts_protocol::{DisplayState, EventKind};
use chrono::Local;
use tokio::{
    sync::Mutex,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};
use tracing::warn;

use crate::AddonContext;

const UPDATE_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct ClockAddon {
    update_task: Mutex<Option<JoinHandle<()>>>,
}

impl ClockAddon {
    pub(crate) fn new() -> Self {
        Self {
            update_task: Mutex::new(None),
        }
    }

    pub(crate) async fn show(&self, context: &AddonContext) -> Result<()> {
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

async fn publish_clock(context: &AddonContext) -> Result<()> {
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

async fn clock_is_active(context: &AddonContext) -> Result<bool> {
    Ok(matches!(
        context.state().await?.display,
        DisplayState::Clock { .. }
    ))
}
