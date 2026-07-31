use std::process::Command;

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
