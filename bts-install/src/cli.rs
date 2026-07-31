use std::{path::PathBuf, str::FromStr};

use anyhow::{Context, Result, bail};

use crate::{
    DEFAULT_CHANNEL, DEFAULT_REPOSITORY,
    model::{Component, Role},
};

pub const HELP: &str = r#"bts-install — modular BTS deployment manager (GPL-3.0-or-later)

Usage:
  bts-install [OPTIONS] install [ROLE] [--component COMPONENT]...
  bts-install [OPTIONS] add COMPONENT...
  bts-install [OPTIONS] remove COMPONENT... [--purge]
  bts-install [OPTIONS] upgrade [COMPONENT...]
  bts-install [OPTIONS] configure [COMPONENT]
  bts-install [OPTIONS] status
  bts-install [OPTIONS] doctor
  bts-install [OPTIONS] uninstall [COMPONENT...] [--purge]
  bts-install licence
  bts-install warranty

Roles: full, server, display, custom
Components: core, display, telephony, addons

Options:
  --component COMPONENT  Select a component for a custom installation
  --core-url URL         Remote Core WebSocket URL for Display
  --core-http-url URL    Remote Core HTTP URL for Addons or Telephony
  --core-ws-url URL      Remote Core WebSocket URL for Display or Addons
  --cage-args ARGS       Override Cage arguments for Display (default: -m last)
  --repository OWNER/REPO  Release repository (default: jedrek0429/bts)
  --channel CHANNEL      Release channel or explicit version tag (default: stable)
  --release-dir PATH     Install verified release assets from a local directory
  --root PATH            Alternate installation root (testing/recovery only)
  --yes                  Confirm planned host changes non-interactively
  --no-start             Install and enable without starting services
  --dry-run              Print the resolved plan without changing the machine
  --json                 Emit stable machine-readable JSON
  --quiet                Print errors only
  --secret-file PATH     Read telephony environment from a protected file
  --secret-fd FD         Read telephony environment from an inherited file descriptor
  --purge                Remove installer-owned configuration as well as components
  -h, --help             Show this help
  -V, --version          Show the installer version

This is free software; run 'bts-install licence' for copying conditions and
'bts-install warranty' for the no-warranty terms.
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub command: Command,
    pub repository: String,
    pub channel: String,
    pub release_dir: Option<PathBuf>,
    pub root: PathBuf,
    pub yes: bool,
    pub no_start: bool,
    pub dry_run: bool,
    pub json: bool,
    pub quiet: bool,
    pub core_http_url: Option<String>,
    pub core_ws_url: Option<String>,
    pub cage_args: Option<String>,
    pub secret_input: Option<SecretInput>,
    pub purge: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretInput {
    File(PathBuf),
    FileDescriptor(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Install {
        role: Option<Role>,
        components: Vec<Component>,
    },
    Add(Vec<Component>),
    Remove(Vec<Component>),
    Upgrade(Vec<Component>),
    Configure(Option<Component>),
    Status,
    Doctor,
    Uninstall(Vec<Component>),
    Licence,
    Warranty,
    Help,
    Version,
}

impl Cli {
    pub fn parse<I, S>(values: I) -> Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut args: Vec<String> = values.into_iter().map(Into::into).collect();
        if !args.is_empty() {
            args.remove(0);
        }
        let mut repository = DEFAULT_REPOSITORY.to_owned();
        let mut channel = DEFAULT_CHANNEL.to_owned();
        let mut release_dir = None;
        let mut root = PathBuf::from("/");
        let mut yes = false;
        let mut no_start = false;
        let mut dry_run = false;
        let mut json = false;
        let mut quiet = false;
        let mut core_http_url = None;
        let mut core_ws_url = None;
        let mut cage_args = None;
        let mut secret_input = None;
        let mut purge = false;
        let mut components = Vec::new();
        let mut positional = Vec::new();
        let mut index = 0;

        while index < args.len() {
            let arg = &args[index];
            let mut take_value = |name: &str| -> Result<String> {
                index += 1;
                args.get(index)
                    .cloned()
                    .with_context(|| format!("{name} requires a value."))
            };
            match arg.as_str() {
                "-h" | "--help" => return Ok(Self::simple(Command::Help)),
                "-V" | "--version" => return Ok(Self::simple(Command::Version)),
                "--repository" => repository = take_value("--repository")?,
                "--channel" => channel = take_value("--channel")?,
                "--release-dir" => release_dir = Some(PathBuf::from(take_value("--release-dir")?)),
                "--root" => root = PathBuf::from(take_value("--root")?),
                "--yes" => yes = true,
                "--no-start" => no_start = true,
                "--dry-run" => dry_run = true,
                "--json" => json = true,
                "--quiet" => quiet = true,
                "--purge" => purge = true,
                "--core-url" | "--core-ws-url" => core_ws_url = Some(take_value(arg)?),
                "--core-http-url" => core_http_url = Some(take_value("--core-http-url")?),
                "--cage-args" => cage_args = Some(take_value("--cage-args")?),
                "--secret-file" => {
                    if secret_input.is_some() {
                        bail!("Use only one secure secret input source.");
                    }
                    secret_input = Some(SecretInput::File(PathBuf::from(take_value(
                        "--secret-file",
                    )?)));
                }
                "--secret-fd" => {
                    if secret_input.is_some() {
                        bail!("Use only one secure secret input source.");
                    }
                    let value = take_value("--secret-fd")?;
                    secret_input = Some(SecretInput::FileDescriptor(
                        value
                            .parse()
                            .context("--secret-fd must be a non-negative integer.")?,
                    ));
                }
                "--component" => components.push(Component::from_str(&take_value("--component")?)?),
                value if value.starts_with('-') => bail!("Unknown option '{value}'."),
                value => positional.push(value.to_owned()),
            }
            index += 1;
        }

        if positional.is_empty() {
            return Ok(Self::simple(Command::Help));
        }
        let name = positional.remove(0);
        let command = match name.as_str() {
            "install" => {
                let role = match positional.as_slice() {
                    [] => None,
                    [value] => Some(Role::from_str(value)?),
                    _ => bail!("install accepts at most one role."),
                };
                Command::Install { role, components }
            }
            "add" => Command::Add(parse_components(&positional, true)?),
            "remove" => Command::Remove(parse_components(&positional, true)?),
            "upgrade" => Command::Upgrade(parse_components(&positional, false)?),
            "configure" => Command::Configure(match positional.as_slice() {
                [] => None,
                [value] => Some(Component::from_str(value)?),
                _ => bail!("configure accepts at most one component."),
            }),
            "status" if positional.is_empty() => Command::Status,
            "doctor" if positional.is_empty() => Command::Doctor,
            "uninstall" => Command::Uninstall(parse_components(&positional, false)?),
            "licence" | "license" if positional.is_empty() => Command::Licence,
            "warranty" if positional.is_empty() => Command::Warranty,
            _ => bail!("Unknown command or unexpected argument '{name}'."),
        };

        if release_dir.is_some()
            && !matches!(
                command,
                Command::Install { .. } | Command::Add(_) | Command::Upgrade(_)
            )
        {
            bail!("--release-dir is only valid for install, add and upgrade.");
        }
        validate_options(
            &command,
            no_start,
            core_http_url.as_deref(),
            core_ws_url.as_deref(),
            cage_args.as_deref(),
            secret_input.as_ref(),
            purge,
        )?;
        if !repository.contains('/') || repository.starts_with('/') || repository.ends_with('/') {
            bail!("--repository must use OWNER/REPOSITORY form.");
        }
        if channel != "stable"
            && (!channel.starts_with('v') || !crate::manifest::is_release_version(&channel))
        {
            bail!("The installer accepts channel 'stable' or an explicit semantic version tag.");
        }

        Ok(Self {
            command,
            repository,
            channel,
            release_dir,
            root,
            yes,
            no_start,
            dry_run,
            json,
            quiet,
            core_http_url,
            core_ws_url,
            cage_args,
            secret_input,
            purge,
        })
    }

    fn simple(command: Command) -> Self {
        Self {
            command,
            repository: DEFAULT_REPOSITORY.into(),
            channel: DEFAULT_CHANNEL.into(),
            release_dir: None,
            root: "/".into(),
            yes: false,
            no_start: false,
            dry_run: false,
            json: false,
            quiet: false,
            core_http_url: None,
            core_ws_url: None,
            cage_args: None,
            secret_input: None,
            purge: false,
        }
    }
}

fn parse_components(values: &[String], required: bool) -> Result<Vec<Component>> {
    if required && values.is_empty() {
        bail!("At least one component is required.");
    }
    values
        .iter()
        .map(|value| Component::from_str(value))
        .collect()
}

fn validate_options(
    command: &Command,
    no_start: bool,
    core_http_url: Option<&str>,
    core_ws_url: Option<&str>,
    cage_args: Option<&str>,
    secret: Option<&SecretInput>,
    purge: bool,
) -> Result<()> {
    if no_start
        && !matches!(
            command,
            Command::Install { .. } | Command::Add(_) | Command::Upgrade(_)
        )
    {
        bail!("--no-start is only valid for install, add and upgrade.");
    }
    if (core_http_url.is_some() || core_ws_url.is_some())
        && !matches!(
            command,
            Command::Install { .. }
                | Command::Add(_)
                | Command::Configure(Some(
                    Component::Display | Component::Telephony | Component::Addons
                ))
        )
    {
        bail!(
            "Core endpoint options are only valid while installing, adding or configuring a Core client component."
        );
    }
    if cage_args.is_some() && !command_uses_display(command) {
        bail!("--cage-args is only valid while installing, adding or configuring Display.");
    }
    if let Some(value) = cage_args {
        crate::config::validate_cage_args(value)?;
    }
    if secret.is_some()
        && !matches!(
            command,
            Command::Configure(None | Some(Component::Telephony))
        )
    {
        bail!("Secure secret input is only valid when configuring Telephony.");
    }
    if purge && !matches!(command, Command::Remove(_) | Command::Uninstall(_)) {
        bail!("--purge is only valid for remove and uninstall.");
    }
    Ok(())
}

fn command_uses_display(command: &Command) -> bool {
    match command {
        Command::Install { role, components } => {
            matches!(role, Some(Role::Full | Role::Display))
                || components.contains(&Component::Display)
        }
        Command::Add(components) => components.contains(&Component::Display),
        Command::Configure(Some(Component::Display)) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli> {
        Cli::parse(args.iter().copied())
    }

    #[test]
    fn parses_documented_install_forms() {
        let cli = parse(&[
            "bts-install",
            "install",
            "display",
            "--core-url",
            "ws://core/events",
            "--yes",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Install {
                role: Some(Role::Display),
                ..
            }
        ));
        assert!(cli.yes);
        let cli = parse(&[
            "bts-install",
            "configure",
            "display",
            "--cage-args",
            "-m extend -s",
        ])
        .unwrap();
        assert_eq!(cli.cage_args.as_deref(), Some("-m extend -s"));
        let cli = parse(&[
            "bts-install",
            "install",
            "--component",
            "core",
            "--component",
            "addons",
        ])
        .unwrap();
        assert!(
            matches!(cli.command, Command::Install { components, .. } if components.len() == 2)
        );
        let cli = parse(&[
            "bts-install",
            "install",
            "--component",
            "addons",
            "--core-http-url",
            "http://core:3100",
            "--core-ws-url",
            "ws://core:3100/api/v1/events/ws",
        ])
        .unwrap();
        assert_eq!(cli.core_http_url.as_deref(), Some("http://core:3100"));
        assert!(cli.core_ws_url.is_some());
    }

    #[test]
    fn help_exposes_every_command_and_legal_information() {
        for text in [
            "install",
            "add",
            "remove",
            "upgrade",
            "configure",
            "status",
            "doctor",
            "uninstall",
            "licence",
            "warranty",
            "GPL-3.0-or-later",
        ] {
            assert!(HELP.contains(text), "missing {text}");
        }
    }

    #[test]
    fn rejects_invalid_combinations_and_secret_arguments() {
        assert!(parse(&["bts-install", "status", "--no-start"]).is_err());
        assert!(parse(&["bts-install", "install", "display", "--purge"]).is_err());
        assert!(
            parse(&[
                "bts-install",
                "configure",
                "display",
                "--secret-file",
                "/tmp/x"
            ])
            .is_err()
        );
        assert!(parse(&["bts-install", "install", "server", "--cage-args", "-s"]).is_err());
        assert!(
            parse(&[
                "bts-install",
                "configure",
                "telephony",
                "--ari-password",
                "secret"
            ])
            .is_err()
        );
    }

    #[test]
    fn only_accepts_stable_or_version_channels() {
        assert!(parse(&["bts-install", "status", "--channel", "v0.3.7"]).is_ok());
        assert!(parse(&["bts-install", "status", "--channel", "v0.4.0"]).is_ok());
        assert!(parse(&["bts-install", "status", "--channel", "v0.4.0-rc.1"]).is_ok());
        assert!(parse(&["bts-install", "status", "--channel", "main"]).is_err());
    }

    #[test]
    fn local_release_directories_are_lifecycle_inputs_only() {
        let parsed = parse(&[
            "bts-install",
            "install",
            "display",
            "--release-dir",
            "/tmp/bts-release",
        ])
        .unwrap();
        assert_eq!(parsed.release_dir, Some("/tmp/bts-release".into()));
        assert!(parse(&["bts-install", "status", "--release-dir", "/tmp/bts-release"]).is_err());
    }
}
