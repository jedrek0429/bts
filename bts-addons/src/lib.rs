//! Runtime contracts and controlled Core client for statically linked BTS addons.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use async_trait::async_trait;
use bts_protocol::addons::v1::{
    API_VERSION, ActionId, ActionRequest, Addon, AddonContext, AddonId, AddonManifest,
};
use bts_protocol::{
    AssetRef, AssetUpload, BtsState, DisplayCommand, DisplayLease, DisplayLeaseId, DisplayState,
    EventKind, NewEvent,
};
use reqwest::Client;

#[derive(Clone)]
pub struct HttpAddonContext {
    http: Client,
    core_http_url: String,
    addon_id: AddonId,
    configuration: HashMap<String, String>,
    data_directory: PathBuf,
}

impl HttpAddonContext {
    pub fn new(core_http_url: impl Into<String>, addon_id: AddonId, data_root: &Path) -> Self {
        let prefix = format!(
            "BTS_ADDON_{}_",
            addon_id.as_str().replace('-', "_").to_uppercase()
        );
        let configuration = std::env::vars()
            .filter_map(|(key, value)| key.strip_prefix(&prefix).map(|key| (key.to_owned(), value)))
            .collect();
        Self {
            http: Client::new(),
            core_http_url: core_http_url.into(),
            data_directory: data_root.join(addon_id.as_str()),
            addon_id,
            configuration,
        }
    }

    pub fn addon_id(&self) -> &AddonId {
        &self.addon_id
    }
    pub fn configuration(&self, key: &str) -> Option<&str> {
        self.configuration.get(key).map(String::as_str)
    }
    pub fn data_directory(&self) -> &Path {
        &self.data_directory
    }

    pub async fn publish(&self, kind: EventKind) -> Result<()> {
        let endpoint = format!(
            "{}{}",
            self.core_http_url.trim_end_matches('/'),
            bts_protocol::core::CORE_EVENTS_PATH
        );
        self.http
            .post(endpoint)
            .json(&NewEvent {
                source: self.addon_id.to_string(),
                kind,
            })
            .send()
            .await
            .context("failed to submit addon event to BTS Core")?
            .error_for_status()
            .context("BTS Core rejected addon event")?;
        Ok(())
    }

    pub async fn state(&self) -> Result<BtsState> {
        let endpoint = format!(
            "{}{}",
            self.core_http_url.trim_end_matches('/'),
            bts_protocol::core::CORE_STATE_PATH
        );
        self.http
            .get(endpoint)
            .send()
            .await
            .context("failed to request BTS state")?
            .error_for_status()
            .context("BTS Core rejected state request")?
            .json()
            .await
            .context("failed to decode BTS state")
    }

    pub async fn request_action(
        &self,
        action: ActionId,
        parameters: serde_json::Value,
    ) -> Result<()> {
        self.publish(EventKind::ActionRequested {
            request: ActionRequest { action, parameters },
        })
        .await
    }

    pub async fn show(&self, display: DisplayState, priority: u8) -> Result<DisplayLeaseId> {
        let lease = DisplayLease {
            id: DisplayLeaseId::new(),
            owner: self.addon_id.clone(),
            priority,
        };
        let id = lease.id;
        self.publish(EventKind::DisplayRequested {
            command: DisplayCommand::Show { lease, display },
        })
        .await?;
        Ok(id)
    }

    pub async fn update(&self, lease_id: DisplayLeaseId, display: DisplayState) -> Result<()> {
        self.publish(EventKind::DisplayRequested {
            command: DisplayCommand::Update {
                addon_id: self.addon_id.clone(),
                lease_id,
                display,
            },
        })
        .await
    }

    pub async fn release(&self, lease_id: DisplayLeaseId) -> Result<()> {
        self.publish(EventKind::DisplayRequested {
            command: DisplayCommand::Release {
                addon_id: self.addon_id.clone(),
                lease_id,
            },
        })
        .await
    }

    pub async fn release_all(&self) -> Result<()> {
        self.publish(EventKind::DisplayRequested {
            command: DisplayCommand::ReleaseAll {
                addon_id: self.addon_id.clone(),
            },
        })
        .await
    }

    pub async fn upload_asset(
        &self,
        content_type: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<AssetRef> {
        let endpoint = format!(
            "{}{}",
            self.core_http_url.trim_end_matches('/'),
            bts_protocol::core::CORE_ASSETS_PATH
        );
        self.http
            .post(endpoint)
            .json(&AssetUpload {
                addon_id: self.addon_id.clone(),
                content_type: content_type.into(),
                bytes,
            })
            .send()
            .await
            .context("failed to upload addon asset to BTS Core")?
            .error_for_status()
            .context("BTS Core rejected addon asset")?
            .json()
            .await
            .context("failed to decode BTS Core asset reference")
    }
}

#[async_trait]
impl AddonContext for HttpAddonContext {
    fn clone_box(&self) -> Box<dyn AddonContext> {
        Box::new(self.clone())
    }
    fn addon_id(&self) -> &AddonId {
        HttpAddonContext::addon_id(self)
    }
    async fn publish(&self, kind: EventKind) -> Result<()> {
        HttpAddonContext::publish(self, kind).await
    }
    async fn state(&self) -> Result<BtsState> {
        HttpAddonContext::state(self).await
    }
    async fn request_action(&self, action: ActionId, parameters: serde_json::Value) -> Result<()> {
        HttpAddonContext::request_action(self, action, parameters).await
    }
    async fn show(&self, display: DisplayState, priority: u8) -> Result<DisplayLeaseId> {
        HttpAddonContext::show(self, display, priority).await
    }
    async fn update(&self, lease_id: DisplayLeaseId, display: DisplayState) -> Result<()> {
        HttpAddonContext::update(self, lease_id, display).await
    }
    async fn release(&self, lease_id: DisplayLeaseId) -> Result<()> {
        HttpAddonContext::release(self, lease_id).await
    }
    async fn release_all(&self) -> Result<()> {
        HttpAddonContext::release_all(self).await
    }
    async fn upload_asset(&self, content_type: String, bytes: Vec<u8>) -> Result<AssetRef> {
        HttpAddonContext::upload_asset(self, content_type, bytes).await
    }
}

#[derive(Debug)]
pub struct AddonFailure {
    pub addon_id: AddonId,
    pub operation: &'static str,
    pub error: anyhow::Error,
}

impl fmt::Display for AddonFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} failed: {}",
            self.addon_id, self.operation, self.error
        )
    }
}
impl std::error::Error for AddonFailure {}

pub struct AddonRegistry {
    addons: HashMap<AddonId, Box<dyn Addon>>,
    actions: HashMap<ActionId, AddonId>,
    digits: HashMap<char, AddonId>,
}

impl AddonRegistry {
    pub fn new(addons: Vec<Box<dyn Addon>>) -> Result<Self> {
        let mut registry = Self {
            addons: HashMap::new(),
            actions: HashMap::new(),
            digits: HashMap::new(),
        };
        for addon in addons {
            registry.register(addon)?;
        }
        Ok(registry)
    }

    fn register(&mut self, addon: Box<dyn Addon>) -> Result<()> {
        let manifest = addon.manifest();
        anyhow::ensure!(
            manifest.api_version == API_VERSION,
            "unsupported addon API version for {}",
            manifest.id
        );
        anyhow::ensure!(
            !manifest.id.as_str().is_empty() && !manifest.name.trim().is_empty(),
            "addon identity and name must not be empty"
        );
        anyhow::ensure!(
            !self.addons.contains_key(&manifest.id),
            "duplicate addon ID {}",
            manifest.id
        );
        for action in &manifest.actions {
            anyhow::ensure!(
                !self.actions.contains_key(&action.id),
                "duplicate action {}",
                action.id
            );
        }
        for entry in &manifest.menu {
            anyhow::ensure!(
                entry.digit.is_ascii_digit(),
                "invalid menu digit {}",
                entry.digit
            );
            anyhow::ensure!(
                manifest
                    .actions
                    .iter()
                    .any(|action| action.id == entry.action),
                "menu digit {} refers to unregistered action {}",
                entry.digit,
                entry.action
            );
            anyhow::ensure!(
                !self.digits.contains_key(&entry.digit),
                "duplicate menu digit {}",
                entry.digit
            );
        }
        for action in &manifest.actions {
            self.actions.insert(action.id.clone(), manifest.id.clone());
        }
        for entry in &manifest.menu {
            self.digits.insert(entry.digit, manifest.id.clone());
        }
        self.addons.insert(manifest.id, addon);
        Ok(())
    }

    pub fn manifests(&self) -> Vec<AddonManifest> {
        let mut values: Vec<_> = self.addons.values().map(|addon| addon.manifest()).collect();
        values.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        values
    }

    pub fn entries(&self) -> impl Iterator<Item = (&AddonId, &dyn Addon)> {
        self.addons.iter().map(|(id, addon)| (id, addon.as_ref()))
    }
    pub fn action_owner(&self, action: &ActionId) -> Option<&AddonId> {
        self.actions.get(action)
    }
    pub fn addon(&self, id: &AddonId) -> Option<&dyn Addon> {
        self.addons.get(id).map(Box::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bts_protocol::Event;
    use bts_protocol::addons::v1::{API_VERSION, ActionRegistration, AddonVersion, MenuEntry};

    struct Stub(AddonManifest);
    #[async_trait]
    impl Addon for Stub {
        fn manifest(&self) -> AddonManifest {
            self.0.clone()
        }
        async fn handle_event(&self, _: &dyn AddonContext, _: &Event) -> Result<()> {
            Ok(())
        }
    }
    fn stub(id: &str, action: &str, digit: char) -> Box<dyn Addon> {
        Box::new(Stub(AddonManifest {
            api_version: API_VERSION,
            id: AddonId::new(id),
            name: id.into(),
            version: AddonVersion::new(1, 0, 0),
            actions: vec![ActionRegistration {
                id: ActionId::new(action),
                description: "test".into(),
            }],
            menu: vec![MenuEntry {
                digit,
                prompt: "sound:test".into(),
                action: ActionId::new(action),
                order: 1,
            }],
            capabilities: vec![],
            screens: vec![],
        }))
    }

    #[test]
    fn registry_rejects_duplicate_identity_action_and_digit() {
        assert!(
            AddonRegistry::new(vec![
                stub("one", "one.run", '1'),
                stub("one", "two.run", '2')
            ])
            .err()
            .unwrap()
            .to_string()
            .contains("duplicate addon ID")
        );
        assert!(
            AddonRegistry::new(vec![
                stub("one", "same.run", '1'),
                stub("two", "same.run", '2')
            ])
            .err()
            .unwrap()
            .to_string()
            .contains("duplicate action")
        );
        assert!(
            AddonRegistry::new(vec![
                stub("one", "one.run", '1'),
                stub("two", "two.run", '1')
            ])
            .err()
            .unwrap()
            .to_string()
            .contains("duplicate menu digit")
        );
    }
}
