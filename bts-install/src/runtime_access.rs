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
