pub(crate) mod clock;
pub(crate) mod message;
pub(crate) mod weather;

use bts_addons::{Addon, AddonContext, AddonFailure, AddonRegistry};
use bts_protocol::{AddonId, Event, EventKind};
use std::path::PathBuf;

pub(crate) struct Addons {
    registry: AddonRegistry,
    core_url: String,
    data_root: PathBuf,
}

impl Addons {
    pub(crate) fn new(core_url: String, data_root: PathBuf) -> anyhow::Result<Self> {
        let addons: Vec<Box<dyn Addon>> = vec![
            Box::new(clock::ClockAddon::new()),
            Box::new(weather::WeatherAddon::new()),
            Box::new(message::MessageAddon),
        ];
        Ok(Self {
            registry: AddonRegistry::new(addons)?,
            core_url,
            data_root,
        })
    }
    fn context(&self, id: &AddonId) -> AddonContext {
        AddonContext::new(&self.core_url, id.clone(), &self.data_root)
    }
    pub(crate) async fn start(&self) -> Vec<AddonFailure> {
        let mut failures = Vec::new();
        for (id, addon) in self.registry.entries() {
            let context = self.context(id);
            let manifest = addon.manifest();
            // Clear a registration and display lease left by an ungraceful host
            // restart before registering the fresh addon instance.
            let _ = context
                .publish(EventKind::AddonStopped {
                    addon_id: id.clone(),
                })
                .await;
            if let Err(error) = context
                .publish(EventKind::AddonRegistered { manifest })
                .await
            {
                failures.push(failure(id, "registration", error));
                continue;
            }
            if let Err(error) = addon.start(&context).await {
                failures.push(failure(id, "start", error));
            }
        }
        failures
    }
    pub(crate) async fn handle(&self, event: &Event) -> Vec<AddonFailure> {
        let targets: Vec<_> = match &event.kind {
            EventKind::ActionRequested { request } => self
                .registry
                .action_owner(&request.action)
                .into_iter()
                .cloned()
                .collect(),
            _ => self.registry.entries().map(|(id, _)| id.clone()).collect(),
        };
        let mut failures = Vec::new();
        for id in targets {
            let addon = self
                .registry
                .addon(&id)
                .expect("registered addon disappeared");
            if let Err(error) = addon.handle_event(&self.context(&id), event).await {
                failures.push(failure(&id, "event handling", error));
            }
        }
        failures
    }
    pub(crate) async fn stop(&self) -> Vec<AddonFailure> {
        let mut failures = Vec::new();
        for (id, addon) in self.registry.entries() {
            let context = self.context(id);
            if let Err(error) = addon.stop(&context).await {
                failures.push(failure(id, "stop", error));
            }
            let _ = context.release_all().await;
            let _ = context
                .publish(EventKind::AddonStopped {
                    addon_id: id.clone(),
                })
                .await;
        }
        failures
    }
}
fn failure(id: &AddonId, operation: &'static str, error: anyhow::Error) -> AddonFailure {
    AddonFailure {
        addon_id: id.clone(),
        operation,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use async_trait::async_trait;
    use bts_protocol::{
        ADDON_API_VERSION, ActionId, ActionRegistration, ActionRequest, AddonManifest, AddonVersion,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct TestAddon {
        manifest: AddonManifest,
        calls: Arc<AtomicUsize>,
        fails: bool,
    }
    #[async_trait]
    impl Addon for TestAddon {
        fn manifest(&self) -> AddonManifest {
            self.manifest.clone()
        }
        async fn handle_event(&self, _: &AddonContext, _: &Event) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fails {
                anyhow::bail!("failure")
            }
            Ok(())
        }
    }
    fn addon(id: &str, action: &str, calls: Arc<AtomicUsize>, fails: bool) -> Box<dyn Addon> {
        Box::new(TestAddon {
            manifest: AddonManifest {
                api_version: ADDON_API_VERSION,
                id: AddonId::new(id),
                name: id.into(),
                version: AddonVersion::new(1, 0, 0),
                actions: vec![ActionRegistration {
                    id: ActionId::new(action),
                    description: "test".into(),
                }],
                menu: vec![],
                capabilities: vec![],
                screens: vec![],
            },
            calls,
            fails,
        })
    }
    #[tokio::test]
    async fn action_dispatches_only_to_owner() {
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));
        let host = Addons {
            registry: AddonRegistry::new(vec![
                addon("a", "a.run", a.clone(), false),
                addon("b", "b.run", b.clone(), false),
            ])
            .unwrap(),
            core_url: "http://127.0.0.1:1".into(),
            data_root: PathBuf::new(),
        };
        let event = Event::new(
            "test",
            EventKind::ActionRequested {
                request: ActionRequest {
                    action: ActionId::new("b.run"),
                    parameters: serde_json::Value::Null,
                },
            },
        );
        assert!(host.handle(&event).await.is_empty());
        assert_eq!(a.load(Ordering::SeqCst), 0);
        assert_eq!(b.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn failure_does_not_stop_broadcast() {
        let a = Arc::new(AtomicUsize::new(0));
        let b = Arc::new(AtomicUsize::new(0));
        let host = Addons {
            registry: AddonRegistry::new(vec![
                addon("a", "a.run", a.clone(), true),
                addon("b", "b.run", b.clone(), false),
            ])
            .unwrap(),
            core_url: "http://127.0.0.1:1".into(),
            data_root: PathBuf::new(),
        };
        let event = Event::new(
            "test",
            EventKind::SystemStarted {
                component: "test".into(),
            },
        );
        assert_eq!(host.handle(&event).await.len(), 1);
        assert_eq!(a.load(Ordering::SeqCst), 1);
        assert_eq!(b.load(Ordering::SeqCst), 1);
    }
}
