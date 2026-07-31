use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    Debian,
    Arch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Architecture {
    X86_64,
    Aarch64,
}

impl Architecture {
    pub fn detect(machine: &str) -> Result<Self> {
        match machine.trim() {
            "x86_64" | "amd64" => Ok(Self::X86_64),
            "aarch64" | "arm64" => Ok(Self::Aarch64),
            other => bail!(
                "Unsupported architecture '{other}'. BTS v0.3 supports x86_64 and aarch64 release assets."
            ),
        }
    }

    pub fn as_manifest_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

impl Platform {
    pub fn detect(os_release: &str) -> Result<Self> {
        let values = parse_os_release(os_release);
        let id = values
            .iter()
            .find(|(key, _)| key == "ID")
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        let like = values
            .iter()
            .find(|(key, _)| key == "ID_LIKE")
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        if matches!(id, "debian" | "ubuntu" | "raspbian")
            || like
                .split_whitespace()
                .any(|value| matches!(value, "debian" | "ubuntu"))
        {
            Ok(Self::Debian)
        } else if matches!(id, "arch" | "archarm")
            || like.split_whitespace().any(|value| value == "arch")
        {
            Ok(Self::Arch)
        } else {
            bail!(
                "Unsupported operating system '{id}'. BTS v0.3 supports Debian-family Linux and Arch Linux through platform adapters."
            )
        }
    }

    pub fn as_manifest_str(self) -> &'static str {
        "linux"
    }

    pub fn packages_for(self, dependency: &str) -> Result<&'static [&'static str]> {
        match (self, dependency) {
            (Self::Debian, "cage") => Ok(&["cage"]),
            (Self::Debian, "seatd") => Ok(&["seatd"]),
            (Self::Debian, "font-cabin") => Ok(&["fonts-cabin"]),
            (Self::Debian, "ca-certificates") => Ok(&["ca-certificates"]),
            (Self::Arch, "cage") => Ok(&["cage"]),
            (Self::Arch, "seatd") => Ok(&["seatd"]),
            (Self::Arch, "font-cabin") => Ok(&["ttf-impallari-cabin-font"]),
            (Self::Arch, "ca-certificates") => Ok(&["ca-certificates"]),
            (_, other) => bail!("No package mapping exists for runtime dependency '{other}'."),
        }
    }

    pub fn package_command(self, packages: &[String], yes: bool) -> Vec<String> {
        match self {
            Self::Debian => [
                vec!["apt-get".into(), "install".into()],
                if yes { vec!["-y".into()] } else { Vec::new() },
                packages.to_vec(),
            ]
            .concat(),
            Self::Arch => [
                vec!["pacman".into(), "-S".into(), "--needed".into()],
                if yes {
                    vec!["--noconfirm".into()]
                } else {
                    Vec::new()
                },
                packages.to_vec(),
            ]
            .concat(),
        }
    }
}

pub fn detect_host(root: &Path) -> Result<(Platform, Architecture)> {
    let release = fs::read_to_string(root.join("etc/os-release"))
        .context("Could not read /etc/os-release")?;
    let platform = Platform::detect(&release)?;
    let machine = if root == Path::new("/") {
        String::from_utf8(Command::new("uname").arg("-m").output()?.stdout)
            .context("uname output was not UTF-8")?
    } else {
        std::env::var("BTS_INSTALL_TEST_ARCH").unwrap_or_else(|_| std::env::consts::ARCH.into())
    };
    Ok((platform, Architecture::detect(&machine)?))
}

fn parse_os_release(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.into(), value.trim_matches(['"', '\'']).into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_debian_family_and_arch() {
        assert_eq!(
            Platform::detect("ID=raspbian\nID_LIKE=debian").unwrap(),
            Platform::Debian
        );
        assert_eq!(
            Platform::detect("ID=archarm\nID_LIKE=arch").unwrap(),
            Platform::Arch
        );
        assert!(Platform::detect("ID=fedora").is_err());
    }

    #[test]
    fn normalises_architectures() {
        assert_eq!(
            Architecture::detect("arm64").unwrap(),
            Architecture::Aarch64
        );
        assert_eq!(Architecture::detect("amd64").unwrap(), Architecture::X86_64);
        assert!(Architecture::detect("armv7l").is_err());
    }

    #[test]
    fn commands_are_adapter_specific() {
        let packages = vec!["cage".into()];
        assert_eq!(
            Platform::Debian.package_command(&packages, true),
            ["apt-get", "install", "-y", "cage"]
        );
        assert_eq!(
            Platform::Arch.package_command(&packages, true),
            ["pacman", "-S", "--needed", "--noconfirm", "cage"]
        );
    }
}
