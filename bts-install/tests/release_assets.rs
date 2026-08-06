use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::tempdir;

#[test]
fn generated_release_assets_and_manifest_are_consistent() {
    let status = Command::new("bash")
        .arg("../scripts/test-release-assets.sh")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("release asset test script should run");
    assert!(status.success());
}

#[test]
fn display_unit_expands_installer_managed_cage_arguments() {
    let unit = std::fs::read_to_string("../deploy/systemd/bts-display.service").unwrap();
    assert!(unit.contains("Environment=\"BTS_CAGE_ARGS=-m last\""));
    assert!(unit.contains("unbraced variable form expands to zero or more arguments"));
    assert!(unit.contains("ExecStart=/usr/bin/cage $BTS_CAGE_ARGS -- "));
    assert!(
        unit.find("Environment=\"BTS_CAGE_ARGS=-m last\"")
            < unit.find("EnvironmentFile=-/etc/bts/display.env"),
        "display.env must be able to override the default Cage arguments"
    );
}

#[test]
fn every_service_uses_only_its_component_environment_after_a_safe_default() {
    for component in ["core", "display", "telephony", "addons"] {
        let unit =
            std::fs::read_to_string(format!("../deploy/systemd/bts-{component}.service")).unwrap();
        assert!(!unit.contains("/etc/bts/bts.env"));
        let default = unit.find("Environment=RUST_LOG=info").unwrap();
        let component_file = unit
            .find(&format!("EnvironmentFile=-/etc/bts/{component}.env"))
            .unwrap();
        assert!(
            default < component_file,
            "{component}.env must be able to override RUST_LOG"
        );
        assert_eq!(unit.matches("EnvironmentFile=").count(), 1);
    }
}

#[test]
fn local_release_reinstalls_and_reconciles_offline() {
    let temporary = tempdir().unwrap();
    let assets = temporary.path().join("assets");
    let root = temporary.path().join("root");
    let fake_bin = temporary.path().join("bin");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::copy("/etc/os-release", root.join("etc/os-release")).unwrap();
    let systemctl = fake_bin.join("systemctl");
    fs::write(&systemctl, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();

    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => panic!("unsupported test architecture {other}"),
    };
    release_command(&[
        "component",
        "core",
        architecture,
        "/usr/bin/true",
        assets.to_str().unwrap(),
    ]);
    release_command(&[
        "installer",
        env!("CARGO_BIN_EXE_bts-install"),
        assets.to_str().unwrap(),
    ]);
    release_command(&["assemble", assets.to_str().unwrap()]);

    let install = [
        "--root",
        root.to_str().unwrap(),
        "--release-dir",
        assets.to_str().unwrap(),
        "--yes",
        "--no-start",
        "install",
        "custom",
        "--component",
        "core",
    ];
    assert!(
        installer_command(&install, &fake_bin)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        installer_command(
            &[
                "--root",
                root.to_str().unwrap(),
                "--yes",
                "uninstall",
                "core"
            ],
            &fake_bin,
        )
        .status()
        .unwrap()
        .success()
    );
    assert!(!root.join("var/lib/bts-install/state.json").exists());
    assert!(root.join("usr/lib/bts/components/core/releases").is_dir());

    assert!(
        installer_command(&install, &fake_bin)
            .status()
            .unwrap()
            .success()
    );
    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("var/lib/bts-install/state.json")).unwrap())
            .unwrap();
    assert_eq!(state["release_channel"], "local");

    let dry_run = installer_command(
        &[
            "--root",
            root.to_str().unwrap(),
            "--release-dir",
            assets.to_str().unwrap(),
            "--yes",
            "--no-start",
            "--dry-run",
            "--json",
            "upgrade",
        ],
        &fake_bin,
    )
    .output()
    .unwrap();
    assert!(dry_run.status.success());
    let plan: serde_json::Value = serde_json::from_slice(&dry_run.stdout).unwrap();
    assert_eq!(plan["actions"], serde_json::json!([]));

    let doctor = installer_command(
        &["--root", root.to_str().unwrap(), "--json", "doctor"],
        &fake_bin,
    )
    .output()
    .unwrap();
    assert!(doctor.status.success());
    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(report["healthy"], true);
    assert!(
        report["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| {
                item["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("verified local release"))
            })
    );
}

fn release_command(arguments: &[&str]) {
    assert!(
        Command::new("../scripts/build-release")
            .args(arguments)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .unwrap()
            .success()
    );
}

fn installer_command(arguments: &[&str], fake_bin: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_bts-install"));
    let path = std::env::var_os("PATH").unwrap_or_default();
    command.args(arguments).env(
        "PATH",
        std::env::join_paths(
            std::iter::once(PathBuf::from(fake_bin)).chain(std::env::split_paths(&path)),
        )
        .unwrap(),
    );
    command
}
