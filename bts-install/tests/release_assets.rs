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
