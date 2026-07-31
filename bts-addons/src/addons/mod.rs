pub(crate) mod clock;
pub(crate) mod message;
pub(crate) mod weather;

use anyhow::Result;
use async_trait::async_trait;
use bts_addons::{
    ADDON_API_VERSION, ActionKind, Addon, AddonCapability, AddonContext, AddonFailure, AddonId,
    AddonManifest, AddonVersion,
};
use bts_protocol::{Action, Event, EventKind};

pub(crate) struct Addons {
    addons: Vec<Box<dyn Addon>>,
}

impl Addons {
    pub(crate) fn new() -> Self {
        Self {
            addons: vec![
                Box::new(DtmfAddon),
                Box::new(clock::ClockAddon::new()),
                Box::new(weather::WeatherAddon::new()),
                Box::new(message::MessageAddon),
            ],
        }
    }

    pub(crate) async fn start(&self, context: &AddonContext) -> Vec<AddonFailure> {
        let mut failures = Vec::new();

        for addon in &self.addons {
            if let Err(error) = addon.start(context).await {
                failures.push(failure(addon.as_ref(), "start", error));
            }
        }

        failures
    }

    pub(crate) async fn handle(&self, context: &AddonContext, event: &Event) -> Vec<AddonFailure> {
        let mut failures = Vec::new();

        for addon in &self.addons {
            if let Err(error) = addon.handle_event(context, event).await {
                failures.push(failure(addon.as_ref(), "event handling", error));
            }
        }

        failures
    }

    pub(crate) async fn stop(&self, context: &AddonContext) -> Vec<AddonFailure> {
        let mut failures = Vec::new();

        for addon in self.addons.iter().rev() {
            if let Err(error) = addon.stop(context).await {
                failures.push(failure(addon.as_ref(), "stop", error));
            }
        }

        failures
    }
}

fn failure(addon: &dyn Addon, operation: &'static str, error: anyhow::Error) -> AddonFailure {
    AddonFailure {
        addon_id: addon.manifest().id,
        operation,
        error,
    }
}

struct DtmfAddon;

#[async_trait]
impl Addon for DtmfAddon {
    fn manifest(&self) -> AddonManifest {
        AddonManifest {
            api_version: ADDON_API_VERSION,
            id: AddonId::new("dtmf-actions"),
            name: "Telephone Actions".to_owned(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionKind::Clock, ActionKind::Weather, ActionKind::Blank],
            capabilities: vec![AddonCapability::PublishEvents],
        }
    }

    async fn handle_event(&self, context: &AddonContext, event: &Event) -> Result<()> {
        let EventKind::PhoneDtmfReceived { digit, .. } = &event.kind else {
            return Ok(());
        };

        if let Some(action) = action_for_digit(digit) {
            context
                .publish(&self.manifest().id, EventKind::ActionRequested { action })
                .await?;
        }

        Ok(())
    }
}

fn action_for_digit(digit: &str) -> Option<Action> {
    match digit {
        "2" => Some(Action::Clock),
        "3" => Some(Action::Weather),
        "0" => Some(Action::Blank),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    struct TestAddon {
        id: &'static str,
        calls: Arc<AtomicUsize>,
        fails: bool,
    }

    #[async_trait]
    impl Addon for TestAddon {
        fn manifest(&self) -> AddonManifest {
            AddonManifest {
                api_version: ADDON_API_VERSION,
                id: AddonId::new(self.id),
                name: self.id.to_owned(),
                version: AddonVersion::new(1, 0, 0),
                actions: Vec::new(),
                capabilities: Vec::new(),
            }
        }

        async fn handle_event(&self, _context: &AddonContext, _event: &Event) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                anyhow::bail!("expected failure");
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn one_addon_failure_does_not_stop_the_host() {
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let addons = Addons {
            addons: vec![
                Box::new(TestAddon {
                    id: "failing",
                    calls: first_calls.clone(),
                    fails: true,
                }),
                Box::new(TestAddon {
                    id: "healthy",
                    calls: second_calls.clone(),
                    fails: false,
                }),
            ],
        };
        let context = AddonContext::new("http://127.0.0.1:1");
        let event = Event::new(
            "test",
            EventKind::ActionRequested {
                action: Action::Blank,
            },
        );

        let failures = addons.handle(&context, &event).await;

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].addon_id, AddonId::new("failing"));
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn telephone_digits_map_to_declared_actions() {
        assert!(matches!(action_for_digit("2"), Some(Action::Clock)));
        assert!(matches!(action_for_digit("3"), Some(Action::Weather)));
        assert!(matches!(action_for_digit("0"), Some(Action::Blank)));
        assert!(action_for_digit("9").is_none());
    }
}
