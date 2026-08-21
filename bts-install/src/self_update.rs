use std::{
    fs::{self, File},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use semver::Version;

use crate::{
    INSTALLER_VERSION,
    manifest::ReleaseManifest,
    platform::Architecture,
    release::ReleaseClient,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfUpdateOutcome {
    Current { version: String },
    Updated { from: String, to: String },
}

impl SelfUpdateOutcome {
    pub fn changed(&self) -> bool {
        matches!(self, Self::Updated { .. })
    }
}

pub async fn self_update(client: &ReleaseClient, executable: &Path) -> Result<SelfUpdateOutcome> {
    let (manifest, urls) = client.fetch_manifest().await?;
    update_from_manifest(client, &manifest, &urls, executable).await
}

pub async fn update_from_manifest(
    client: &ReleaseClient,
    manifest: &ReleaseManifest,
    urls: &std::collections::BTreeMap<String, String>,
    executable: &Path,
) -> Result<SelfUpdateOutcome> {
    let current = parse_version(INSTALLER_VERSION)?;
    let target = parse_version(&manifest.release_version)?;

    if target == current {
        return Ok(SelfUpdateOutcome::Current {
            version: current.to_string(),
        });
    }
    ensure!(
        target > current,
        "Refusing to replace bts-install {} with older release {}.",
        current,
        target
    );

    let architecture = Architecture::detect(std::env::consts::ARCH)?;
    let installer = manifest.select_installer(architecture)?;
    let bytes = client
        .download_asset(urls, &installer.filename, &installer.sha256)
        .await?;
    ensure!(!bytes.is_empty(), "Downloaded bts-install asset is empty.");
    replace_executable_atomically(executable, &bytes)?;

    Ok(SelfUpdateOutcome::Updated {
        from: current.to_string(),
        to: target.to_string(),
    })
}

pub fn target_is_newer(manifest: &ReleaseManifest) -> Result<bool> {
    Ok(parse_version(&manifest.release_version)? > parse_version(INSTALLER_VERSION)?)
}

fn parse_version(value: &str) -> Result<Version> {
    Version::parse(value.trim_start_matches('v'))
        .with_context(|| format!("Installer version '{value}' is invalid."))
}

fn replace_executable_atomically(executable: &Path, bytes: &[u8]) -> Result<()> {
    replace_executable_atomically_with(executable, bytes, |_| Ok(()))
}

fn replace_executable_atomically_with(
    executable: &Path,
    bytes: &[u8],
    before_activate: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let parent = executable
        .parent()
        .context("The running installer path has no parent directory.")?;
    ensure!(
        parent.is_dir(),
        "Installer directory does not exist: {}",
        parent.display()
    );

    let mut temporary = tempfile::Builder::new()
        .prefix(".bts-install-update-")
        .tempfile_in(parent)
        .context("Could not create installer update beside the active executable")?;
    temporary
        .write_all(bytes)
        .context("Could not write the replacement installer")?;
    temporary
        .as_file_mut()
        .sync_all()
        .context("Could not sync the replacement installer")?;
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o755))?;
    before_activate(temporary.path())?;

    let temporary_path = temporary.into_temp_path();
    fs::rename(&temporary_path, executable).with_context(|| {
        format!(
            "Could not atomically replace the installer at {}",
            executable.display()
        )
    })?;
    temporary_path.keep().ok();
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .context("Could not sync the installer directory")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[test]
    fn atomic_replacement_activates_a_complete_executable() {
        let root = tempdir().unwrap();
        let executable = root.path().join("bts-install");
        fs::write(&executable, b"old").unwrap();

        replace_executable_atomically(&executable, b"new").unwrap();

        assert_eq!(fs::read(&executable).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn interrupted_replacement_preserves_the_active_installer() {
        let root = tempdir().unwrap();
        let executable = root.path().join("bts-install");
        fs::write(&executable, b"old").unwrap();

        let error = replace_executable_atomically_with(&executable, b"new", |_| {
            anyhow::bail!("simulated interruption before activation")
        })
        .unwrap_err();

        assert!(error.to_string().contains("simulated interruption"));
        assert_eq!(fs::read(&executable).unwrap(), b"old");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn local_release_self_update_is_checksum_verified() {
        let root = tempdir().unwrap();
        let release = root.path().join("release");
        fs::create_dir(&release).unwrap();
        let executable = root.path().join("bts-install");
        fs::write(&executable, b"old installer").unwrap();
        let replacement = b"new installer";
        let digest = hex::encode(Sha256::digest(replacement));
        let installer_filename = native_installer_filename();
        fs::write(release.join(&installer_filename), replacement).unwrap();
        fs::write(release.join("bts-install"), replacement).unwrap();
        fs::write(release.join("LICENSE"), b"licence").unwrap();
        fs::write(release.join("SHA256SUMS"), b"checksums").unwrap();
        let licence_digest = hex::encode(Sha256::digest(b"licence"));
        let manifest = serde_json::json!({
            "schema_version": crate::manifest::MANIFEST_SCHEMA_VERSION,
            "release_version": next_test_version(),
            "installer": { "filename": "bts-install", "sha256": digest },
            "installers": [{
                "platform": "linux",
                "architecture": Architecture::detect(std::env::consts::ARCH).unwrap().as_manifest_str(),
                "filename": installer_filename,
                "sha256": digest
            }],
            "components": {},
            "licence_asset": { "filename": "LICENSE", "sha256": licence_digest }
        });
        fs::write(
            release.join("release-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let client =
            ReleaseClient::new("unused/repository".into(), "stable".into(), Some(release)).unwrap();
        let outcome = self_update(&client, &executable).await.unwrap();

        assert!(outcome.changed());
        assert_eq!(fs::read(executable).unwrap(), replacement);
    }

    #[tokio::test]
    async fn current_installer_is_a_no_op() {
        let root = tempdir().unwrap();
        let executable = root.path().join("bts-install");
        fs::write(&executable, b"current installer").unwrap();
        let manifest = manifest(INSTALLER_VERSION, "0".repeat(64));

        let outcome = update_from_manifest(
            &ReleaseClient::new("unused/repository".into(), "stable".into(), None).unwrap(),
            &manifest,
            &std::collections::BTreeMap::new(),
            &executable,
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            SelfUpdateOutcome::Current {
                version: INSTALLER_VERSION.into()
            }
        );
        assert_eq!(fs::read(executable).unwrap(), b"current installer");
    }

    #[tokio::test]
    async fn automatic_preflight_update_verifies_before_replacing() {
        let root = tempdir().unwrap();
        let release = root.path().join("release");
        let executable = root.path().join("bts-install");
        fs::create_dir(&release).unwrap();
        fs::write(&executable, b"old installer").unwrap();
        write_local_release(&release, b"new installer", None);
        let client =
            ReleaseClient::new("unused/repository".into(), "stable".into(), Some(release)).unwrap();
        let (manifest, urls) = client.fetch_manifest().await.unwrap();

        let outcome = update_from_manifest(&client, &manifest, &urls, &executable)
            .await
            .unwrap();

        assert!(outcome.changed());
        assert_eq!(fs::read(executable).unwrap(), b"new installer");
    }

    #[tokio::test]
    async fn failed_checksum_verification_preserves_the_active_installer() {
        let root = tempdir().unwrap();
        let release = root.path().join("release");
        let executable = root.path().join("bts-install");
        fs::create_dir(&release).unwrap();
        fs::write(&executable, b"old installer").unwrap();
        write_local_release(&release, b"corrupt installer", Some("0".repeat(64)));
        let client =
            ReleaseClient::new("unused/repository".into(), "stable".into(), Some(release)).unwrap();

        let error = self_update(&client, &executable).await.unwrap_err();

        assert!(format!("{error:#}").contains("checksum"));
        assert_eq!(fs::read(executable).unwrap(), b"old installer");
    }

    fn write_local_release(release: &Path, replacement: &[u8], digest: Option<String>) {
        let installer_digest = digest.unwrap_or_else(|| hex::encode(Sha256::digest(replacement)));
        let installer_filename = native_installer_filename();
        fs::write(release.join(&installer_filename), replacement).unwrap();
        fs::write(release.join("bts-install"), replacement).unwrap();
        fs::write(release.join("LICENSE"), b"licence").unwrap();
        fs::write(release.join("SHA256SUMS"), b"checksums").unwrap();
        fs::write(
            release.join("release-manifest.json"),
            serde_json::to_vec(&manifest(&next_test_version(), installer_digest)).unwrap(),
        )
        .unwrap();
    }

    fn manifest(version: &str, installer_digest: String) -> ReleaseManifest {
        let architecture = Architecture::detect(std::env::consts::ARCH).unwrap();
        ReleaseManifest {
            schema_version: crate::manifest::MANIFEST_SCHEMA_VERSION,
            release_version: version.into(),
            installer: crate::manifest::ReleaseAsset {
                filename: "bts-install".into(),
                sha256: installer_digest.clone(),
            },
            installers: vec![crate::manifest::InstallerAsset {
                platform: "linux".into(),
                architecture: architecture.as_manifest_str().into(),
                filename: native_installer_filename(),
                sha256: installer_digest,
            }],
            components: std::collections::BTreeMap::new(),
            licence_asset: Some(crate::manifest::ReleaseAsset {
                filename: "LICENSE".into(),
                sha256: hex::encode(Sha256::digest(b"licence")),
            }),
        }
    }

    fn native_installer_filename() -> String {
        format!(
            "bts-install-linux-{}",
            Architecture::detect(std::env::consts::ARCH)
                .unwrap()
                .as_manifest_str()
        )
    }

    fn next_test_version() -> String {
        let mut version = parse_version(INSTALLER_VERSION).unwrap();
        version.patch += 1;
        version.pre = semver::Prerelease::EMPTY;
        version.to_string()
    }
}
