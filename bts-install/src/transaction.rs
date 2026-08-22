use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::Component;

const JOURNAL: &str = "/var/lib/bts-install/transaction.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
enum PathState {
    Missing,
    File { bytes: Vec<u8>, mode: u32 },
    Symlink { target: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PathSnapshot {
    path: PathBuf,
    state: PathState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceSnapshot {
    unit: String,
    enabled: bool,
    active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Journal {
    paths: Vec<PathSnapshot>,
    services: Vec<ServiceSnapshot>,
}

pub struct HostTransaction {
    root: PathBuf,
    journal_path: PathBuf,
    journal: Journal,
}

impl HostTransaction {
    pub fn begin(root: &Path) -> Result<Self> {
        let journal_path = rooted(root, JOURNAL);
        if journal_path.exists() {
            anyhow::bail!(
                "An unfinished installer transaction exists. Re-run bts-install to recover it before starting another operation."
            );
        }
        let paths = managed_paths(root)
            .into_iter()
            .map(capture)
            .collect::<Result<Vec<_>>>()?;
        let services = if root == Path::new("/") {
            managed_units().into_iter().map(snapshot_service).collect()
        } else {
            Vec::new()
        };
        let journal = Journal { paths, services };
        write_journal(&journal_path, &journal)?;
        Ok(Self {
            root: root.to_owned(),
            journal_path,
            journal,
        })
    }

    pub fn commit(self) -> Result<()> {
        remove_journal(&self.journal_path)
    }

    pub fn rollback(self) -> Result<()> {
        restore(&self.root, &self.journal)?;
        remove_journal(&self.journal_path)
    }
}

/// Commits an operation whose host mutations succeeded and whose final state
/// has subsequently been persisted by the caller.
pub fn commit_pending(root: &Path) -> Result<()> {
    remove_journal(&rooted(root, JOURNAL))
}

/// Restores a transaction interrupted by process termination or a host reboot.
/// Returns true when recovery was performed.
pub fn recover_pending(root: &Path) -> Result<bool> {
    let path = rooted(root, JOURNAL);
    if !path.is_file() {
        return Ok(false);
    }
    let journal: Journal = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("Could not read {}", path.display()))?,
    )
    .context("Installer transaction journal is invalid")?;
    restore(root, &journal).context("Could not recover the interrupted installer transaction")?;
    remove_journal(&path)?;
    Ok(true)
}

pub fn pending(root: &Path) -> bool {
    rooted(root, JOURNAL).is_file()
}

fn managed_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        rooted(root, "/var/lib/bts-install/state.json"),
        rooted(root, "/etc/bts/bts.env"),
        rooted(root, "/etc/systemd/system/getty@tty1.service"),
        rooted(
            root,
            "/etc/systemd/system/getty.target.wants/getty@tty1.service",
        ),
        rooted(root, "/usr/share/licenses/bts/LICENSE"),
        rooted(root, "/usr/bin/btscli"),
    ];
    for component in Component::ALL {
        if let Some(config) = component.config_name() {
            paths.push(rooted(root, &format!("/etc/bts/{config}")));
        }
        if let Some(unit) = component.unit() {
            paths.push(rooted(root, &format!("/usr/lib/systemd/system/{unit}")));
        }
        paths.push(rooted(
            root,
            &format!("/usr/lib/bts/components/{component}/current"),
        ));
    }
    paths
}

fn managed_units() -> Vec<String> {
    let mut units = Component::ALL
        .into_iter()
        .filter_map(Component::unit)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    units.push("getty@tty1.service".into());
    units.push("seatd.service".into());
    units
}

fn capture(path: PathBuf) -> Result<PathSnapshot> {
    let state = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => PathState::Symlink {
            target: fs::read_link(&path)?,
        },
        Ok(metadata) if metadata.is_file() => PathState::File {
            bytes: fs::read(&path)?,
            mode: metadata.permissions().mode(),
        },
        Ok(_) => PathState::Missing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => PathState::Missing,
        Err(error) => return Err(error.into()),
    };
    Ok(PathSnapshot { path, state })
}

fn snapshot_service(unit: String) -> ServiceSnapshot {
    ServiceSnapshot {
        enabled: command_success("systemctl", &["is-enabled", &unit]),
        active: command_success("systemctl", &["is-active", &unit]),
        unit,
    }
}

fn restore(root: &Path, journal: &Journal) -> Result<()> {
    for snapshot in journal.paths.iter().rev() {
        restore_path(snapshot)?;
    }
    if root == Path::new("/") {
        let _ = Command::new("systemctl").arg("daemon-reload").status();
        for service in &journal.services {
            let enable_verb = if service.enabled { "enable" } else { "disable" };
            let active_verb = if service.active { "start" } else { "stop" };
            let _ = Command::new("systemctl")
                .args([enable_verb, &service.unit])
                .status();
            let _ = Command::new("systemctl")
                .args([active_verb, &service.unit])
                .status();
        }
    }
    Ok(())
}

fn restore_path(snapshot: &PathSnapshot) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(&snapshot.path) {
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            return Ok(());
        }
        fs::remove_file(&snapshot.path)?;
    }
    match &snapshot.state {
        PathState::Missing => Ok(()),
        PathState::File { bytes, mode } => {
            if let Some(parent) = snapshot.path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&snapshot.path, bytes)?;
            fs::set_permissions(&snapshot.path, fs::Permissions::from_mode(*mode))?;
            Ok(())
        }
        PathState::Symlink { target } => {
            if let Some(parent) = snapshot.path.parent() {
                fs::create_dir_all(parent)?;
            }
            symlink(target, &snapshot.path)?;
            Ok(())
        }
    }
}

fn write_journal(path: &Path, journal: &Journal) -> Result<()> {
    let parent = path.parent().context("Transaction journal has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".transaction.{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec_pretty(journal)?)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn remove_journal(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn command_success(program: &str, arguments: &[&str]) -> bool {
    Command::new(program)
        .args(arguments)
        .status()
        .is_ok_and(|status| status.success())
}

fn rooted(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_restores_files_links_and_missing_paths() {
        let root = tempfile::tempdir().unwrap();
        let config = rooted(root.path(), "/etc/bts/display.env");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "before").unwrap();
        let current = rooted(root.path(), "/usr/lib/bts/components/display/current");
        fs::create_dir_all(current.parent().unwrap()).unwrap();
        symlink("releases/old", &current).unwrap();

        let transaction = HostTransaction::begin(root.path()).unwrap();
        fs::write(&config, "after").unwrap();
        fs::remove_file(&current).unwrap();
        symlink("releases/new", &current).unwrap();
        let new_config = rooted(root.path(), "/etc/bts/core.env");
        fs::write(&new_config, "new").unwrap();

        transaction.rollback().unwrap();
        assert_eq!(fs::read_to_string(config).unwrap(), "before");
        assert_eq!(
            fs::read_link(current).unwrap(),
            PathBuf::from("releases/old")
        );
        assert!(!new_config.exists());
        assert!(!pending(root.path()));
    }

    #[test]
    fn interrupted_transaction_is_recovered_on_next_invocation() {
        let root = tempfile::tempdir().unwrap();
        let config = rooted(root.path(), "/etc/bts/display.env");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "before").unwrap();
        let transaction = HostTransaction::begin(root.path()).unwrap();
        fs::write(&config, "partial").unwrap();
        std::mem::forget(transaction);

        assert!(recover_pending(root.path()).unwrap());
        assert_eq!(fs::read_to_string(config).unwrap(), "before");
    }

    #[test]
    fn committing_removes_the_durable_journal() {
        let root = tempfile::tempdir().unwrap();
        let transaction = HostTransaction::begin(root.path()).unwrap();
        assert!(pending(root.path()));
        transaction.commit().unwrap();
        assert!(!pending(root.path()));
    }
}
