use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_CHANNEL, DEFAULT_REPOSITORY,
    model::{Component, Role},
    platform::{Architecture, Platform},
};

pub const STATE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallerState {
    pub schema_version: u32,
    pub installed_version: String,
    pub installer_version: String,
    pub selected_role: Option<Role>,
    pub installed_components: BTreeSet<Component>,
    #[serde(default)]
    pub component_versions: BTreeMap<Component, String>,
    pub repository: String,
    pub release_channel: String,
    pub platform: Platform,
    pub architecture: Architecture,
    pub installed_at: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub tty1_managed: bool,
}

impl InstallerState {
    pub fn new(version: impl Into<String>, platform: Platform, architecture: Architecture) -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            installed_version: version.into(),
            installer_version: crate::INSTALLER_VERSION.into(),
            selected_role: None,
            installed_components: BTreeSet::new(),
            component_versions: BTreeMap::new(),
            repository: DEFAULT_REPOSITORY.into(),
            release_channel: DEFAULT_CHANNEL.into(),
            platform,
            architecture,
            installed_at: unix_timestamp().to_string(),
            updated_at: None,
            tty1_managed: false,
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Could not read installer state at {}", path.display())
                });
            }
        };
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("Installer state is not valid JSON")?;
        let schema = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(1);
        let mut state = match schema {
            1 => migrate_v1(value)?,
            2 => serde_json::from_value(value).context("Installer state schema 2 is invalid")?,
            other => bail!("Installer state schema {other} is newer than this installer supports."),
        };
        state.schema_version = STATE_SCHEMA_VERSION;
        if state.component_versions.is_empty() {
            state.component_versions = state
                .installed_components
                .iter()
                .map(|component| (*component, state.installed_version.clone()))
                .collect();
        }
        Ok(Some(state))
    }

    pub fn write_atomic(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .context("State path has no parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("Could not create {}", parent.display()))?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o750))?;
        let temporary = temporary_path(path);
        let result = (|| -> Result<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)
                .with_context(|| format!("Could not create {}", temporary.display()))?;
            serde_json::to_writer_pretty(&mut file, self)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn migrate_v1(value: serde_json::Value) -> Result<InstallerState> {
    #[derive(Deserialize)]
    struct V1 {
        installed_version: String,
        #[serde(default)]
        role: Option<Role>,
        #[serde(default)]
        components: BTreeSet<Component>,
        #[serde(default = "default_repository")]
        repository: String,
        #[serde(default = "default_channel")]
        channel: String,
        platform: Platform,
        architecture: Architecture,
        installed_at: String,
    }
    let old: V1 = serde_json::from_value(value).context("Installer state schema 1 is invalid")?;
    Ok(InstallerState {
        schema_version: STATE_SCHEMA_VERSION,
        installed_version: old.installed_version,
        installer_version: crate::INSTALLER_VERSION.into(),
        selected_role: old.role,
        installed_components: old.components,
        component_versions: BTreeMap::new(),
        repository: old.repository,
        release_channel: old.channel,
        platform: old.platform,
        architecture: old.architecture,
        installed_at: old.installed_at,
        updated_at: None,
        tty1_managed: false,
    })
}

fn default_repository() -> String {
    DEFAULT_REPOSITORY.into()
}
fn default_channel() -> String {
    DEFAULT_CHANNEL.into()
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".tmp.{}", std::process::id()));
    PathBuf::from(name)
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn state_round_trips_with_atomic_permissions() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state/state.json");
        let mut state = InstallerState::new("0.3.1", Platform::Debian, Architecture::Aarch64);
        state.installed_components.insert(Component::Display);
        state
            .component_versions
            .insert(Component::Display, "0.3.1".into());
        state.write_atomic(&path).unwrap();
        assert_eq!(InstallerState::load(&path).unwrap(), Some(state));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!temporary_path(&path).exists());
    }

    #[test]
    fn migrates_schema_one_without_secrets() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, r#"{"schema_version":1,"installed_version":"0.3.0","role":"display","components":["display"],"platform":"debian","architecture":"aarch64","installed_at":"now"}"#).unwrap();
        let state = InstallerState::load(&path).unwrap().unwrap();
        assert_eq!(state.schema_version, STATE_SCHEMA_VERSION);
        assert_eq!(state.selected_role, Some(Role::Display));
        assert!(!serde_json::to_string(&state).unwrap().contains("password"));
    }

    #[test]
    fn rejects_future_schema() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(&path, r#"{"schema_version":99}"#).unwrap();
        assert!(
            InstallerState::load(&path)
                .unwrap_err()
                .to_string()
                .contains("newer")
        );
    }
}
