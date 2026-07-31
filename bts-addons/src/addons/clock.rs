use anyhow::Result;
use async_trait::async_trait;
use bts_protocol::addons::v1::{
    API_VERSION, ActionId, ActionRegistration, Addon, AddonCapability, AddonContext, AddonId,
    AddonManifest, MenuEntry,
};
use bts_protocol::{DisplayLeaseId, DisplayState, Event, EventKind, ScreenKind};
use chrono::Local;
use std::time::Duration;
use tokio::{
    sync::Mutex,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

use super::addon_version;

pub(crate) const ID: &str = "clock";
pub(crate) const ACTION: &str = "clock.show";

pub(crate) struct ClockAddon {
    task: Mutex<Option<JoinHandle<()>>>,
    lease: Mutex<Option<DisplayLeaseId>>,
}
impl ClockAddon {
    pub(crate) fn new() -> Self {
        Self {
            task: Mutex::new(None),
            lease: Mutex::new(None),
        }
    }
}

#[async_trait]
impl Addon for ClockAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new(ID),
            name: "Clock Service".into(),
            version: addon_version(bts_compat::CLOCK_ADDON_VERSION),
            actions: vec![ActionRegistration {
                id: ActionId::new(ACTION),
                description: "Show the clock".into(),
            }],
            menu: vec![MenuEntry {
                digit: '2',
                prompt: "sound:bts/press-2-time".into(),
                action: ActionId::new(ACTION),
                order: 20,
            }],
            capabilities: vec![AddonCapability::Display],
            screens: vec![ScreenKind::Clock],
        }
    }

    async fn handle_event(&self, context: &dyn AddonContext, event: &Event) -> Result<()> {
        let EventKind::ActionRequested { request } = &event.kind else {
            return Ok(());
        };
        if request.action.as_str() != ACTION {
            return Ok(());
        }
        self.stop(context).await?;
        let lease = context.show(clock_state(), 10).await?;
        *self.lease.lock().await = Some(lease);
        let context = context.clone_box();
        let task = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if context.update(lease, clock_state()).await.is_err() {
                    break;
                }
            }
        });
        *self.task.lock().await = Some(task);
        Ok(())
    }

    async fn stop(&self, context: &dyn AddonContext) -> Result<()> {
        if let Some(task) = self.task.lock().await.take() {
            task.abort();
        }
        if let Some(lease) = self.lease.lock().await.take() {
            let _ = context.release(lease).await;
        }
        Ok(())
    }
}

fn clock_state() -> DisplayState {
    let now = Local::now();
    DisplayState::Clock {
        time: now.format("%H:%M").to_string(),
        seconds: now.format("%S").to_string(),
        date: now.format("%A, %-d %B %Y").to_string(),
    }
}
