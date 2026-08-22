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


# #92: an explicit action supersedes background presentations only on its target.
rep(
    "bts-addons/src/addons/mod.rs",
    "    pub(crate) async fn handle(&self, event: &Event) -> Vec<AddonFailure> {\n        let targets: Vec<_> = match &event.kind {",
    '''    pub(crate) async fn handle(&self, event: &Event) -> Vec<AddonFailure> {
        let mut failures = Vec::new();
        if let EventKind::ActionRequested { request } = &event.kind
            && let Some(target) = &request.target
        {
            let owner = self.registry.action_owner(&request.action).cloned();
            for (id, addon) in self.registry.entries() {
                if owner.as_ref() == Some(id) {
                    continue;
                }
                let context = self.context(id).with_selected_target(Some(target.clone()));
                if let Err(error) = addon.presentation_superseded(&context, target).await {
                    failures.push(failure(id, "presentation supersession", error));
                }
            }
        }
        let targets: Vec<_> = match &event.kind {''',
)
rep(
    "bts-addons/src/addons/mod.rs",
    "        let mut failures = Vec::new();\n        for id in targets {",
    "        for id in targets {",
)

# #95: enumerate known session speech for startup warming.
rep(
    "bts-telephony/src/session.rs",
    "fn target_choices(targets: &TelephonyTargets) -> Vec<TargetChoice> {",
    '''pub fn static_speech_prompts() -> Vec<String> {
    let mut prompts = Vec::new();
    for encoded in [
        CONFIGURATION_PROMPT,
        NO_TERMINALS_PROMPT,
        SELECT_TARGET_PROMPT,
        TARGET_UNAVAILABLE_PROMPT,
        INVALID_SELECTION_PROMPT,
    ] {
        for item in media_items(encoded) {
            if let MediaItem::Speech(text) = item
                && !prompts.contains(&text)
            {
                prompts.push(text);
            }
        }
    }
    for text in ["Returning to the previous service.", "Press hash to confirm."] {
        if !prompts.iter().any(|value| value == text) {
            prompts.push(text.to_owned());
        }
    }
    prompts
}

fn target_choices(targets: &TelephonyTargets) -> Vec<TargetChoice> {''',
)

# #95: static speech warms before ARI starts; live misses never await Kokoro.
rep(
    "bts-telephony/src/main.rs",
    "    session::{CallerIdentity, MediaItem, TelephonySession},",
    "    session::{CallerIdentity, MediaItem, TelephonySession, static_speech_prompts},",
)
rep(
    "bts-telephony/src/main.rs",
    "    let voice = Arc::new(runtime_voice_cache());\n",
    "    let voice = Arc::new(runtime_voice_cache());\n    warm_static_speech(&menu_media_uris, &voice)\n        .await\n        .context(\"required Telephony speech could not be warmed\")?;\n",
)
rep(
    "bts-telephony/src/main.rs",
    "    voice: &VoiceCache<KokoroSynthesizer>,\n) {",
    "    voice: &Arc<VoiceCache<KokoroSynthesizer>>,\n) {",
)
rep(
    "bts-telephony/src/main.rs",
    '''            MediaItem::Speech(text) => match voice.render(text).await {
                Ok(prompt) => prompt.media_uri,
                Err(error) => {
                    warn!(
                        %channel_id,
                        %error,
                        "failed to render voice prompt; playing the emergency error tone and abandoning the incomplete queue"
                    );
                    stop_after_item = true;
                    "sound:beeperr".to_owned()
                }
            },''',
    '''            MediaItem::Speech(text) => match voice.cached(text).await {
                Ok(Some(prompt)) => prompt.media_uri,
                Ok(None) => {
                    let voice = Arc::clone(voice);
                    let prompt_text = text.clone();
                    tokio::spawn(async move {
                        if let Err(error) = voice.render(&prompt_text).await {
                            warn!(%error, prompt = %prompt_text, "background voice-cache warm failed");
                        }
                    });
                    warn!(
                        %channel_id,
                        prompt = %text,
                        "uncached dynamic speech skipped on live call; warming in background"
                    );
                    stop_after_item = true;
                    "sound:beeperr".to_owned()
                }
                Err(error) => {
                    warn!(
                        %channel_id,
                        %error,
                        "failed to read cached voice prompt; playing the emergency error tone and abandoning the incomplete queue"
                    );
                    stop_after_item = true;
                    "sound:beeperr".to_owned()
                }
            },''',
)
rep(
    "bts-telephony/src/main.rs",
    "fn runtime_voice_cache() -> VoiceCache<KokoroSynthesizer> {",
    '''async fn warm_static_speech(
    menu: &[MediaItem],
    voice: &VoiceCache<KokoroSynthesizer>,
) -> anyhow::Result<()> {
    let mut prompts = static_speech_prompts();
    for item in menu {
        if let MediaItem::Speech(text) = item
            && !prompts.contains(text)
        {
            prompts.push(text.clone());
        }
    }
    for prompt in prompts {
        voice
            .render(&prompt)
            .await
            .with_context(|| format!("failed to warm static speech {prompt:?}"))?;
    }
    Ok(())
}

fn runtime_voice_cache() -> VoiceCache<KokoroSynthesizer> {''',
)

# Installer transaction import and crash recovery.
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

# Migrations for plan-based operations now run after the journal starts.
p = Path("bts-install/src/main.rs")
text = p.read_text()
text = text.replace("                migrate_legacy_configuration(&cli, &plan.after)?;\n", "")
text = text.replace("                migrate_legacy_configuration(&cli, &plan.before)?;\n", "")
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

# Telephony Asterisk traversal/write access is reconciled after configuration.
rep(
    "bts-install/src/main.rs",
    "            if let Some(unit) = component.unit() {\n                systemctl(&mut system, &cli.root, \"enable\", &[unit])?;\n            }",
    "            if component == Component::Telephony {\n                reconcile_telephony_runtime_access(cli)?;\n            }\n            if let Some(unit) = component.unit() {\n                systemctl(&mut system, &cli.root, \"enable\", &[unit])?;\n            }",
)
rep(
    "bts-install/src/main.rs",
    "        migrate_legacy_voice_assets(cli)?;\n    }\n    let changed = activations",
    "        migrate_legacy_voice_assets(cli)?;\n        reconcile_telephony_runtime_access(cli)?;\n    }\n    let changed = activations",
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
                let status = std::process::Command::new("usermod")
                    .args(["-aG", &group, "bts"])
                    .status()?;
                ensure!(status.success(), "Could not grant bts access through group {group}.");
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

# Upgrade activation and final state persistence participate in the durable journal.
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
    match upgrade_inner(cli, state, selected, platform, architecture).await {
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

# #94: validate generated sounds as the actual bts runtime identity.
rep(
    "bts-install/src/diagnostics.rs",
    "            }\n        }\n\n        if root == Path::new(\"/\") && component.unit().is_some() {",
    '''            }
            if root == Path::new("/") {
                let configured = fs::read_to_string(root.join("etc/bts/telephony.env"))
                    .ok()
                    .and_then(|text| crate::config::parse_environment(&text).ok());
                let generated = configured
                    .as_ref()
                    .and_then(|values| values.get("BTS_ASTERISK_GENERATED_SOUNDS_DIR"))
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/var/lib/asterisk/sounds/en/bts-generated"));
                let inaccessible = generated
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                    .take_while(|path| *path != Path::new("/"))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .find(|path| {
                        system.run_quiet(
                            "runuser",
                            &[
                                "-u".into(), "bts".into(), "--".into(), "test".into(),
                                "-x".into(), path.display().to_string(),
                            ],
                        ).is_err()
                    });
                if let Some(path) = inaccessible {
                    diagnostics.push(Diagnostic {
                        component: Some(Component::Telephony),
                        severity: Severity::Error,
                        message: format!("Telephony runtime account cannot traverse {} on the way to generated Asterisk sounds.", path.display()),
                        suggested_action: Some("Re-run: sudo bts-install add telephony".into()),
                    });
                } else if system.run_quiet(
                    "runuser",
                    &[
                        "-u".into(), "bts".into(), "--".into(), "test".into(),
                        "-w".into(), generated.display().to_string(),
                    ],
                ).is_err() {
                    diagnostics.push(Diagnostic {
                        component: Some(Component::Telephony),
                        severity: Severity::Error,
                        message: format!("Telephony runtime account cannot write generated Asterisk sounds at {}.", generated.display()),
                        suggested_action: Some("Re-run: sudo bts-install add telephony".into()),
                    });
                } else {
                    diagnostics.push(Diagnostic {
                        component: Some(Component::Telephony),
                        severity: Severity::Ok,
                        message: "Telephony Asterisk generated-sound namespace is accessible to the runtime account.".into(),
                        suggested_action: None,
                    });
                }
            }
        }

        if root == Path::new("/") && component.unit().is_some() {''',
)

# #102: Debian installs seatd in /usr/sbin.
rep(
    "bts-install/src/diagnostics.rs",
    '''                for executable in ["/usr/bin/cage", "/usr/bin/seatd"] {
                    if !system.exists(Path::new(executable)) {
                        diagnostics.push(Diagnostic {
                            component: Some(*component),
                            severity: Severity::Error,
                            message: format!("Display runtime dependency {executable} is missing."),
                            suggested_action: Some("Re-run: sudo bts-install add display".into()),
                        });
                    }
                }''',
    '''                for (name, candidates) in [
                    ("cage", &["/usr/bin/cage", "/usr/local/bin/cage"][..]),
                    ("seatd", &["/usr/bin/seatd", "/usr/sbin/seatd"][..]),
                ] {
                    if !candidates.iter().any(|path| system.exists(Path::new(path))) {
                        diagnostics.push(Diagnostic {
                            component: Some(*component),
                            severity: Severity::Error,
                            message: format!("Display runtime dependency {name} is missing (checked {}).", candidates.join(", ")),
                            suggested_action: Some("Re-run: sudo bts-install add display".into()),
                        });
                    }
                }''',
)

# #96: diagnostic detail and a chunked streamed RIFF/WAVE regression.
rep(
    "bts-install/src/main.rs",
    '"Kokoro TTS responded but returned an unusable TTS response.\\n  Endpoint: {tts_endpoint}"',
    '"Kokoro TTS returned HTTP 200, but the decoded response body was not a RIFF/WAVE stream.\\n  Endpoint: {tts_endpoint}"',
)
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
