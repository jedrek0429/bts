use std::{
    collections::BTreeMap,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::manifest::{ReleaseManifest, validate_release_assets};

#[derive(Debug, Clone)]
pub struct ReleaseClient {
    client: reqwest::Client,
    api_base_url: String,
    repository: String,
    selection: ReleaseSelection,
    local_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseSelection {
    Track(ReleaseTrack),
    Version(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseTrack {
    Stable(Option<ReleaseSeries>),
    Candidate(Option<ReleaseSeries>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReleaseSeries {
    major: u64,
    minor: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

impl ReleaseClient {
    pub fn new(
        repository: String,
        channel: String,
        local_directory: Option<PathBuf>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(format!("bts-install/{}", crate::INSTALLER_VERSION))
            .build()?;
        Ok(Self {
            client,
            api_base_url: "https://api.github.com".into(),
            repository,
            selection: ReleaseSelection::parse(&channel)?,
            local_directory,
        })
    }

    pub fn recorded_channel(&self, manifest: &ReleaseManifest) -> Result<String> {
        let version = Version::parse(manifest.release_version.trim_start_matches('v'))
            .context("Release manifest version is invalid")?;
        Ok(match &self.selection {
            ReleaseSelection::Version(tag) => tag.clone(),
            ReleaseSelection::Track(ReleaseTrack::Stable(None)) => "stable".into(),
            ReleaseSelection::Track(ReleaseTrack::Stable(Some(series))) => {
                format!("stable/{series}")
            }
            ReleaseSelection::Track(ReleaseTrack::Candidate(_)) if version.pre.is_empty() => {
                format!("stable/{}.{}", version.major, version.minor)
            }
            ReleaseSelection::Track(ReleaseTrack::Candidate(_)) => {
                format!("rc/{}.{}", version.major, version.minor)
            }
        })
    }

    pub fn is_exact_version(&self) -> bool {
        matches!(self.selection, ReleaseSelection::Version(_))
    }

    #[cfg(test)]
    pub(crate) fn with_api_base_url(mut self, api_base_url: String) -> Self {
        self.api_base_url = api_base_url;
        self
    }

    pub async fn fetch_manifest(&self) -> Result<(ReleaseManifest, BTreeMap<String, String>)> {
        if let Some(directory) = &self.local_directory {
            return load_local_manifest(directory);
        }
        let release: GithubRelease = if matches!(self.selection, ReleaseSelection::Track(_)) {
            let endpoint = format!(
                "{}/repos/{}/releases?per_page=100",
                self.api_base_url.trim_end_matches('/'),
                self.repository,
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
            select_release(&releases, &self.selection)
                .cloned()
                .context("Repository has no published compatible BTS release")?
        } else {
            let ReleaseSelection::Version(tag) = &self.selection else {
                unreachable!()
            };
            let endpoint = format!(
                "{}/repos/{}/releases/tags/{}",
                self.api_base_url.trim_end_matches('/'),
                self.repository,
                tag,
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
        let bytes = if self.local_directory.is_some() {
            fs::read(url)
                .with_context(|| format!("Could not read local release asset '{filename}'."))?
        } else {
            self.download_url(url).await?
        };
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

impl ReleaseSelection {
    pub fn parse(value: &str) -> Result<Self> {
        if value == "stable" {
            return Ok(Self::Track(ReleaseTrack::Stable(None)));
        }
        if value == "rc" {
            return Ok(Self::Track(ReleaseTrack::Candidate(None)));
        }
        if let Some(value) = value.strip_prefix("stable/") {
            return Ok(Self::Track(ReleaseTrack::Stable(Some(
                ReleaseSeries::parse(value)?,
            ))));
        }
        if let Some(value) = value.strip_prefix("rc/") {
            return Ok(Self::Track(ReleaseTrack::Candidate(Some(
                ReleaseSeries::parse(value)?,
            ))));
        }
        if value.starts_with('v') && crate::manifest::is_release_version(value) {
            return Ok(Self::Version(value.into()));
        }
        bail!(
            "Release selection must be stable, stable/MAJOR.MINOR, rc, rc/MAJOR.MINOR or an explicit vVERSION."
        )
    }
}

pub fn normalise_legacy_selection(value: &str) -> Result<String> {
    if value == crate::LOCAL_RELEASE_CHANNEL {
        return Ok(value.into());
    }
    match ReleaseSelection::parse(value)? {
        ReleaseSelection::Version(tag) => {
            let version = Version::parse(tag.trim_start_matches('v'))?;
            if is_rc(&version) {
                Ok(format!("rc/{}.{}", version.major, version.minor))
            } else {
                Ok(tag)
            }
        }
        _ => Ok(value.into()),
    }
}

impl ReleaseSeries {
    fn parse(value: &str) -> Result<Self> {
        let (major, minor) = value
            .split_once('.')
            .context("A release track series must use MAJOR.MINOR form")?;
        ensure!(
            !minor.contains('.'),
            "A release track series must use MAJOR.MINOR form."
        );
        Ok(Self {
            major: major
                .parse()
                .context("Release track major version is invalid")?,
            minor: minor
                .parse()
                .context("Release track minor version is invalid")?,
        })
    }

    fn contains(self, version: &Version) -> bool {
        version.major == self.major && version.minor == self.minor
    }
}

impl std::fmt::Display for ReleaseSeries {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

fn load_local_manifest(directory: &Path) -> Result<(ReleaseManifest, BTreeMap<String, String>)> {
    ensure!(
        directory.is_dir(),
        "Local release directory does not exist: {}",
        directory.display()
    );
    let manifest_path = directory.join("release-manifest.json");
    let manifest = ReleaseManifest::parse(
        &fs::read(&manifest_path)
            .with_context(|| format!("Could not read {}", manifest_path.display()))?,
    )?;
    let mut locations = BTreeMap::new();
    let mut checksums = BTreeMap::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("Local release contains a non-UTF-8 filename."))?;
        let path = entry.path();
        let bytes = fs::read(&path)?;
        checksums.insert(name.clone(), hex::encode(Sha256::digest(&bytes)));
        locations.insert(name, path.to_string_lossy().into_owned());
    }
    validate_release_assets(&manifest, &checksums)?;
    Ok((manifest, locations))
}

fn is_stable_release(release: &GithubRelease) -> bool {
    release_version(&release.tag_name).is_some()
        && !release.draft
        && !release.prerelease
        && has_release_manifest(release)
}

fn is_candidate_release(release: &GithubRelease) -> bool {
    release_semver(release).is_some_and(|version| {
        !release.draft && release.prerelease && is_rc(&version) && has_release_manifest(release)
    })
}

fn select_release<'a>(
    releases: &'a [GithubRelease],
    selection: &ReleaseSelection,
) -> Option<&'a GithubRelease> {
    match selection {
        ReleaseSelection::Version(_) => None,
        ReleaseSelection::Track(ReleaseTrack::Stable(series)) => releases
            .iter()
            .filter(|release| is_stable_release(release))
            .filter(|release| {
                series.is_none_or(|series| {
                    release_semver(release).is_some_and(|version| series.contains(&version))
                })
            })
            .max_by_key(|release| release_semver(release)),
        ReleaseSelection::Track(ReleaseTrack::Candidate(series)) => {
            let candidate = releases
                .iter()
                .filter(|release| is_candidate_release(release))
                .filter(|release| {
                    series.is_none_or(|series| {
                        release_semver(release).is_some_and(|version| series.contains(&version))
                    })
                })
                .max_by_key(|release| release_semver(release))?;
            let mut final_version = release_semver(candidate)?;
            final_version.pre = semver::Prerelease::EMPTY;
            releases
                .iter()
                .find(|release| {
                    is_stable_release(release)
                        && release_semver(release).as_ref() == Some(&final_version)
                })
                .or(Some(candidate))
        }
    }
}

fn has_release_manifest(release: &GithubRelease) -> bool {
    release
        .assets
        .iter()
        .any(|asset| asset.name == "release-manifest.json")
}

fn is_rc(version: &Version) -> bool {
    version
        .pre
        .as_str()
        .strip_prefix("rc.")
        .is_some_and(|number| {
            number.as_bytes().first().is_some_and(|byte| *byte != b'0')
                && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn release_semver(release: &GithubRelease) -> Option<Version> {
    let version = release.tag_name.strip_prefix('v')?;
    let version = Version::parse(version).ok()?;
    version.build.is_empty().then_some(version)
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
    use crate::{
        manifest::{ComponentAsset, ReleaseAsset},
        model::Component,
    };
    use tempfile::tempdir;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    async fn serve_http(responses: Vec<(String, u16, Vec<u8>)>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = spawn_http(listener, responses);
        (format!("http://{address}"), handle)
    }

    fn spawn_http(listener: TcpListener, responses: Vec<(String, u16, Vec<u8>)>) -> JoinHandle<()> {
        tokio::spawn(async move {
            for (expected_path, status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut buffer = [0; 1024];
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                assert!(
                    request.starts_with(&format!("GET {expected_path} HTTP/1.1")),
                    "unexpected request: {request}"
                );
                let reason = if status == 200 {
                    "OK"
                } else {
                    "Service Unavailable"
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
        })
    }

    fn published_release(tag_name: &str, prerelease: bool) -> GithubRelease {
        GithubRelease {
            tag_name: tag_name.into(),
            draft: false,
            prerelease,
            assets: vec![GithubAsset {
                name: "release-manifest.json".into(),
                browser_download_url: "https://example.invalid/manifest".into(),
            }],
        }
    }

    fn manifest(version: &str) -> ReleaseManifest {
        ReleaseManifest {
            schema_version: crate::manifest::MANIFEST_SCHEMA_VERSION,
            release_version: version.into(),
            installer: ReleaseAsset {
                filename: "bts-install".into(),
                sha256: "0".repeat(64),
            },
            components: BTreeMap::new(),
            licence_asset: None,
        }
    }

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

    #[tokio::test]
    async fn github_lookup_failure_is_reported_without_resolving_a_manifest() {
        let (base_url, server) = serve_http(vec![(
            "/repos/example/bts/releases?per_page=100".into(),
            503,
            b"unavailable".to_vec(),
        )])
        .await;
        let client = ReleaseClient::new("example/bts".into(), "stable".into(), None)
            .unwrap()
            .with_api_base_url(base_url);

        let error = client.fetch_manifest().await.unwrap_err();

        assert!(format!("{error:#}").contains("503"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn explicit_prerelease_tag_is_selected_and_installed() {
        let replacement = b"new prerelease installer";
        let mut selected_manifest = manifest("0.4.0-rc.2");
        selected_manifest.installer.sha256 = hex::encode(Sha256::digest(replacement));
        let release_manifest = serde_json::to_vec(&selected_manifest).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let base_url = format!("http://{address}");
        let release = serde_json::to_vec(&serde_json::json!({
            "tag_name": "v0.4.0-rc.2",
            "draft": false,
            "prerelease": true,
            "assets": [{
                "name": "release-manifest.json",
                "browser_download_url": format!("{base_url}/release-manifest.json")
            }, {
                "name": "bts-install",
                "browser_download_url": format!("{base_url}/bts-install")
            }]
        }))
        .unwrap();
        let server = spawn_http(
            listener,
            vec![
                (
                    "/repos/example/bts/releases/tags/v0.4.0-rc.2".into(),
                    200,
                    release,
                ),
                ("/release-manifest.json".into(), 200, release_manifest),
                ("/bts-install".into(), 200, replacement.to_vec()),
            ],
        );
        let client = ReleaseClient::new("example/bts".into(), "v0.4.0-rc.2".into(), None)
            .unwrap()
            .with_api_base_url(base_url);
        let directory = tempdir().unwrap();
        let executable = directory.path().join("bts-install");
        fs::write(&executable, b"old installer").unwrap();

        let outcome = crate::self_update::self_update(&client, &executable)
            .await
            .unwrap();

        assert!(outcome.changed());
        assert_eq!(fs::read(executable).unwrap(), replacement);
        server.await.unwrap();
    }

    #[test]
    fn candidate_tracks_are_bounded_after_bare_rc_enrolment() {
        let releases = [
            published_release("v0.3.0-rc.2", true),
            published_release("v0.4.0-rc.1", true),
            published_release("v0.4.0-rc.3", true),
            published_release("v0.5.0-rc.1", true),
        ];
        let latest = ReleaseSelection::parse("rc").unwrap();
        let bounded = ReleaseSelection::parse("rc/0.4").unwrap();

        assert_eq!(
            select_release(&releases, &latest).unwrap().tag_name,
            "v0.5.0-rc.1"
        );
        assert_eq!(
            select_release(&releases, &bounded).unwrap().tag_name,
            "v0.4.0-rc.3"
        );
    }

    #[test]
    fn candidate_track_promotes_to_its_matching_stable_release() {
        let releases = [
            published_release("v0.4.0-rc.3", true),
            published_release("v0.4.0", false),
            published_release("v0.5.0-rc.1", true),
        ];
        let selection = ReleaseSelection::parse("rc/0.4").unwrap();

        assert_eq!(
            select_release(&releases, &selection).unwrap().tag_name,
            "v0.4.0"
        );
    }

    #[test]
    fn release_selection_distinguishes_tracks_and_pins() {
        assert_eq!(
            ReleaseSelection::parse("stable/0.4").unwrap(),
            ReleaseSelection::Track(ReleaseTrack::Stable(Some(ReleaseSeries {
                major: 0,
                minor: 4
            })))
        );
        assert_eq!(
            ReleaseSelection::parse("v0.4.0-rc.2").unwrap(),
            ReleaseSelection::Version("v0.4.0-rc.2".into())
        );
        assert!(ReleaseSelection::parse("rc/0.4.0").is_err());
    }

    #[test]
    fn legacy_state_selections_normalise_to_current_tracks_and_pins() {
        assert_eq!(normalise_legacy_selection("v0.4.0-rc.2").unwrap(), "rc/0.4");
        assert_eq!(normalise_legacy_selection("v0.4.0").unwrap(), "v0.4.0");
    }

    #[test]
    fn resolved_candidate_tracks_are_persisted_canonically() {
        let latest = ReleaseClient::new("example/bts".into(), "rc".into(), None).unwrap();
        let bounded = ReleaseClient::new("example/bts".into(), "rc/0.4".into(), None).unwrap();
        let pinned = ReleaseClient::new("example/bts".into(), "v0.4.0-rc.2".into(), None).unwrap();

        assert_eq!(
            latest.recorded_channel(&manifest("0.4.0-rc.2")).unwrap(),
            "rc/0.4"
        );
        assert_eq!(
            bounded.recorded_channel(&manifest("0.4.0")).unwrap(),
            "stable/0.4"
        );
        assert_eq!(
            pinned.recorded_channel(&manifest("0.4.0-rc.2")).unwrap(),
            "v0.4.0-rc.2"
        );
    }

    #[tokio::test]
    async fn loads_and_verifies_a_local_release() {
        let directory = tempdir().unwrap();
        let component = b"bundle";
        let installer = b"installer";
        let licence = b"licence";
        let digest = |bytes: &[u8]| hex::encode(Sha256::digest(bytes));
        fs::write(directory.path().join("component.tar.zst"), component).unwrap();
        fs::write(directory.path().join("bts-install"), installer).unwrap();
        fs::write(directory.path().join("LICENSE"), licence).unwrap();
        fs::write(directory.path().join("SHA256SUMS"), "checksums").unwrap();
        let manifest = ReleaseManifest {
            schema_version: crate::manifest::MANIFEST_SCHEMA_VERSION,
            release_version: "0.4.0-dev.1".into(),
            installer: ReleaseAsset {
                filename: "bts-install".into(),
                sha256: digest(installer),
            },
            components: BTreeMap::from([(
                Component::Core,
                vec![ComponentAsset {
                    platform: "linux".into(),
                    architecture: "x86_64".into(),
                    filename: "component.tar.zst".into(),
                    sha256: digest(component),
                    bundle_format_version: crate::manifest::BUNDLE_FORMAT_VERSION,
                }],
            )]),
            licence_asset: Some(ReleaseAsset {
                filename: "LICENSE".into(),
                sha256: digest(licence),
            }),
        };
        fs::write(
            directory.path().join("release-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();

        let client = ReleaseClient::new(
            "unused/repository".into(),
            "stable".into(),
            Some(directory.path().into()),
        )
        .unwrap();
        let (loaded, locations) = client.fetch_manifest().await.unwrap();
        assert_eq!(loaded.release_version, "0.4.0-dev.1");
        assert_eq!(
            client
                .download_asset(&locations, "component.tar.zst", &digest(component))
                .await
                .unwrap(),
            component
        );

        fs::write(directory.path().join("component.tar.zst"), "changed").unwrap();
        assert!(client.fetch_manifest().await.is_err());
    }
}
