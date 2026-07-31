use std::{collections::BTreeMap, io::Cursor};

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::manifest::{ReleaseManifest, validate_release_assets};

#[derive(Debug, Clone)]
pub struct ReleaseClient {
    client: reqwest::Client,
    repository: String,
    channel: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

impl ReleaseClient {
    pub fn new(repository: String, channel: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("bts-install/{}", crate::INSTALLER_VERSION))
            .build()?;
        Ok(Self {
            client,
            repository,
            channel,
        })
    }

    pub async fn fetch_manifest(&self) -> Result<(ReleaseManifest, BTreeMap<String, String>)> {
        let release: GithubRelease = if self.channel == "stable" {
            let endpoint = format!(
                "https://api.github.com/repos/{}/releases?per_page=100",
                self.repository
            );
            let releases: Vec<GithubRelease> = self
                .client
                .get(endpoint)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
                .context("GitHub release metadata is invalid")?;
            releases
                .into_iter()
                .filter(is_stable_release)
                .max_by_key(|release| release_version(&release.tag_name))
                .context("Repository has no published compatible BTS release")?
        } else {
            let endpoint = format!(
                "https://api.github.com/repos/{}/releases/tags/{}",
                self.repository, self.channel
            );
            self.client
                .get(endpoint)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
                .context("GitHub release metadata is invalid")?
        };
        ensure!(
            release.tag_name.starts_with('v')
                && crate::manifest::is_release_version(&release.tag_name),
            "Selected release tag '{}' is invalid.",
            release.tag_name
        );
        let urls: BTreeMap<_, _> = release
            .assets
            .into_iter()
            .map(|asset| (asset.name, asset.browser_download_url))
            .collect();
        let url = urls
            .get("release-manifest.json")
            .context("Release does not contain release-manifest.json")?;
        let bytes = self.download_url(url).await?;
        let manifest = ReleaseManifest::parse(&bytes)?;
        ensure!(
            release.tag_name.trim_start_matches('v')
                == manifest.release_version.trim_start_matches('v'),
            "Release tag and manifest version differ."
        );
        Ok((manifest, urls))
    }

    pub async fn download_asset(
        &self,
        urls: &BTreeMap<String, String>,
        filename: &str,
        expected: &str,
    ) -> Result<Vec<u8>> {
        let url = urls
            .get(filename)
            .with_context(|| format!("Release asset '{filename}' is missing."))?;
        let bytes = self.download_url(url).await?;
        crate::archive::verify_sha256(Cursor::new(&bytes), expected)?;
        Ok(bytes)
    }

    async fn download_url(&self, url: &str) -> Result<Vec<u8>> {
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?
            .to_vec())
    }
}

fn is_stable_release(release: &GithubRelease) -> bool {
    release_version(&release.tag_name).is_some()
        && !release.draft
        && !release.prerelease
        && release
            .assets
            .iter()
            .any(|asset| asset.name == "release-manifest.json")
}

fn release_version(tag: &str) -> Option<Version> {
    let version = tag.strip_prefix('v')?;
    let version = Version::parse(version).ok()?;
    (version.pre.is_empty() && version.build.is_empty()).then_some(version)
}

pub fn validate_local_assets(
    manifest: &ReleaseManifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    let checksums = files
        .iter()
        .map(|(name, bytes)| (name.clone(), hex::encode(Sha256::digest(bytes))))
        .collect();
    validate_release_assets(manifest, &checksums)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_release_excludes_drafts_and_prereleases() {
        let release = |tag_name: &str, draft, prerelease| GithubRelease {
            tag_name: tag_name.into(),
            draft,
            prerelease,
            assets: Vec::new(),
        };
        let releases = [
            release("v0.9.0", true, false),
            release("v0.10.0-rc.1", false, true),
            release("v0.10.0", false, false),
        ];

        let mut releases = releases;
        for release in &mut releases {
            release.assets.push(GithubAsset {
                name: "release-manifest.json".into(),
                browser_download_url: "https://example.invalid/manifest".into(),
            });
        }
        let selected = releases
            .into_iter()
            .filter(is_stable_release)
            .max_by_key(|release| release_version(&release.tag_name));
        assert_eq!(selected.unwrap().tag_name, "v0.10.0");
    }

    #[test]
    fn stable_release_ignores_legacy_releases_without_manifests() {
        let release = GithubRelease {
            tag_name: "v0.2.1".into(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        };
        assert!(!is_stable_release(&release));
    }
}
