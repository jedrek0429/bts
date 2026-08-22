const MAIN_SOURCE: &str = include_str!("../src/main.rs");

#[test]
fn mutating_operations_recover_and_commit_durable_transactions() {
    assert!(MAIN_SOURCE.contains("transaction::recover_pending(&cli.root)?"));
    assert!(MAIN_SOURCE.contains("transaction::HostTransaction::begin(&cli.root)?"));
    assert!(MAIN_SOURCE.contains("transaction::commit_pending(&cli.root)?"));
}

#[test]
fn telephony_reconciliation_runs_inside_installer_lifecycle() {
    assert!(MAIN_SOURCE.contains("reconcile_telephony_runtime_access(cli)?"));
    assert!(MAIN_SOURCE.contains("BTS_ASTERISK_GENERATED_SOUNDS_DIR"));
    assert!(MAIN_SOURCE.contains("usermod"));
}
