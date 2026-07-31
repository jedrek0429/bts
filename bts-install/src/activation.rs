use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};

use crate::model::Component;

#[derive(Debug, Clone)]
pub struct Activation {
    pub component: Component,
    pub previous: Option<PathBuf>,
    pub current: PathBuf,
}

pub fn activate(root: &Path, component: Component, version: &str) -> Result<Activation> {
    ensure!(
        version.starts_with("0.3.")
            && version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')),
        "Unsafe or incompatible activation version '{version}'."
    );
    let base = rooted(root, &format!("/usr/lib/bts/components/{component}"));
    let release = base.join("releases").join(version);
    ensure!(
        release.is_dir(),
        "Staged {} release {} is incomplete.",
        component,
        version
    );
    ensure!(
        release.join("bin").join(component.binary()).is_file(),
        "Staged {} release {} has no binary.",
        component,
        version
    );
    fs::create_dir_all(&base)?;
    let current = base.join("current");
    let previous = fs::read_link(&current).ok();
    let temporary = base.join(format!(".current.{}", std::process::id()));
    let _ = fs::remove_file(&temporary);
    symlink(Path::new("releases").join(version), &temporary)?;
    fs::rename(&temporary, &current).context("Could not atomically activate staged component")?;
    Ok(Activation {
        component,
        previous,
        current,
    })
}

pub fn rollback(activation: &Activation) -> Result<()> {
    let Some(previous) = &activation.previous else {
        bail!(
            "No previous {} release is available for rollback.",
            activation.component
        );
    };
    let base = activation
        .current
        .parent()
        .context("Activation link has no parent")?;
    let temporary = base.join(format!(".rollback.{}", std::process::id()));
    let _ = fs::remove_file(&temporary);
    symlink(previous, &temporary)?;
    fs::rename(temporary, &activation.current)
        .context("Could not restore previous component release")?;
    Ok(())
}

fn rooted(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn stage(root: &Path, version: &str) {
        let path = root.join(format!(
            "usr/lib/bts/components/core/releases/{version}/bin"
        ));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("bts-core"), "binary").unwrap();
    }

    #[test]
    fn staged_activation_and_rollback_are_atomic() {
        let root = tempdir().unwrap();
        stage(root.path(), "0.3.0");
        stage(root.path(), "0.3.1");
        let first = activate(root.path(), Component::Core, "0.3.0").unwrap();
        assert!(first.previous.is_none());
        let second = activate(root.path(), Component::Core, "0.3.1").unwrap();
        assert_eq!(
            fs::read_link(&second.current).unwrap(),
            PathBuf::from("releases/0.3.1")
        );
        rollback(&second).unwrap();
        assert_eq!(
            fs::read_link(&second.current).unwrap(),
            PathBuf::from("releases/0.3.0")
        );
    }

    #[test]
    fn never_activates_incomplete_stage() {
        let root = tempdir().unwrap();
        fs::create_dir_all(
            root.path()
                .join("usr/lib/bts/components/core/releases/0.3.0"),
        )
        .unwrap();
        assert!(activate(root.path(), Component::Core, "0.3.0").is_err());
    }
}
