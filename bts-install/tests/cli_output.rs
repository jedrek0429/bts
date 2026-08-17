use std::process::Command;

#[test]
fn help_version_licence_and_machine_output_contracts() {
    let binary = env!("CARGO_BIN_EXE_bts-install");
    let help = Command::new(binary).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("GPL-3.0-or-later"));
    assert!(help.contains("doctor"));

    let version = Command::new(binary).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8(version.stdout).unwrap().trim(),
        concat!("bts-install ", env!("CARGO_PKG_VERSION"))
    );

    let licence = Command::new(binary).arg("licence").output().unwrap();
    assert!(
        String::from_utf8(licence.stdout)
            .unwrap()
            .contains("GNU GENERAL PUBLIC LICENSE")
    );

    let root = tempfile::tempdir().unwrap();
    let status = Command::new(binary)
        .args(["status", "--json", "--root", root.path().to_str().unwrap()])
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        value["schema_version"],
        bts_install::diagnostics::OUTPUT_SCHEMA_VERSION
    );
    let quiet = Command::new(binary)
        .args(["status", "--quiet", "--root", root.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(quiet.stdout.is_empty());

    let json_text = String::from_utf8(status.stdout).unwrap();
    assert!(!json_text.contains('\u{1b}'));
    let plain = Command::new(binary)
        .args(["status", "--root", root.path().to_str().unwrap()])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(plain.status.success());
    assert!(!String::from_utf8(plain.stdout).unwrap().contains('\u{1b}'));
}
