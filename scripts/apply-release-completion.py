#!/usr/bin/env python3
from pathlib import Path


def rep(path, old, new, count=1):
    p = Path(path)
    text = p.read_text()
    found = text.count(old)
    if found < count:
        raise SystemExit(f"{path}: expected at least {count} occurrences, found {found}: {old[:100]!r}")
    for _ in range(count):
        text = text.replace(old, new, 1)
    p.write_text(text)


# #89: durable transaction import and recovery before any new mutation.
rep(
    "bts-install/src/main.rs",
    "    system::{RealSystem, SystemAdapter, create_service_account, systemctl},\n};",
    "    system::{RealSystem, SystemAdapter, create_service_account, systemctl},\n    transaction,\n};",
)
rep(
    "bts-install/src/main.rs",
    "    let (platform, architecture) = detect_host(&cli.root)?;\n\n    match &cli.command {",
    '''    let (platform, architecture) = detect_host(&cli.root)?;
    if transaction::recover_pending(&cli.root)? {
        state = InstallerState::load(&state_path)?;
        if !cli.quiet && !cli.json {
            println!("Recovered an interrupted installer transaction before applying the new operation.");
        }
    }

    match &cli.command {''',
)

# Plan-based migrations happen only after the transaction journal exists.
p = Path("bts-install/src/main.rs")
text = p.read_text()
text = text.replace("                migrate_legacy_configuration(&cli, &plan.after)?;\n", "")
text = text.replace("                migrate_legacy_configuration(&cli, &plan.before)?;\n", "")
# Upgrade migration is also moved under its transaction.
text = text.replace(
    '''            if !cli.dry_run {
                migrate_legacy_configuration(&cli, &current.installed_components)?;
            } else {
                config::plan_legacy_environment_migration(
                    &cli.root,
                    &current.installed_components,
                )?;
            }
''',
    '''            if cli.dry_run {
                config::plan_legacy_environment_migration(
                    &cli.root,
                    &current.installed_components,
                )?;
            }
''',
)
p.write_text(text)

rep(
    "bts-install/src/main.rs",
    "                next.write_atomic(&state_path)?;\n                state = Some(next);",
    "                next.write_atomic(&state_path)?;\n                transaction::commit_pending(&cli.root)?;\n                state = Some(next);",
    count=2,
)
rep(
    "bts-install/src/main.rs",
    "                persist_uninstall_state(&state_path, &mut state, next)?;",
    "                persist_uninstall_state(&state_path, &mut state, next)?;\n                transaction::commit_pending(&cli.root)?;",
)
rep(
    "bts-install/src/main.rs",
    "            if !cli.dry_run {\n                current.write_atomic(&state_path)?;\n            }",
    "            if !cli.dry_run {\n                current.write_atomic(&state_path)?;\n                transaction::commit_pending(&cli.root)?;\n            }",
)

rep(
    "bts-install/src/main.rs",
    '''async fn execute_plan(
    cli: &Cli,
    plan: &InstallationPlan,
    state: &mut InstallerState,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
    refresh_existing: bool,
) -> Result<()> {
    let added: Vec<_> = plan.after.difference(&plan.before).copied().collect();''',
    '''async fn execute_plan(
    cli: &Cli,
    plan: &InstallationPlan,
    state: &mut InstallerState,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
    refresh_existing: bool,
) -> Result<()> {
    let transaction = transaction::HostTransaction::begin(&cli.root)?;
    match execute_plan_inner(cli, plan, state, platform, architecture, refresh_existing).await {
        Ok(()) => {
            drop(transaction);
            Ok(())
        }
        Err(error) => {
            let rollback = transaction.rollback();
            anyhow::bail!(
                "Installer transaction failed: {error:#}; rollback {}.",
                if rollback.is_ok() { "succeeded" } else { "failed" }
            )
        }
    }
}

async fn execute_plan_inner(
    cli: &Cli,
    plan: &InstallationPlan,
    state: &mut InstallerState,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
    refresh_existing: bool,
) -> Result<()> {
    let migration_components = if plan
        .actions
        .iter()
        .any(|action| matches!(action, Action::RemoveComponent { .. }))
    {
        &plan.before
    } else {
        &plan.after
    };
    migrate_legacy_configuration(cli, migration_components)?;
    let added: Vec<_> = plan.after.difference(&plan.before).copied().collect();''',
)

# #93: reconcile the actual Asterisk traversal group and BTS-owned generated directory.
rep(
    "bts-install/src/main.rs",
    "            if let Some(unit) = component.unit() {\n                systemctl(&mut system, &cli.root, \"enable\", &[unit])?;\n            }",
    "            if component == Component::Telephony {\n                reconcile_telephony_runtime_access(cli)?;\n                changed.insert(Component::Telephony);\n            }\n            if let Some(unit) = component.unit() {\n                systemctl(&mut system, &cli.root, \"enable\", &[unit])?;\n            }",
)
rep(
    "bts-install/src/main.rs",
    "        migrate_legacy_voice_assets(cli)?;\n    }\n    let changed = activations",
    "        migrate_legacy_voice_assets(cli)?;\n        reconcile_telephony_runtime_access(cli)?;\n    }\n    let mut changed = activations",
)
rep(
    "bts-install/src/main.rs",
    "fn migrate_legacy_voice_assets(cli: &Cli) -> Result<()> {",
    '''fn reconcile_telephony_runtime_access(cli: &Cli) -> Result<()> {
    if cli.root != Path::new("/") {
        return Ok(());
    }
    let configured = read_component_configuration(&cli.root, Component::Telephony).ok();
    let generated = PathBuf::from(
        configured
            .as_ref()
            .and_then(|values| values.get("BTS_ASTERISK_GENERATED_SOUNDS_DIR"))
            .map(String::as_str)
            .unwrap_or("/var/lib/asterisk/sounds/en/bts-generated"),
    );
    if let Some(parent) = generated.parent() {
        for ancestor in parent.ancestors() {
            if ancestor == Path::new("/") {
                break;
            }
            let Ok(output) = std::process::Command::new("stat")
                .args(["-c", "%G"])
                .arg(ancestor)
                .output()
            else {
                continue;
            };
            if !output.status.success() {
                continue;
            }
            let group = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if group.is_empty() || group == "root" || group == "bts" || group == "UNKNOWN" {
                continue;
            }
            if std::process::Command::new("getent")
                .args(["group", &group])
                .status()
                .is_ok_and(|status| status.success())
            {
                let current_groups = std::process::Command::new("id")
                    .args(["-nG", "bts"])
                    .output()?;
                let already_member = current_groups.status.success()
                    && String::from_utf8_lossy(&current_groups.stdout)
                        .split_whitespace()
                        .any(|value| value == group);
                if !already_member {
                    let status = std::process::Command::new("usermod")
                        .args(["-aG", &group, "bts"])
                        .status()?;
                    ensure!(status.success(), "Could not grant bts access through group {group}.");
                }
                break;
            }
        }
    }
    let status = std::process::Command::new("install")
        .args(["-d", "-o", "bts", "-g", "bts", "-m", "0755"])
        .arg(&generated)
        .status()?;
    ensure!(status.success(), "Could not reconcile the BTS generated Asterisk sound namespace.");
    Ok(())
}

fn migrate_legacy_voice_assets(cli: &Cli) -> Result<()> {''',
)

# #89: upgrade is journalled as one installer-owned transaction.
rep(
    "bts-install/src/main.rs",
    '''async fn upgrade(
    cli: &Cli,
    state: &mut InstallerState,
    selected: &BTreeSet<Component>,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
) -> Result<()> {
    let client = release_client(cli)?;''',
    '''async fn upgrade(
    cli: &Cli,
    state: &mut InstallerState,
    selected: &BTreeSet<Component>,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
) -> Result<()> {
    if cli.dry_run {
        return upgrade_inner(cli, state, selected, platform, architecture).await;
    }
    let transaction = transaction::HostTransaction::begin(&cli.root)?;
    let result = async {
        migrate_legacy_configuration(cli, &state.installed_components)?;
        upgrade_inner(cli, state, selected, platform, architecture).await
    }
    .await;
    match result {
        Ok(()) => {
            drop(transaction);
            Ok(())
        }
        Err(error) => {
            let rollback = transaction.rollback();
            anyhow::bail!(
                "Upgrade transaction failed: {error:#}; rollback {}.",
                if rollback.is_ok() { "succeeded" } else { "failed" }
            )
        }
    }
}

async fn upgrade_inner(
    cli: &Cli,
    state: &mut InstallerState,
    selected: &BTreeSet<Component>,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
) -> Result<()> {
    let client = release_client(cli)?;''',
)

# An upgrade must refresh a Telephony process after supplementary-group reconciliation.
rep(
    "bts-install/src/main.rs",
    '''    let mut changed = activations
        .iter()
        .filter(|activation| activation.changed)
        .map(|activation| activation.component)
        .collect::<BTreeSet<_>>();''',
    '''    let mut changed = activations
        .iter()
        .filter(|activation| activation.changed)
        .map(|activation| activation.component)
        .collect::<BTreeSet<_>>();
    if staged
        .iter()
        .any(|(component, _)| *component == Component::Telephony)
    {
        changed.insert(Component::Telephony);
    }''',
)

# #96: decoded chunked streamed WAV is valid even with RIFF length 0xffffffff.
rep(
    "bts-install/src/main.rs",
    "    #[tokio::test]\n    async fn telephony_probes_distinguish_unreachable_services() {",
    '''    #[tokio::test]
    async fn tts_probe_accepts_chunked_streamed_riff_wave() {
        let kokoro_url = serve_once(
            b"HTTP/1.1 200 OK\\r\\nContent-Type: audio/wav\\r\\nTransfer-Encoding: chunked\\r\\n\\r\\n10\\r\\nRIFF\\xff\\xff\\xff\\xffWAVEdata\\r\\n0\\r\\n\\r\\n",
        )
        .await;
        let values = telephony_values("http://127.0.0.1:1".into(), kokoro_url);
        assert_eq!(probe_tts(&values).await, TtsProbe::Rendered);
    }

    #[tokio::test]
    async fn telephony_probes_distinguish_unreachable_services() {''',
)
rep(
    "bts-install/src/main.rs",
    '"Kokoro TTS responded but returned an unusable TTS response.\\n  Endpoint: {tts_endpoint}"',
    '"Kokoro TTS returned HTTP 200, but the decoded response body was not a RIFF/WAVE stream.\\n  Endpoint: {tts_endpoint}"',
)
