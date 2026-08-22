use std::path::Path;

use bts_install::{runtime_access::reconcile_telephony_runtime_access, system::RecordingSystem};

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
