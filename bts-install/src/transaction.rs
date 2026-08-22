use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
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
    File {
        bytes: Vec<u8>,
        mode: u32,
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        gid: Option<u32>,
    },
    Directory {
        mode: u32,
        #[serde(default)]
        uid: Option<u32>,
        #[serde(default)]
        gid: Option<u32>,
    },
    Symlink {
        target: PathBuf,
    },
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
        })
    }

    pub fn commit(self) -> Result<()> {
        remove_journal(&self.journal_path)
    }

    pub fn rollback(self) -> Result<()> {
        let journal = read_journal(&self.journal_path)?;
        restore(&self.root, &journal)?;
        remove_journal(&self.journal_path)
    }
}

/// Adds an absolute installer-owned path to the durable transaction before it
/// is first mutated. This covers paths derived from release metadata or
/// configuration after the transaction has started.
pub fn track_path(root: &Path, absolute: &Path) -> Result<bool> {
    anyhow::ensure!(
        absolute.is_absolute(),
        "Tracked installer path must be absolute."
    );
    anyhow::ensure!(
        !absolute
            .components()
            .any(|part| part == std::path::Component::ParentDir),
        "Tracked installer path must not contain '..'."
    );
    let journal_path = rooted_path(root, absolute);
    let transaction_path = rooted(root, JOURNAL);
    if !transaction_path.is_file() {
        return Ok(false);
    }
    let mut journal = read_journal(&transaction_path)?;
    if journal
        .paths
        .iter()
        .any(|snapshot| snapshot.path == journal_path)
    {
        return Ok(false);
    }
    journal.paths.push(capture(journal_path)?);
    write_journal(&transaction_path, &journal)?;
    Ok(true)
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
    let journal = read_journal(&path)?;
    restore(root, &journal).context("Could not recover the interrupted installer transaction")?;
    remove_journal(&path)?;
    Ok(true)
}

pub fn pending(root: &Path) -> bool {
    rooted(root, JOURNAL).is_file()
}

fn managed_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        rooted(root, "/var/lib/bts-install"),
        rooted(root, "/var/lib/bts-install/state.json"),
        rooted(root, "/etc/bts/bts.env"),
        rooted(root, "/etc/systemd/system/getty@tty1.service"),
        rooted(
            root,
            "/etc/systemd/system/getty.target.wants/getty@tty1.service",
        ),
        rooted(root, "/usr/share/licenses/bts/LICENSE"),
        rooted(root, "/usr/bin/btscli"),
        rooted(root, "/var/lib/asterisk/sounds/en/bts-generated"),
        rooted(root, "/usr/lib/systemd/system/bts.target"),
        rooted(root, "/usr/lib/systemd/system/bts-server.target"),
        rooted(root, "/usr/lib/systemd/system/bts-display.target"),
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
            uid: Some(metadata.uid()),
            gid: Some(metadata.gid()),
        },
        Ok(metadata) if metadata.is_dir() => PathState::Directory {
            mode: metadata.permissions().mode(),
            uid: Some(metadata.uid()),
            gid: Some(metadata.gid()),
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
    let mut failures = Vec::new();
    for snapshot in journal.paths.iter().rev() {
        if let Err(error) = restore_path(snapshot) {
            failures.push(format!("{}: {error:#}", snapshot.path.display()));
        }
    }
    if root == Path::new("/")
        && let Err(error) = restore_services_with(&journal.services, |arguments| {
            let status = Command::new("systemctl")
                .args(arguments)
                .status()
                .with_context(|| format!("Could not run systemctl {}", arguments.join(" ")))?;
            anyhow::ensure!(
                status.success(),
                "systemctl {} exited with {status}",
                arguments.join(" ")
            );
            Ok(())
        })
    {
        failures.push(error.to_string());
    }
    anyhow::ensure!(
        failures.is_empty(),
        "Rollback could not restore all installer-owned state: {}",
        failures.join("; ")
    );
    Ok(())
}

fn restore_services_with<F>(services: &[ServiceSnapshot], mut run: F) -> Result<()>
where
    F: FnMut(&[&str]) -> Result<()>,
{
    let mut failures = Vec::new();
    attempt_service_restore(
        &mut run,
        &["daemon-reload"],
        "systemd manager",
        &mut failures,
    );
    for service in services {
        let enable_verb = if service.enabled { "enable" } else { "disable" };
        let active_verb = if service.active { "start" } else { "stop" };
        attempt_service_restore(
            &mut run,
            &[enable_verb, &service.unit],
            &service.unit,
            &mut failures,
        );
        attempt_service_restore(
            &mut run,
            &[active_verb, &service.unit],
            &service.unit,
            &mut failures,
        );
    }
    anyhow::ensure!(
        failures.is_empty(),
        "Could not restore service state: {}",
        failures.join("; ")
    );
    Ok(())
}

fn attempt_service_restore<F>(
    run: &mut F,
    arguments: &[&str],
    subject: &str,
    failures: &mut Vec<String>,
) where
    F: FnMut(&[&str]) -> Result<()>,
{
    if let Err(error) = run(arguments) {
        failures.push(format!("{subject}: {error:#}"));
    }
}

fn restore_path(snapshot: &PathSnapshot) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(&snapshot.path) {
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            if matches!(
                snapshot.state,
                PathState::Missing | PathState::File { .. } | PathState::Symlink { .. }
            ) {
                fs::remove_dir_all(&snapshot.path)?;
            }
        } else {
            fs::remove_file(&snapshot.path)?;
        }
    }
    match &snapshot.state {
        PathState::Missing => Ok(()),
        PathState::File {
            bytes,
            mode,
            uid,
            gid,
        } => {
            if let Some(parent) = snapshot.path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&snapshot.path, bytes)?;
            fs::set_permissions(&snapshot.path, fs::Permissions::from_mode(*mode))?;
            restore_ownership(&snapshot.path, *uid, *gid)?;
            Ok(())
        }
        PathState::Directory { mode, uid, gid } => {
            fs::create_dir_all(&snapshot.path)?;
            fs::set_permissions(&snapshot.path, fs::Permissions::from_mode(*mode))?;
            restore_ownership(&snapshot.path, *uid, *gid)?;
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

fn restore_ownership(path: &Path, uid: Option<u32>, gid: Option<u32>) -> Result<()> {
    let (Some(uid), Some(gid)) = (uid, gid) else {
        return Ok(());
    };
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() == uid && metadata.gid() == gid {
        return Ok(());
    }
    let status = Command::new("chown")
        .arg(format!("{uid}:{gid}"))
        .arg(path)
        .status()
        .context("Could not restore installer-owned path ownership")?;
    anyhow::ensure!(
        status.success(),
        "Could not restore ownership of {}.",
        path.display()
    );
    Ok(())
}

fn write_journal(path: &Path, journal: &Journal) -> Result<()> {
    let parent = path.parent().context("Transaction journal has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".transaction.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(journal)?)?;
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

fn read_journal(path: &Path) -> Result<Journal> {
    serde_json::from_slice(
        &fs::read(path).with_context(|| format!("Could not read {}", path.display()))?,
    )
    .context("Installer transaction journal is invalid")
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

fn rooted_path(root: &Path, absolute: &Path) -> PathBuf {
    root.join(absolute.strip_prefix("/").unwrap_or(absolute))
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
        assert!(!recover_pending(root.path()).unwrap());
    }

    #[test]
    fn committing_removes_the_durable_journal() {
        let root = tempfile::tempdir().unwrap();
        let transaction = HostTransaction::begin(root.path()).unwrap();
        assert!(pending(root.path()));
        transaction.commit().unwrap();
        assert!(!pending(root.path()));
    }

    #[test]
    fn rollback_removes_a_new_installer_state_directory() {
        let root = tempfile::tempdir().unwrap();
        let transaction = HostTransaction::begin(root.path()).unwrap();

        transaction.rollback().unwrap();

        assert!(!rooted(root.path(), "/var/lib/bts-install").exists());
    }

    #[test]
    fn rollback_removes_a_new_generated_sound_namespace() {
        let root = tempfile::tempdir().unwrap();
        let generated = rooted(root.path(), "/srv/asterisk/custom/bts-generated");

        let transaction = HostTransaction::begin(root.path()).unwrap();
        track_path(root.path(), Path::new("/srv/asterisk/custom/bts-generated")).unwrap();
        fs::create_dir_all(&generated).unwrap();
        fs::write(generated.join("prompt.wav"), b"generated").unwrap();

        transaction.rollback().unwrap();

        assert!(!generated.exists());
    }

    #[test]
    fn dynamically_tracked_paths_cannot_escape_the_selected_root() {
        let root = tempfile::tempdir().unwrap();
        let transaction = HostTransaction::begin(root.path()).unwrap();

        assert!(track_path(root.path(), Path::new("/srv/../etc")).is_err());

        transaction.rollback().unwrap();
    }

    #[test]
    fn service_restore_attempts_every_snapshot_and_reports_failures() {
        let services = vec![
            ServiceSnapshot {
                unit: "bts-core.service".into(),
                enabled: true,
                active: true,
            },
            ServiceSnapshot {
                unit: "bts-display.service".into(),
                enabled: false,
                active: false,
            },
        ];
        let mut commands: Vec<Vec<String>> = Vec::new();

        let error = restore_services_with(&services, |arguments| {
            commands.push(arguments.iter().map(|value| (*value).to_owned()).collect());
            if arguments == ["start", "bts-core.service"] {
                anyhow::bail!("injected start failure");
            }
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains("bts-core.service"));
        assert_eq!(
            commands,
            vec![
                vec!["daemon-reload"],
                vec!["enable", "bts-core.service"],
                vec!["start", "bts-core.service"],
                vec!["disable", "bts-display.service"],
                vec!["stop", "bts-display.service"],
            ]
        );
    }
}
