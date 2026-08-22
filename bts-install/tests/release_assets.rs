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
fn display_unit_does_not_require_optional_distribution_groups() {
    let unit = std::fs::read_to_string("../deploy/systemd/bts-display.service").unwrap();
    assert!(!unit.contains("SupplementaryGroups="));
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

    let architecture = test_architecture();
    release_command(&[
        "component",
        "core",
        architecture,
        "/usr/bin/true",
        assets.to_str().unwrap(),
    ]);
    stage_test_installers(&assets);
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
    let current = root.join("usr/lib/bts/components/core/current");
    let first_activation = fs::read_link(&current).unwrap();
    release_command(&[
        "component",
        "core",
        architecture,
        "/usr/bin/false",
        assets.to_str().unwrap(),
    ]);
    release_command(&["assemble", assets.to_str().unwrap()]);
    assert!(
        installer_command(&install, &fake_bin)
            .status()
            .unwrap()
            .success()
    );
    let replacement_activation = fs::read_link(&current).unwrap();
    assert_ne!(first_activation, replacement_activation);
    assert!(
        installer_command(&install, &fake_bin)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::read_link(&current).unwrap(), replacement_activation);
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

#[test]
fn local_release_installs_cli_without_runtime_components() {
    let temporary = tempdir().unwrap();
    let assets = temporary.path().join("assets");
    let root = temporary.path().join("root");
    let fake_bin = temporary.path().join("bin");
    fs::create_dir_all(root.join("etc")).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::copy("/etc/os-release", root.join("etc/os-release")).unwrap();
    let systemctl = fake_bin.join("systemctl");
    fs::write(&systemctl, "#!/bin/sh\nexit 99\n").unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();
    let architecture = test_architecture();
    release_command(&[
        "component",
        "cli",
        architecture,
        "/usr/bin/true",
        assets.to_str().unwrap(),
    ]);
    stage_test_installers(&assets);
    release_command(&["assemble", assets.to_str().unwrap()]);

    assert!(
        installer_command(
            &[
                "--root",
                root.to_str().unwrap(),
                "--release-dir",
                assets.to_str().unwrap(),
                "--yes",
                "--no-start",
                "install",
                "custom",
                "--component",
                "cli",
            ],
            &fake_bin,
        )
        .status()
        .unwrap()
        .success()
    );
    assert_eq!(
        fs::read_link(root.join("usr/bin/btscli")).unwrap(),
        PathBuf::from("../lib/bts/components/cli/current/bin/btscli")
    );
    assert!(
        root.join("usr/lib/bts/components/cli/current/bin/btscli")
            .is_file()
    );
    assert!(!root.join("etc/bts/cli.env").exists());
    assert!(!root.join("usr/lib/systemd/system/bts-cli.service").exists());
}

#[test]
fn fresh_telephony_install_configures_unavailable_external_services() {
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
    let secret = temporary.path().join("telephony.env");
    fs::write(
        &secret,
        concat!(
            "BTS_ARI_URL=http://127.0.0.1:1\n",
            "BTS_ARI_USERNAME=bts\n",
            "BTS_ARI_PASSWORD=installation-secret\n",
            "BTS_KOKORO_URL=http://127.0.0.1:2/v1/audio/speech\n",
        ),
    )
    .unwrap();
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
    let architecture = test_architecture();
    release_command(&[
        "component",
        "telephony",
        architecture,
        "/usr/bin/true",
        assets.to_str().unwrap(),
    ]);
    stage_test_installers(&assets);
    release_command(&["assemble", assets.to_str().unwrap()]);

    let output = installer_command(
        &[
            "--root",
            root.to_str().unwrap(),
            "--release-dir",
            assets.to_str().unwrap(),
            "--secret-file",
            secret.to_str().unwrap(),
            "--core-http-url",
            "http://127.0.0.1:3",
            "--yes",
            "--no-start",
            "install",
            "telephony",
        ],
        &fake_bin,
    )
    .output()
    .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Telephony configuration saved"));
    assert!(stdout.contains("BTS Telephony is installed but not ready yet"));
    assert!(stdout.contains("docs/telephony-setup.md"));
    assert!(!stdout.contains("installation-secret"));
    let values = fs::read_to_string(root.join("etc/bts/telephony.env")).unwrap();
    assert!(values.contains("BTS_ARI_URL=\"http://127.0.0.1:1\""));
    assert!(values.contains("BTS_KOKORO_URL=\"http://127.0.0.1:2/v1/audio/speech\""));
    assert!(values.contains("BTS_ARI_PASSWORD=\"installation-secret\""));
}

fn test_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => panic!("unsupported test architecture {other}"),
    }
}

fn stage_test_installers(assets: &Path) {
    for architecture in ["x86_64", "aarch64"] {
        release_command(&[
            "installer",
            architecture,
            env!("CARGO_BIN_EXE_bts-install"),
            assets.to_str().unwrap(),
        ]);
    }
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
