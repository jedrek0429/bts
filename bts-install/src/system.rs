use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

pub trait SystemAdapter {
    fn run(&mut self, program: &str, arguments: &[String]) -> Result<()>;
    fn run_quiet(&mut self, program: &str, arguments: &[String]) -> Result<()> {
        self.run(program, arguments)
    }
    fn output(&mut self, program: &str, arguments: &[String]) -> Result<String>;
    fn exists(&self, path: &Path) -> bool;
}

#[derive(Debug, Default)]
pub struct RealSystem;

impl SystemAdapter for RealSystem {
    fn run(&mut self, program: &str, arguments: &[String]) -> Result<()> {
        let status = Command::new(program)
            .args(arguments)
            .status()
            .with_context(|| format!("Could not run {program}"))?;
        if !status.success() {
            bail!("{program} failed with status {status}.");
        }
        Ok(())
    }

    fn run_quiet(&mut self, program: &str, arguments: &[String]) -> Result<()> {
        let output = Command::new(program)
            .args(arguments)
            .output()
            .with_context(|| format!("Could not run {program}"))?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            if detail.is_empty() {
                bail!("{program} failed with status {}.", output.status);
            }
            bail!("{program} failed with status {}: {detail}", output.status);
        }
        Ok(())
    }

    fn output(&mut self, program: &str, arguments: &[String]) -> Result<String> {
        let output = Command::new(program)
            .args(arguments)
            .output()
            .with_context(|| format!("Could not run {program}"))?;
        if !output.status.success() {
            bail!("{program} failed with status {}.", output.status);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().into())
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

#[derive(Debug, Default)]
pub struct RecordingSystem {
    pub commands: Vec<(String, Vec<String>)>,
    pub outputs: std::collections::BTreeMap<String, String>,
    pub root: PathBuf,
}

impl SystemAdapter for RecordingSystem {
    fn run(&mut self, program: &str, arguments: &[String]) -> Result<()> {
        self.commands.push((program.into(), arguments.to_vec()));
        Ok(())
    }
    fn run_quiet(&mut self, program: &str, arguments: &[String]) -> Result<()> {
        self.run(program, arguments)
    }
    fn output(&mut self, program: &str, arguments: &[String]) -> Result<String> {
        self.commands.push((program.into(), arguments.to_vec()));
        Ok(self.outputs.get(program).cloned().unwrap_or_default())
    }
    fn exists(&self, path: &Path) -> bool {
        fs::metadata(self.root.join(path.strip_prefix("/").unwrap_or(path))).is_ok()
    }
}

pub fn systemctl<S: SystemAdapter>(
    system: &mut S,
    root: &Path,
    verb: &str,
    units: &[&str],
) -> Result<()> {
    let mut arguments = Vec::new();
    if root != Path::new("/") {
        arguments.push(format!("--root={}", root.display()));
    }
    arguments.push(verb.into());
    arguments.extend(units.iter().map(|value| (*value).into()));
    system.run_quiet("systemctl", &arguments)
}

fn ensure_system_group<S: SystemAdapter>(system: &mut S, group: &str) -> Result<()> {
    if system
        .output("getent", &["group".into(), group.into()])
        .is_err()
    {
        system.run("groupadd", &["--system".into(), group.into()])?;
    }
    Ok(())
}

pub fn create_service_account<S: SystemAdapter>(
    system: &mut S,
    root: &Path,
    account: &str,
) -> Result<()> {
    if root != Path::new("/") {
        return Ok(());
    }
    ensure_system_group(system, account)?;
    if system.output("id", &["-u".into(), account.into()]).is_err() {
        system.run(
            "useradd",
            &[
                "--system".into(),
                "--gid".into(),
                account.into(),
                "--home-dir".into(),
                format!("/var/lib/{account}"),
                "--create-home".into(),
                "--shell".into(),
                "/usr/sbin/nologin".into(),
                account.into(),
            ],
        )?;
    }
    if account == "bts"
        && system
            .output("getent", &["group".into(), "asterisk".into()])
            .is_ok()
    {
        system.run("usermod", &["-aG".into(), "asterisk".into(), "bts".into()])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MissingSeatSystem {
        commands: Vec<(String, Vec<String>)>,
    }

    impl SystemAdapter for MissingSeatSystem {
        fn run(&mut self, program: &str, arguments: &[String]) -> Result<()> {
            self.commands.push((program.into(), arguments.to_vec()));
            Ok(())
        }

        fn output(&mut self, program: &str, arguments: &[String]) -> Result<String> {
            self.commands.push((program.into(), arguments.to_vec()));
            if program == "getent" && arguments == ["group", "seat"] {
                bail!("seat group missing");
            }
            Ok(String::new())
        }

        fn exists(&self, _path: &Path) -> bool {
            false
        }
    }

    #[test]
    fn display_account_does_not_invent_a_distribution_specific_seat_group() {
        let mut system = MissingSeatSystem::default();
        create_service_account(&mut system, Path::new("/"), "bts-display").unwrap();
        assert!(
            !system
                .commands
                .contains(&("groupadd".into(), vec!["--system".into(), "seat".into()]))
        );
    }
}
