use std::path::Path;

use anyhow::Result;

use crate::system::SystemAdapter;

pub fn reconcile_telephony_runtime_access<S: SystemAdapter>(
    system: &mut S,
    root: &Path,
    generated: &Path,
) -> Result<bool> {
    if root != Path::new("/") {
        return Ok(false);
    }

    let mut membership_changed = false;
    if let Some(parent) = generated.parent() {
        for ancestor in parent.ancestors() {
            if ancestor == Path::new("/") {
                break;
            }
            let group = match system.output(
                "stat",
                &["-c".into(), "%G".into(), ancestor.display().to_string()],
            ) {
                Ok(group) => group,
                Err(_) => continue,
            };
            let group = group.trim();
            if group.is_empty() || matches!(group, "root" | "bts" | "UNKNOWN") {
                continue;
            }
            if system
                .output("getent", &["group".into(), group.into()])
                .is_err()
            {
                continue;
            }
            let memberships = system
                .output("id", &["-nG".into(), "bts".into()])
                .unwrap_or_default();
            if !memberships.split_whitespace().any(|value| value == group) {
                system.run("usermod", &["-aG".into(), group.into(), "bts".into()])?;
                membership_changed = true;
            }
            break;
        }
    }

    system.run(
        "install",
        &[
            "-d".into(),
            "-o".into(),
            "bts".into(),
            "-g".into(),
            "bts".into(),
            "-m".into(),
            "0755".into(),
            generated.display().to_string(),
        ],
    )?;

    Ok(membership_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::RecordingSystem;

    #[test]
    fn restrictive_asterisk_parent_adds_runtime_group_and_owns_only_generated_namespace() {
        let mut system = RecordingSystem::default();
        system.outputs.insert("stat".into(), "asterisk".into());
        system
            .outputs
            .insert("getent".into(), "asterisk:x:995:bts".into());
        system.outputs.insert("id".into(), "bts".into());

        let changed = reconcile_telephony_runtime_access(
            &mut system,
            Path::new("/"),
            Path::new("/var/lib/asterisk/sounds/en/bts-generated"),
        )
        .unwrap();

        assert!(changed);
        assert!(system.commands.iter().any(|(program, arguments)| {
            program == "usermod"
                && arguments
                    .iter()
                    .map(String::as_str)
                    .eq(["-aG", "asterisk", "bts"])
        }));
        assert!(system.commands.iter().any(|(program, arguments)| {
            program == "install"
                && arguments.ends_with(&["/var/lib/asterisk/sounds/en/bts-generated".into()])
        }));
    }

    #[test]
    fn existing_runtime_group_membership_is_idempotent() {
        let mut system = RecordingSystem::default();
        system.outputs.insert("stat".into(), "asterisk".into());
        system
            .outputs
            .insert("getent".into(), "asterisk:x:995:bts".into());
        system.outputs.insert("id".into(), "bts asterisk".into());

        let changed = reconcile_telephony_runtime_access(
            &mut system,
            Path::new("/"),
            Path::new("/var/lib/asterisk/sounds/en/bts-generated"),
        )
        .unwrap();

        assert!(!changed);
        assert!(
            !system
                .commands
                .iter()
                .any(|(program, _)| program == "usermod")
        );
    }

    #[test]
    fn non_host_root_does_not_mutate_runtime_accounts() {
        let root = tempfile::tempdir().unwrap();
        let mut system = RecordingSystem::default();

        let changed = reconcile_telephony_runtime_access(
            &mut system,
            root.path(),
            Path::new("/var/lib/asterisk/sounds/en/bts-generated"),
        )
        .unwrap();

        assert!(!changed);
        assert!(system.commands.is_empty());
    }
}
