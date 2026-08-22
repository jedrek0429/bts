const ENTRY_SOURCE: &str = include_str!("../src/entry.rs");

#[test]
fn mutating_operations_recover_begin_and_commit_durable_transactions() {
    assert!(ENTRY_SOURCE.contains("transaction::recover_pending(&cli.root)?"));
    assert!(ENTRY_SOURCE.contains("transaction::HostTransaction::begin(&cli.root)?"));
    assert!(ENTRY_SOURCE.contains("transaction.commit()?"));
    assert!(ENTRY_SOURCE.contains("transaction.rollback()"));
}

#[test]
fn telephony_runtime_access_is_reconciled_at_the_installer_boundary() {
    assert!(ENTRY_SOURCE.contains("runtime_access::reconcile_telephony_runtime_access"));
}
