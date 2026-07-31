use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{
    model::Component,
    platform::{Architecture, Platform},
};

pub use bts_compat::{
    COMPONENT_BUNDLE_FORMAT_VERSION as BUNDLE_FORMAT_VERSION,
    RELEASE_MANIFEST_SCHEMA_VERSION as MANIFEST_SCHEMA_VERSION,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub release_version: String,
    pub installer: ReleaseAsset,
    pub components: BTreeMap<Component, Vec<ComponentAsset>>,
    #[serde(default)]
    pub licence_asset: Option<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub filename: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentAsset {
    pub platform: String,
    pub architecture: String,
    pub filename: String,
    pub sha256: String,
    pub bundle_format_version: u32,
}

impl ReleaseManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let manifest: Self =
            serde_json::from_slice(bytes).context("Release manifest is not valid JSON")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == MANIFEST_SCHEMA_VERSION,
            "Release manifest schema {} is not supported; expected {}.",
            self.schema_version,
            MANIFEST_SCHEMA_VERSION
        );
        ensure!(
            is_release_version(&self.release_version),
            "Release version '{}' is invalid.",
            self.release_version
        );
        validate_asset(&self.installer)?;
        if let Some(asset) = &self.licence_asset {
            validate_asset(asset)?;
        }
        let mut filenames = BTreeSet::new();
        filenames.insert(self.installer.filename.as_str());
        for assets in self.components.values() {
            for asset in assets {
                ensure!(
                    asset.bundle_format_version == BUNDLE_FORMAT_VERSION,
                    "Bundle format {} is not supported.",
                    asset.bundle_format_version
                );
                validate_asset(&ReleaseAsset {
                    filename: asset.filename.clone(),
                    sha256: asset.sha256.clone(),
                })?;
                ensure!(
                    filenames.insert(asset.filename.as_str()),
                    "Release manifest contains duplicate asset '{}'.",
                    asset.filename
                );
            }
        }
        Ok(())
    }

    pub fn select(
        &self,
        component: Component,
        platform: Platform,
        architecture: Architecture,
    ) -> Result<&ComponentAsset> {
        self.components
            .get(&component)
            .and_then(|assets| {
                assets.iter().find(|asset| {
                    asset.platform == platform.as_manifest_str()
                        && asset.architecture == architecture.as_manifest_str()
                })
            })
            .with_context(|| {
                format!(
                    "Release {} does not provide {} for linux/{}.",
                    self.release_version,
                    component,
                    architecture.as_manifest_str()
                )
            })
    }

    pub fn required_filenames(&self) -> BTreeSet<&str> {
        let mut files = BTreeSet::from([
            self.installer.filename.as_str(),
            "release-manifest.json",
            "SHA256SUMS",
        ]);
        if let Some(asset) = &self.licence_asset {
            files.insert(asset.filename.as_str());
        }
        for asset in self.components.values().flatten() {
            files.insert(asset.filename.as_str());
        }
        files
    }
}

fn validate_asset(asset: &ReleaseAsset) -> Result<()> {
    ensure!(
        !asset.filename.is_empty()
            && !asset.filename.contains('/')
            && asset.filename != "."
            && asset.filename != "..",
        "Unsafe release asset filename '{}'.",
        asset.filename
    );
    ensure!(
        asset.sha256.len() == 64 && asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Asset '{}' has an invalid SHA-256 checksum.",
        asset.filename
    );
    Ok(())
}

pub fn is_release_version(version: &str) -> bool {
    let version = version.strip_prefix('v').unwrap_or(version);
    Version::parse(version).is_ok_and(|version| version.build.is_empty())
}

pub fn validate_release_assets(
    manifest: &ReleaseManifest,
    assets: &BTreeMap<String, String>,
) -> Result<()> {
    for filename in manifest.required_filenames() {
        let actual = assets
            .get(filename)
            .with_context(|| format!("Release asset '{filename}' is missing."))?;
        let expected = if filename == manifest.installer.filename {
            Some(&manifest.installer.sha256)
        } else if manifest
            .licence_asset
            .as_ref()
            .is_some_and(|value| value.filename == filename)
        {
            manifest.licence_asset.as_ref().map(|value| &value.sha256)
        } else {
            manifest
                .components
                .values()
                .flatten()
                .find(|value| value.filename == filename)
                .map(|value| &value.sha256)
        };
        if let Some(expected) = expected {
            ensure!(
                actual.eq_ignore_ascii_case(expected),
                "Release asset '{filename}' checksum does not match its manifest entry."
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ReleaseManifest {
        ReleaseManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            release_version: "0.3.2".into(),
            installer: ReleaseAsset {
                filename: "bts-install".into(),
                sha256: "a".repeat(64),
            },
            components: BTreeMap::from([(
                Component::Display,
                vec![ComponentAsset {
                    platform: "linux".into(),
                    architecture: "aarch64".into(),
                    filename: "display.tar.zst".into(),
                    sha256: "b".repeat(64),
                    bundle_format_version: BUNDLE_FORMAT_VERSION,
                }],
            )]),
            licence_asset: Some(ReleaseAsset {
                filename: "LICENSE".into(),
                sha256: "c".repeat(64),
            }),
        }
    }

    #[test]
    fn parses_and_selects_assets_only_from_manifest() {
        let bytes = serde_json::to_vec(&manifest()).unwrap();
        let parsed = ReleaseManifest::parse(&bytes).unwrap();
        assert_eq!(
            parsed
                .select(Component::Display, Platform::Debian, Architecture::Aarch64)
                .unwrap()
                .filename,
            "display.tar.zst"
        );
        assert!(
            parsed
                .select(Component::Core, Platform::Debian, Architecture::Aarch64)
                .is_err()
        );
        assert!(is_release_version("v0.4.0-rc.1"));
        assert!(!is_release_version("v0.4.0+local"));
    }

    #[test]
    fn rejects_incompatible_schema_or_unsafe_manifests() {
        let mut value = manifest();
        value.schema_version = 2;
        assert!(value.validate().is_err());
        let mut value = manifest();
        value.release_version = "0.4.0".into();
        assert!(value.validate().is_ok());
        value.release_version = "0.4".into();
        assert!(value.validate().is_err());
        let mut value = manifest();
        value.installer.filename = "../installer".into();
        assert!(value.validate().is_err());
        let mut value = manifest();
        value.installer.sha256 = "no".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn detects_missing_and_mismatched_release_assets() {
        let value = manifest();
        let mut assets = BTreeMap::new();
        assert!(
            validate_release_assets(&value, &assets)
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        for name in value.required_filenames() {
            assets.insert(name.into(), "0".repeat(64));
        }
        assert!(
            validate_release_assets(&value, &assets)
                .unwrap_err()
                .to_string()
                .contains("does not match")
        );
    }
}
