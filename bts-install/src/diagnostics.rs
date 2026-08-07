use std::{fs, os::unix::fs::PermissionsExt, path::Path};

use serde::{Deserialize, Serialize};

use crate::{model::Component, state::InstallerState, system::SystemAdapter};

pub use bts_compat::INSTALLER_OUTPUT_SCHEMA_VERSION as OUTPUT_SCHEMA_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    pub schema_version: u32,
    pub installer_version: String,
    pub installed_version: Option<String>,
    pub available_version: Option<String>,
    pub selected_role: Option<String>,
    pub components: Vec<ComponentStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub component: Component,
    pub installed: bool,
    pub version: Option<String>,
    pub enabled: Option<bool>,
    pub active: Option<bool>,
    pub configured_endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub schema_version: u32,
    pub healthy: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub component: Option<Component>,
    pub severity: Severity,
    pub message: String,
    pub suggested_action: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Warning,
    Error,
}

pub fn status<S: SystemAdapter>(
    root: &Path,
    state: Option<&InstallerState>,
    system: &mut S,
) -> StatusReport {
    let components = Component::ALL
        .into_iter()
        .map(|component| {
            let installed =
                state.is_some_and(|value| value.installed_components.contains(&component));
            let (enabled, active) =
                if root == Path::new("/") && installed && component.unit().is_some() {
                    let unit = component.unit().expect("checked service component");
                    (
                        Some(
                            system
                                .output("systemctl", &["is-enabled".into(), unit.into()])
                                .is_ok(),
                        ),
                        Some(
                            system
                                .output("systemctl", &["is-active".into(), unit.into()])
                                .is_ok(),
                        ),
                    )
                } else {
                    (None, None)
                };
            let configured_endpoint = read_endpoint(root, component);
            ComponentStatus {
                component,
                installed,
                version: state.and_then(|value| value.component_versions.get(&component).cloned()),
                enabled,
                active,
                configured_endpoint,
            }
        })
        .collect();
    StatusReport {
        schema_version: OUTPUT_SCHEMA_VERSION,
        installer_version: crate::INSTALLER_VERSION.into(),
        installed_version: state.map(|value| value.installed_version.clone()),
        available_version: None,
        selected_role: state.and_then(|value| value.selected_role.map(|role| role.to_string())),
        components,
    }
}

pub fn doctor<S: SystemAdapter>(
    root: &Path,
    state: Option<&InstallerState>,
    system: &mut S,
) -> DoctorReport {
    let mut diagnostics = Vec::new();
    let Some(state) = state else {
        diagnostics.push(Diagnostic {
            component: None,
            severity: Severity::Error,
            message: "Installer state is missing.".into(),
            suggested_action: Some("Run: sudo bts-install install ROLE".into()),
        });
        return DoctorReport {
            schema_version: OUTPUT_SCHEMA_VERSION,
            healthy: false,
            diagnostics,
        };
    };
    match crate::config::plan_legacy_environment_migration(root, &state.installed_components) {
        Ok(Some(_)) => diagnostics.push(Diagnostic {
            component: None,
            severity: Severity::Error,
            message: "Legacy shared configuration /etc/bts/bts.env requires migration.".into(),
            suggested_action: Some(
                "Run a bts-install upgrade after reviewing the component configuration migration."
                    .into(),
            ),
        }),
        Err(error) => diagnostics.push(Diagnostic {
            component: None,
            severity: Severity::Error,
            message: format!("Legacy shared configuration cannot be migrated safely: {error}"),
            suggested_action: Some(
                "Move ambiguous values into the authoritative component environment files, then remove /etc/bts/bts.env."
                    .into(),
            ),
        }),
        Ok(None) => {}
    }
    for component in &state.installed_components {
        if let Some(config_name) = component.config_name() {
            let config = root.join("etc/bts").join(config_name);
            if !config.is_file() {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Error,
                    message: format!("{} configuration is missing.", component),
                    suggested_action: Some(format!("Run: sudo bts-install configure {component}")),
                });
            } else if fs::metadata(&config)
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o007 != 0)
            {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Error,
                    message: format!("{} configuration is accessible to other users.", component),
                    suggested_action: Some(format!("Run: sudo chmod 0640 {}", config.display())),
                });
            } else if let Err(error) = validate_component_configuration(&config, *component) {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Error,
                    message: format!("{} configuration is invalid: {error}", component),
                    suggested_action: Some(format!("Run: sudo bts-install configure {component}")),
                });
            } else {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Ok,
                    message: format!("{} configuration is valid.", component),
                    suggested_action: None,
                });
            }
        }

        let binary = root.join(format!(
            "usr/lib/bts/components/{component}/current/bin/{}",
            component.binary()
        ));
        if !binary.is_file() {
            diagnostics.push(Diagnostic {
                component: Some(*component),
                severity: Severity::Error,
                message: format!("{} release is incomplete.", component),
                suggested_action: Some(format!("Run: sudo bts-install upgrade {component}")),
            });
        }
        if let Some(unit_name) = component.unit() {
            let unit = root.join("usr/lib/systemd/system").join(unit_name);
            if !unit.is_file() {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Error,
                    message: format!("{} service unit is missing.", component),
                    suggested_action: Some(format!("Run: sudo bts-install upgrade {component}")),
                });
            }
        }

        if root == Path::new("/") && component.unit().is_some() {
            let unit = component.unit().expect("checked service component");
            let account = if *component == Component::Display {
                "bts-display"
            } else {
                "bts"
            };
            if system
                .output("getent", &["passwd".into(), account.into()])
                .is_err()
            {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Error,
                    message: format!("Required account '{account}' is missing."),
                    suggested_action: Some(format!("Run: sudo bts-install add {component}")),
                });
            }
            if system
                .output("systemctl", &["is-enabled".into(), unit.into()])
                .is_err()
            {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Error,
                    message: format!("{} service is not enabled.", component),
                    suggested_action: Some(format!("Run: sudo systemctl enable {}", unit)),
                });
            }
            if system
                .output("systemctl", &["is-active".into(), unit.into()])
                .is_err()
            {
                diagnostics.push(Diagnostic {
                    component: Some(*component),
                    severity: Severity::Error,
                    message: format!("{} service is not active.", component),
                    suggested_action: Some(format!("Run: sudo systemctl restart {}", unit)),
                });
            }
            if *component == Component::Display {
                for executable in ["/usr/bin/cage", "/usr/bin/seatd"] {
                    if !system.exists(Path::new(executable)) {
                        diagnostics.push(Diagnostic {
                            component: Some(*component),
                            severity: Severity::Error,
                            message: format!("Display runtime dependency {executable} is missing."),
                            suggested_action: Some("Re-run: sudo bts-install add display".into()),
                        });
                    }
                }
                for path in ["/dev/dri", "/dev/tty1"] {
                    if !system.exists(Path::new(path)) {
                        diagnostics.push(Diagnostic {
                            component: Some(*component),
                            severity: Severity::Warning,
                            message: format!("Display device path {path} is unavailable."),
                            suggested_action: Some(
                                "Check DRM, tty1, seatd and display hardware access.".into(),
                            ),
                        });
                    }
                }
                if system
                    .output("getent", &["passwd".into(), "bts-display".into()])
                    .is_ok()
                    && system
                        .output("id", &["-nG".into(), "bts-display".into()])
                        .is_ok_and(|groups| !groups.split_whitespace().any(|group| group == "seat"))
                {
                    diagnostics.push(Diagnostic {
                        component: Some(*component),
                        severity: Severity::Warning,
                        message: "Display account is not a member of the seat group.".into(),
                        suggested_action: Some("Run: sudo usermod -aG seat bts-display".into()),
                    });
                }
            }
        }
    }
    let healthy = !diagnostics
        .iter()
        .any(|value| value.severity == Severity::Error);
    DoctorReport {
        schema_version: OUTPUT_SCHEMA_VERSION,
        healthy,
        diagnostics,
    }
}

fn validate_component_configuration(path: &Path, component: Component) -> anyhow::Result<()> {
    let values = crate::config::parse_environment(&fs::read_to_string(path)?)?;
    match component {
        Component::Core => {
            values
                .get("BTS_CORE_BIND")
                .ok_or_else(|| anyhow::anyhow!("BTS_CORE_BIND is missing"))?
                .parse::<std::net::SocketAddr>()?;
        }
        Component::Display => {
            crate::config::validate_display(&values)?;
            if let Some(arguments) = values.get("BTS_CAGE_ARGS") {
                crate::config::validate_cage_args(arguments)?;
            }
        }
        Component::Telephony => {
            crate::config::validate_telephony(&values)?;
            crate::config::validate_http_url(
                values
                    .get("BTS_CORE_URL")
                    .ok_or_else(|| anyhow::anyhow!("BTS_CORE_URL is missing"))?,
                "BTS_CORE_URL",
            )?;
        }
        Component::Addons => {
            crate::config::validate_http_url(
                values
                    .get("BTS_CORE_HTTP_URL")
                    .ok_or_else(|| anyhow::anyhow!("BTS_CORE_HTTP_URL is missing"))?,
                "BTS_CORE_HTTP_URL",
            )?;
            crate::config::validate_websocket_url(
                values
                    .get("BTS_CORE_WS_URL")
                    .ok_or_else(|| anyhow::anyhow!("BTS_CORE_WS_URL is missing"))?,
            )?;
        }
        Component::Cli => {}
    }
    Ok(())
}

fn read_endpoint(root: &Path, component: Component) -> Option<String> {
    let key = match component {
        Component::Core => "BTS_CORE_BIND",
        Component::Display => "BTS_CORE_WS_URL",
        Component::Telephony => "BTS_ARI_URL",
        Component::Addons => "BTS_CORE_HTTP_URL",
        Component::Cli => return None,
    };
    let text = fs::read_to_string(
        root.join("etc/bts").join(
            component
                .config_name()
                .expect("endpoint components have configuration"),
        ),
    )
    .ok()?;
    crate::config::parse_environment(&text).ok()?.remove(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        platform::{Architecture, Platform},
        system::RecordingSystem,
    };
    use tempfile::tempdir;

    #[test]
    fn status_machine_output_is_stable_and_redacts_secrets() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/bts")).unwrap();
        fs::write(
            root.path().join("etc/bts/telephony.env"),
            "BTS_ARI_URL=http://ari\nBTS_ARI_PASSWORD=secret\n",
        )
        .unwrap();
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        state.installed_components.insert(Component::Telephony);
        let report = status(root.path(), Some(&state), &mut RecordingSystem::default());
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("http://ari"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn doctor_reports_actions_for_common_failures() {
        let root = tempdir().unwrap();
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        state.installed_components.insert(Component::Display);
        let report = doctor(root.path(), Some(&state), &mut RecordingSystem::default());
        assert!(!report.healthy);
        assert!(
            report
                .diagnostics
                .iter()
                .filter(|value| value.severity == Severity::Error)
                .all(|value| value.suggested_action.is_some())
        );
    }

    #[test]
    fn doctor_reports_legacy_shared_configuration() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("etc/bts")).unwrap();
        fs::write(
            root.path().join("etc/bts/bts.env"),
            "BTS_CORE_BIND=0.0.0.0:3100\n",
        )
        .unwrap();
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        state.installed_components.insert(Component::Core);
        let report = doctor(root.path(), Some(&state), &mut RecordingSystem::default());
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("requires migration")
                && diagnostic.severity == Severity::Error
        }));
    }
}
