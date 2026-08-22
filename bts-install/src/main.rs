use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Cursor, IsTerminal, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use bts_install::{
    INSTALLER_VERSION, activation,
    archive::extract_tar_zst,
    cli::{Cli, Command, HELP},
    config, diagnostics,
    manifest::ComponentAsset,
    model::{Component, Role},
    output::{Palette, human_plan},
    plan::{Action, InstallationPlan},
    platform::{Platform, detect_host},
    release::ReleaseClient,
    services,
    state::InstallerState,
    system::{RealSystem, SystemAdapter, create_service_account, systemctl},
};

const LICENCE: &str = include_str!("../../LICENSE");
const DEFAULT_ARI_URL: &str = "http://127.0.0.1:8088";
const DEFAULT_ARI_USERNAME: &str = "bts";
const DEFAULT_KOKORO_URL: &str = "http://127.0.0.1:8880/v1/audio/speech";
const TELEPHONY_SETUP_GUIDE: &str = "docs/telephony-setup.md";

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse(std::env::args())?;
    match cli.command {
        Command::Help => {
            print!("{HELP}");
            return Ok(());
        }
        Command::Version => {
            println!("bts-install {INSTALLER_VERSION}");
            return Ok(());
        }
        Command::Licence => {
            print_licence(&cli)?;
            return Ok(());
        }
        Command::Warranty => {
            println!("{}", bts_install::warranty_notice());
            return Ok(());
        }
        _ => {}
    }

    let state_path = rooted(&cli.root, "/var/lib/bts-install/state.json");
    let mut state = InstallerState::load(&state_path)?;

    match &cli.command {
        Command::Status => {
            let report = diagnostics::status(&cli.root, state.as_ref(), &mut RealSystem);
            print_status(&report, cli.json, cli.quiet)?;
            return Ok(());
        }
        Command::Doctor => {
            let mut report = diagnostics::doctor(&cli.root, state.as_ref(), &mut RealSystem);
            extend_remote_diagnostics(&cli, state.as_ref(), &mut report).await;
            print_doctor(&report, cli.json, cli.quiet)?;
            if !report.healthy {
                std::process::exit(2);
            }
            return Ok(());
        }
        _ => {}
    }

    require_root_or_alternate(&cli.root)?;
    if interactive(&cli) {
        println!("{}\n", bts_install::legal_notice());
    }
    let (platform, architecture) = detect_host(&cli.root)?;

    match &cli.command {
        Command::Install { role, components } => {
            let plan = InstallationPlan::install(
                state.as_ref(),
                *role,
                components,
                platform,
                cli.no_start,
            )?;
            confirm_plan(&cli, &plan)?;
            if !cli.dry_run {
                migrate_legacy_configuration(&cli, &plan.after)?;
                let mut next = state.unwrap_or_else(|| {
                    InstallerState::new(INSTALLER_VERSION, platform, architecture)
                });
                execute_plan(&cli, &plan, &mut next, platform, architecture, true).await?;
                next.selected_role = plan.role;
                next.installed_components = plan.after.clone();
                next.write_atomic(&state_path)?;
                state = Some(next);
            }
        }
        Command::Add(components) => {
            let current = state
                .as_ref()
                .context("No managed BTS installation exists; run install first.")?;
            let plan = InstallationPlan::add(current, components, platform, cli.no_start)?;
            confirm_plan(&cli, &plan)?;
            if !cli.dry_run {
                migrate_legacy_configuration(&cli, &plan.after)?;
                let mut next = current.clone();
                execute_plan(&cli, &plan, &mut next, platform, architecture, false).await?;
                next.selected_role = Some(Role::Custom);
                next.installed_components = plan.after.clone();
                next.write_atomic(&state_path)?;
                state = Some(next);
            }
        }
        Command::Remove(components) => {
            let current = state
                .as_ref()
                .context("No managed BTS installation exists.")?;
            let plan = InstallationPlan::remove(current, components, platform, cli.purge)?;
            confirm_plan(&cli, &plan)?;
            if !cli.dry_run {
                migrate_legacy_configuration(&cli, &plan.before)?;
                let mut next = current.clone();
                execute_plan(&cli, &plan, &mut next, platform, architecture, false).await?;
                next.selected_role = Some(Role::Custom);
                next.installed_components = plan.after.clone();
                next.write_atomic(&state_path)?;
                state = Some(next);
            }
        }
        Command::Upgrade(components) => {
            let current = state
                .as_mut()
                .context("No managed BTS installation exists.")?;
            let selected = select_upgrade_components(current, components)?;
            if !cli.dry_run {
                migrate_legacy_configuration(&cli, &current.installed_components)?;
            } else {
                config::plan_legacy_environment_migration(
                    &cli.root,
                    &current.installed_components,
                )?;
            }
            require_display_migration_before_upgrade(&cli, &selected)?;
            upgrade(&cli, current, &selected, platform, architecture).await?;
            if !cli.dry_run {
                current.write_atomic(&state_path)?;
            }
        }
        Command::Configure(component) => {
            let current = state
                .as_ref()
                .context("No managed BTS installation exists.")?;
            let selected = choose_configuration_component(*component, current, &cli)?;
            if !cli.dry_run {
                migrate_legacy_configuration(&cli, &current.installed_components)?;
            } else {
                config::plan_legacy_environment_migration(
                    &cli.root,
                    &current.installed_components,
                )?;
            }
            configure_component(&cli, selected).await?;
        }
        Command::Uninstall(components) => {
            let current = state
                .as_ref()
                .context("No managed BTS installation exists.")?;
            let selected: Vec<_> = if components.is_empty() {
                current.installed_components.iter().copied().collect()
            } else {
                components.clone()
            };
            let plan = InstallationPlan::remove(current, &selected, platform, cli.purge)?;
            confirm_plan(&cli, &plan)?;
            if !cli.dry_run {
                migrate_legacy_configuration(&cli, &plan.before)?;
                let mut next = current.clone();
                execute_plan(&cli, &plan, &mut next, platform, architecture, false).await?;
                next.installed_components = plan.after.clone();
                persist_uninstall_state(&state_path, &mut state, next)?;
            }
        }
        _ => unreachable!(),
    }

    if !cli.quiet && !cli.json && !cli.dry_run && let Some(state) = state {
        let mut report = diagnostics::doctor(&cli.root, Some(&state), &mut RealSystem);
        extend_remote_diagnostics(&cli, Some(&state), &mut report).await;
        let palette = terminal_palette(false, false);
        if state.installed_components.contains(&Component::Telephony) {
            println!("Checking Telephony configuration...");
            for diagnostic in report
                .diagnostics
                .iter()
                .filter(|item| telephony_service_check(item))
            {
                let marker = match diagnostic.severity {
                    diagnostics::Severity::Ok => palette.success("✓"),
                    diagnostics::Severity::Warning => palette.warning("!"),
                    diagnostics::Severity::Error => palette.error("✗"),
                };
                println!("{marker} {}", diagnostic.message);
                if let Some(action) = &diagnostic.suggested_action {
                    println!("  {}", palette.dim(action));
                }
            }
        }
        if report.healthy {
            println!(
                "{}",
                palette.success(&format!(
                    "✓ BTS {} is reconciled and ready ({}).",
                    state.installed_version,
                    join_components(&state.installed_components)
                ))
            );
        } else {
            if state.installed_components.contains(&Component::Telephony) {
                println!("BTS Telephony is installed but not ready yet.");
            } else {
                println!("Installation files are complete, but BTS is not yet ready.");
            }
            for diagnostic in report
                .diagnostics
                .iter()
                .filter(|item| {
                    item.severity == diagnostics::Severity::Error
                        && !telephony_service_check(item)
                })
            {
                println!("{} {}", palette.error("✗"), diagnostic.message);
                if let Some(action) = &diagnostic.suggested_action {
                    println!("  {}", palette.dim(action));
                }
            }
            if state.installed_components.contains(&Component::Telephony) {
                println!("See {TELEPHONY_SETUP_GUIDE} for setup instructions.");
            }
            println!("Run: sudo bts-install doctor");
        }
    }
    Ok(())
}

fn telephony_service_check(diagnostic: &diagnostics::Diagnostic) -> bool {
    diagnostic.component == Some(Component::Telephony)
        && [
            "BTS Core ",
            "Asterisk ARI ",
            "ARI credentials ",
            "Kokoro TTS ",
            "Test speech ",
        ]
        .iter()
        .any(|prefix| diagnostic.message.starts_with(prefix))
}

async fn execute_plan(
    cli: &Cli,
    plan: &InstallationPlan,
    state: &mut InstallerState,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
    refresh_existing: bool,
) -> Result<()> {
    let added: Vec<_> = plan.after.difference(&plan.before).copied().collect();
    let deployed: Vec<_> = if refresh_existing {
        plan.after.iter().copied().collect()
    } else {
        added.clone()
    };
    let mut system = RealSystem;
    let packages: Vec<_> = plan
        .actions
        .iter()
        .filter_map(|action| {
            if let Action::InstallPackage { package } = action {
                Some(package.clone())
            } else {
                None
            }
        })
        .collect();
    let packages = packages
        .into_iter()
        .filter(|package| package != "ffmpeg" || !Path::new("/usr/bin/ffmpeg").is_file())
        .collect::<Vec<_>>();
    if !packages.is_empty() && cli.root == Path::new("/") {
        let command = platform.package_command(&packages, cli.yes);
        system.run(&command[0], &command[1..])?;
    }
    for action in &plan.actions {
        match action {
            Action::CreateAccount { account } => {
                create_service_account(&mut system, &cli.root, account)?
            }
            Action::StopService { unit } if cli.root == Path::new("/") => {
                systemctl(&mut system, &cli.root, "stop", &[unit])?
            }
            Action::DisableService { unit } => {
                systemctl(&mut system, &cli.root, "disable", &[unit])?
            }
            Action::RemoveComponent { component, purge } => {
                remove_component(cli, *component, *purge)?
            }
            Action::RestoreTty1 => restore_tty1(cli, &mut system)?,
            _ => {}
        }
    }
    if plan
        .actions
        .iter()
        .any(|action| matches!(action, Action::RestoreTty1))
    {
        state.tty1_managed = false;
    }
    if cli.root == Path::new("/")
        && plan
            .actions
            .iter()
            .any(|action| matches!(action, Action::RemoveComponent { .. }))
    {
        systemctl(&mut system, &cli.root, "daemon-reload", &[])?;
    }
    if !deployed.is_empty() {
        let client = release_client(cli)?;
        let (manifest, urls) = client.fetch_manifest().await?;
        let mut changed = BTreeSet::new();
        for component in deployed {
            let asset = manifest.select(component, platform, architecture)?;
            let activation = install_component(
                cli,
                &mut system,
                &manifest.release_version,
                component,
                asset,
                &urls,
            )
            .await?;
            if activation.changed {
                changed.insert(component);
            }
            if component == Component::Display && added.contains(&component) {
                prepare_display_host(cli, &mut system, state)?;
            }
            if component == Component::Telephony {
                migrate_legacy_voice_assets(cli)?;
            }
            if component.config_name().is_some() && added.contains(&component) {
                ensure_default_configuration(
                    cli,
                    component,
                    plan.after.contains(&Component::Core),
                )?;
                if component == Component::Telephony && !cli.quiet && !cli.json {
                    println!("Telephony configuration saved.");
                }
            }
            if let Some(unit) = component.unit() {
                systemctl(&mut system, &cli.root, "enable", &[unit])?;
            }
        }
        let mut desired_services = plan.after.clone();
        if desired_services.contains(&Component::Telephony)
            && !component_configuration_is_valid(&cli.root, Component::Telephony)
        {
            desired_services.remove(&Component::Telephony);
            if changed.contains(&Component::Telephony)
                && !cli.no_start
                && cli.root == Path::new("/")
            {
                systemctl(
                    &mut system,
                    &cli.root,
                    "stop",
                    &[Component::Telephony.unit().expect("Telephony has a unit")],
                )?;
            }
        }
        if desired_services.contains(&Component::Telephony) {
            let values = read_component_configuration(&cli.root, Component::Telephony)?;
            if !telephony_startup_ready(&values, plan.after.contains(&Component::Core)).await {
                desired_services.remove(&Component::Telephony);
            }
        }
        services::reconcile(
            &mut system,
            &cli.root,
            &desired_services,
            &changed,
            cli.no_start,
        )?;
        record_release_source(cli, state, &client, &manifest)?;
        state.installed_version = manifest.release_version;
        for component in &plan.after {
            state
                .component_versions
                .insert(*component, state.installed_version.clone());
        }
        state.installer_version = INSTALLER_VERSION.into();
        state.platform = platform;
        state.architecture = architecture;
    }
    state
        .component_versions
        .retain(|component, _| plan.after.contains(component));
    if !plan.actions.is_empty() {
        state.updated_at = Some(timestamp());
    }
    Ok(())
}

fn release_client(cli: &Cli) -> Result<ReleaseClient> {
    ReleaseClient::new(
        cli.repository.clone(),
        cli.channel.clone(),
        cli.release_dir.clone(),
    )
}

fn record_release_source(
    cli: &Cli,
    state: &mut InstallerState,
    client: &ReleaseClient,
    manifest: &bts_install::manifest::ReleaseManifest,
) -> Result<()> {
    state.repository = cli.repository.clone();
    state.release_channel = if cli.release_dir.is_some() {
        bts_install::LOCAL_RELEASE_CHANNEL.into()
    } else {
        client.recorded_channel(manifest)?
    };
    state.release_pinned = cli.release_dir.is_none() && client.is_exact_version();
    Ok(())
}

async fn install_component(
    cli: &Cli,
    system: &mut RealSystem,
    version: &str,
    component: Component,
    asset: &ComponentAsset,
    urls: &BTreeMap<String, String>,
) -> Result<activation::Activation> {
    let base = rooted(
        &cli.root,
        &format!("/usr/lib/bts/components/{component}/releases"),
    );
    fs::create_dir_all(&base)?;
    let activation_id = activation_id(version, &asset.sha256);
    let destination = base.join(&activation_id);
    if destination.exists() {
        ensure!(
            destination.is_dir(),
            "Preserved release path is not a directory."
        );
        validate_bundle_metadata(&destination, component, version)?;
        install_bundle_integration(cli, component, &destination)?;
        if cli.root == Path::new("/") {
            systemctl(system, &cli.root, "daemon-reload", &[])?;
        }
        return activation::activate(&cli.root, component, &activation_id);
    }
    if !cli.quiet && !cli.json {
        println!("Downloading and verifying {component}...");
    }
    let client = release_client(cli)?;
    let bytes = client
        .download_asset(urls, &asset.filename, &asset.sha256)
        .await?;
    let temporary = tempfile::Builder::new()
        .prefix(".stage-")
        .tempdir_in(&base)?;
    extract_tar_zst(Cursor::new(bytes), temporary.path())?;
    let bundle = temporary.path().join(component.bundle_root());
    ensure!(
        bundle.join("bin").join(component.binary()).is_file(),
        "Bundle '{}' does not contain the expected portable layout.",
        asset.filename
    );
    validate_bundle_metadata(&bundle, component, version)?;
    fs::rename(&bundle, &destination)?;
    install_bundle_integration(cli, component, &destination)?;
    if cli.root == Path::new("/") {
        systemctl(system, &cli.root, "daemon-reload", &[])?;
    }
    activation::activate(&cli.root, component, &activation_id)
}

async fn upgrade(
    cli: &Cli,
    state: &mut InstallerState,
    selected: &BTreeSet<Component>,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
) -> Result<()> {
    let client = release_client(cli)?;
    let (manifest, urls) = client.fetch_manifest().await?;
    let mut pending = BTreeMap::new();
    for component in selected {
        let asset = manifest.select(*component, platform, architecture)?;
        let activation_id = activation_id(&manifest.release_version, &asset.sha256);
        let current = rooted(
            &cli.root,
            &format!("/usr/lib/bts/components/{component}/current"),
        );
        if fs::read_link(current).ok().as_deref()
            != Some(Path::new("releases").join(&activation_id).as_path())
        {
            pending.insert(*component, activation_id);
        }
    }
    let mut actions: Vec<_> = pending
        .keys()
        .flat_map(|component| {
            let mut actions = vec![
                Action::Download {
                    component: *component,
                },
                Action::Stage {
                    component: *component,
                },
            ];
            actions.push(Action::Activate {
                component: *component,
            });
            if !cli.no_start && let Some(unit) = component.unit() {
                actions.push(Action::StartService { unit: unit.into() });
            }
            actions
        })
        .collect();
    if selected.contains(&Component::Telephony) && !Path::new("/usr/bin/ffmpeg").is_file() {
        actions.insert(
            0,
            Action::InstallPackage {
                package: "ffmpeg".into(),
            },
        );
    }
    let plan = InstallationPlan {
        role: state.selected_role,
        before: state.installed_components.clone(),
        after: state.installed_components.clone(),
        actions,
    };
    let release_source_changed = print_release_source_change(cli, state, &client, &manifest)?;
    confirm_plan(cli, &plan)?;
    if release_source_changed && plan.actions.is_empty() && !cli.dry_run && !cli.yes {
        ensure!(
            interactive(cli),
            "Changing the release track requires --yes in non-interactive mode."
        );
        ensure!(confirm("Change the release track")?, "Operation cancelled.");
    }
    if cli.dry_run {
        return Ok(());
    }
    if selected.contains(&Component::Telephony)
        && cli.root == Path::new("/")
        && !Path::new("/usr/bin/ffmpeg").is_file()
    {
        let packages = platform
            .packages_for("ffmpeg")?
            .iter()
            .map(|package| (*package).to_owned())
            .collect::<Vec<_>>();
        let command = platform.package_command(&packages, cli.yes);
        RealSystem.run(&command[0], &command[1..])?;
    }
    let mut staged = Vec::new();
    for (component, activation_id) in &pending {
        let asset = manifest.select(*component, platform, architecture)?;
        let bytes = client
            .download_asset(&urls, &asset.filename, &asset.sha256)
            .await?;
        let base = rooted(
            &cli.root,
            &format!("/usr/lib/bts/components/{component}/releases"),
        );
        fs::create_dir_all(&base)?;
        let temporary = tempfile::Builder::new()
            .prefix(".stage-")
            .tempdir_in(&base)?;
        extract_tar_zst(Cursor::new(bytes), temporary.path())?;
        let bundle = temporary.path().join(component.bundle_root());
        validate_bundle_metadata(&bundle, *component, &manifest.release_version)?;
        let destination = base.join(activation_id);
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        fs::rename(bundle, &destination)?;
        install_bundle_integration(cli, *component, &destination)?;
        staged.push((*component, activation_id.clone()));
    }

    let mut system = RealSystem;
    if cli.root == Path::new("/") && !staged.is_empty() {
        systemctl(&mut system, &cli.root, "daemon-reload", &[])?;
    }
    let mut activations = Vec::new();
    for (component, activation_id) in &staged {
        match activation::activate(&cli.root, *component, activation_id) {
            Ok(value) => activations.push(value),
            Err(error) => {
                let rollback = activations.iter().rev().try_for_each(activation::rollback);
                restart_restored_services(cli, &mut system, &activations);
                bail!(
                    "{} could not be activated: {error}; rollback {}.",
                    component,
                    if rollback.is_ok() {
                        "succeeded"
                    } else {
                        "failed"
                    }
                );
            }
        }
    }
    if staged
        .iter()
        .any(|(component, _)| *component == Component::Telephony)
    {
        migrate_legacy_voice_assets(cli)?;
    }
    let changed = activations
        .iter()
        .filter(|activation| activation.changed)
        .map(|activation| activation.component)
        .collect::<BTreeSet<_>>();
    let mut desired_services = state.installed_components.clone();
    if desired_services.contains(&Component::Telephony)
        && !component_configuration_is_valid(&cli.root, Component::Telephony)
    {
        desired_services.remove(&Component::Telephony);
        if changed.contains(&Component::Telephony)
            && !cli.no_start
            && cli.root == Path::new("/")
        {
            systemctl(
                &mut system,
                &cli.root,
                "stop",
                &[Component::Telephony.unit().expect("Telephony has a unit")],
            )?;
        }
    }
    if let Err(error) = services::reconcile(
        &mut system,
        &cli.root,
        &desired_services,
        &changed,
        cli.no_start,
    ) {
        let rollback = activations.iter().rev().try_for_each(activation::rollback);
        restart_restored_services(cli, &mut system, &activations);
        bail!(
            "Services failed activation health checks: {error}; rollback {}.",
            if rollback.is_ok() {
                "succeeded"
            } else {
                "failed"
            }
        );
    }
    record_release_source(cli, state, &client, &manifest)?;
    state.installed_version = manifest.release_version;
    for component in selected {
        state
            .component_versions
            .insert(*component, state.installed_version.clone());
    }
    state.installer_version = INSTALLER_VERSION.into();
    if !pending.is_empty() {
        state.updated_at = Some(timestamp());
    }
    Ok(())
}

fn print_release_source_change(
    cli: &Cli,
    state: &InstallerState,
    client: &ReleaseClient,
    manifest: &bts_install::manifest::ReleaseManifest,
) -> Result<bool> {
    if cli.release_dir.is_some() {
        return Ok(false);
    }
    let resolved = client.recorded_channel(manifest)?;
    let changed = resolved != state.release_channel;
    if changed && !cli.quiet && !cli.json {
        println!(
            "Release track: {} -> {} (resolved release {}).",
            state.release_channel, resolved, manifest.release_version
        );
    }
    Ok(changed)
}

fn restart_restored_services(
    cli: &Cli,
    system: &mut RealSystem,
    activations: &[activation::Activation],
) {
    if cli.no_start || cli.root != Path::new("/") {
        return;
    }
    for restored in activations {
        if let Some(unit) = restored.component.unit() {
            let _ = systemctl(system, &cli.root, "restart", &[unit]);
        }
    }
}

fn validate_bundle_metadata(bundle: &Path, component: Component, version: &str) -> Result<()> {
    let metadata = fs::read_to_string(bundle.join("install/component.conf"))
        .context("Bundle component metadata is missing")?;
    let values = config::parse_environment(&metadata)?;
    ensure!(
        values
            .get("BTS_COMPONENT")
            .is_some_and(|value| value == &component.to_string()),
        "Bundle metadata identifies the wrong component."
    );
    ensure!(
        values
            .get("BTS_BUNDLE_FORMAT")
            .is_some_and(|value| value == "1"),
        "Bundle metadata has an unsupported format."
    );
    let bundled_version = fs::read_to_string(bundle.join("VERSION"))?;
    ensure!(
        bundled_version.trim().trim_start_matches('v') == version.trim_start_matches('v'),
        "Bundle version differs from the release manifest."
    );
    ensure!(
        bundle.join("LICENSE").is_file(),
        "Bundle does not contain the complete licence text."
    );
    Ok(())
}

fn install_bundle_integration(cli: &Cli, component: Component, release: &Path) -> Result<()> {
    let units = rooted(&cli.root, "/usr/lib/systemd/system");
    fs::create_dir_all(&units)?;
    for entry in fs::read_dir(release.join("systemd")).into_iter().flatten() {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|value| value == "service" || value == "target")
        {
            fs::copy(entry.path(), units.join(entry.file_name()))?;
        }
    }
    let licence = rooted(&cli.root, "/usr/share/licenses/bts/LICENSE");
    fs::create_dir_all(licence.parent().unwrap())?;
    fs::copy(release.join("LICENSE"), licence)?;
    let binary = release.join("bin").join(component.binary());
    fs::set_permissions(binary, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn ensure_default_configuration(
    cli: &Cli,
    component: Component,
    local_core_selected: bool,
) -> Result<()> {
    let config_name = component
        .config_name()
        .context("This component has no service configuration")?;
    let path = rooted(&cli.root, &format!("/etc/bts/{config_name}"));
    let existing = if path.exists() {
        config::parse_environment(&fs::read_to_string(&path)?)?
    } else {
        BTreeMap::new()
    };
    let values = match component {
        Component::Display => resolve_display_configuration(
            cli,
            existing,
            local_core_selected.then_some(bts_compat::LOCAL_CORE_TERMINAL_WEBSOCKET_URL),
        )?,
        Component::Telephony => {
            let values = resolve_telephony_configuration(
                cli,
                existing,
                local_core_selected,
                false,
            )?;
            if values.contains_key("BTS_ARI_PASSWORD") {
                config::validate_telephony(&values)?;
                config::validate_http_url(
                    values
                        .get("BTS_CORE_URL")
                        .expect("Telephony Core URL was populated"),
                    "BTS_CORE_URL",
                )?;
            }
            values
        }
        Component::Core => {
            let mut values = existing;
            values
                .entry("BTS_CORE_BIND".into())
                .or_insert_with(|| "0.0.0.0:3100".into());
            values
        }
        Component::Addons => {
            let mut values = existing;
            if !values.contains_key("BTS_CORE_HTTP_URL") {
                values.insert(
                    "BTS_CORE_HTTP_URL".into(),
                    resolve_core_http(cli, local_core_selected)?,
                );
            }
            if !values.contains_key("BTS_CORE_WS_URL") {
                values.insert(
                    "BTS_CORE_WS_URL".into(),
                    resolve_core_websocket(cli, local_core_selected)?,
                );
            }
            values
                .entry("BTS_ADDON_DATA_ROOT".into())
                .or_insert_with(|| "/var/lib/bts/addons".into());
            values
        }
        Component::Cli => unreachable!("CLI has no service configuration"),
    };
    config::write_secure(&path, &values)?;
    secure_config_ownership(cli, &path, component)
}

fn resolve_telephony_configuration(
    cli: &Cli,
    mut values: BTreeMap<String, String>,
    local_core_selected: bool,
    force_input: bool,
) -> Result<BTreeMap<String, String>> {
    values
        .entry("BTS_ARI_URL".into())
        .or_insert_with(|| DEFAULT_ARI_URL.into());
    values
        .entry("BTS_ARI_USERNAME".into())
        .or_insert_with(|| DEFAULT_ARI_USERNAME.into());
    values
        .entry("BTS_KOKORO_URL".into())
        .or_insert_with(|| DEFAULT_KOKORO_URL.into());
    for (key, default) in [
        ("BTS_KOKORO_VOICE", "bf_emma"),
        ("BTS_KOKORO_MODEL", "kokoro"),
        ("BTS_KOKORO_MODEL_VERSION", "0.6.0"),
        ("BTS_KOKORO_SPEED", "1.05"),
        ("BTS_VOICE_LANGUAGE", "en"),
    ] {
        values.entry(key.into()).or_insert_with(|| default.into());
    }
    if !values.contains_key("BTS_CORE_URL") {
        if let Some(url) = &cli.core_http_url {
            values.insert("BTS_CORE_URL".into(), url.clone());
        } else if local_core_selected {
            values.insert("BTS_CORE_URL".into(), bts_compat::LOCAL_CORE_HTTP_URL.into());
        }
    }

    let complete = config::validate_telephony(&values).is_ok()
        && values.get("BTS_CORE_URL").is_some_and(|url| {
            config::validate_http_url(url, "BTS_CORE_URL").is_ok()
        });
    if complete && !force_input {
        return Ok(values);
    }

    if let Some(input) = &cli.secret_input {
        values.extend(config::read_secret_input(input)?);
    } else if interactive(cli) {
        println!("\nConfigure BTS Telephony");
        println!(
            "The default addresses assume Asterisk, Kokoro and BTS Telephony run on this computer."
        );
        println!(
            "If a service runs on another computer, enter that computer's address instead.\n"
        );
        let ari_url = prompt("Asterisk ARI URL", &values["BTS_ARI_URL"])?;
        let username = prompt("ARI username", &values["BTS_ARI_USERNAME"])?;
        let password = if values.contains_key("BTS_ARI_PASSWORD") {
            rpassword::prompt_password("ARI password (leave blank to keep the current password): ")?
        } else {
            rpassword::prompt_password("ARI password: ")?
        };
        ensure!(
            !password.is_empty() || values.contains_key("BTS_ARI_PASSWORD"),
            "ARI password is required."
        );
        if !password.is_empty() {
            let confirmation = rpassword::prompt_password("Confirm ARI password: ")?;
            ensure!(password == confirmation, "ARI passwords did not match.");
            values.insert("BTS_ARI_PASSWORD".into(), password);
        }
        println!("\nText-to-speech server");
        println!("BTS Telephony uses a Kokoro-compatible text-to-speech service.");
        println!("Use the default for Kokoro on this computer, or enter its remote API URL.");
        let kokoro_url = prompt("Kokoro TTS URL", &values["BTS_KOKORO_URL"])?;
        let core_url = match &cli.core_http_url {
            Some(value) => value.clone(),
            None => prompt(
                "BTS Core URL",
                values
                    .get("BTS_CORE_URL")
                    .map(String::as_str)
                    .unwrap_or(bts_compat::LOCAL_CORE_HTTP_URL),
            )?,
        };
        values.insert("BTS_ARI_URL".into(), ari_url);
        values.insert("BTS_ARI_USERNAME".into(), username);
        values.insert("BTS_KOKORO_URL".into(), kokoro_url);
        values.insert("BTS_CORE_URL".into(), core_url);
    } else {
        bail!(
            "Non-interactive Telephony configuration requires --secret-file or --secret-fd."
        );
    }

    config::validate_telephony(&values)?;
    config::validate_http_url(
        values
            .get("BTS_CORE_URL")
            .context("BTS_CORE_URL is not configured")?,
        "BTS_CORE_URL",
    )?;
    Ok(values)
}

fn migrate_legacy_configuration(cli: &Cli, installed: &BTreeSet<Component>) -> Result<()> {
    let Some(migration) = config::plan_legacy_environment_migration(&cli.root, installed)? else {
        return Ok(());
    };
    for (component, values) in migration {
        let config_name = component
            .config_name()
            .context("Migrated component must have configuration")?;
        let path = rooted(&cli.root, &format!("/etc/bts/{config_name}"));
        config::write_secure(&path, &values)?;
        secure_config_ownership(cli, &path, component)?;
    }
    fs::remove_file(rooted(&cli.root, "/etc/bts/bts.env"))?;
    Ok(())
}

fn resolve_display_configuration(
    cli: &Cli,
    mut existing: BTreeMap<String, String>,
    local_core_default: Option<&str>,
) -> Result<BTreeMap<String, String>> {
    let core_default = existing
        .get("BTS_CORE_WS_URL")
        .map(String::as_str)
        .or(local_core_default)
        .unwrap_or("");
    let core_url = match &cli.core_ws_url {
        Some(value) => value.clone(),
        None if interactive(cli) => prompt("Core terminal WebSocket URL", core_default)?,
        None => existing
            .get("BTS_CORE_WS_URL")
            .cloned()
            .or_else(|| local_core_default.map(str::to_owned))
            .context("Display installation requires --core-url in non-interactive mode.")?,
    };
    let terminal_id = match &cli.terminal_id {
        Some(value) => value.clone(),
        None if interactive(cli) => prompt(
            "Stable terminal ID",
            existing
                .get("BTS_TERMINAL_ID")
                .map(String::as_str)
                .unwrap_or(""),
        )?,
        None => existing
            .get("BTS_TERMINAL_ID")
            .cloned()
            .context("Display installation requires --terminal-id in non-interactive mode.")?,
    };
    let terminal_name = match &cli.terminal_name {
        Some(value) => value.clone(),
        None if interactive(cli) => prompt(
            "Suggested terminal name",
            existing
                .get("BTS_TERMINAL_NAME")
                .map(String::as_str)
                .unwrap_or(""),
        )?,
        None => existing
            .get("BTS_TERMINAL_NAME")
            .cloned()
            .context("Display installation requires --terminal-name in non-interactive mode.")?,
    };
    let cage_args = match &cli.cage_args {
        Some(value) => value.clone(),
        None if interactive(cli) => prompt(
            "Cage arguments",
            existing
                .get("BTS_CAGE_ARGS")
                .map(String::as_str)
                .unwrap_or("-m last"),
        )?,
        None => existing
            .get("BTS_CAGE_ARGS")
            .cloned()
            .unwrap_or_else(|| "-m last".into()),
    };

    existing.insert("BTS_CORE_WS_URL".into(), core_url);
    existing.insert("BTS_TERMINAL_ID".into(), terminal_id);
    existing.insert("BTS_TERMINAL_NAME".into(), terminal_name);
    existing.insert("BTS_CAGE_ARGS".into(), cage_args);
    existing
        .entry("BTS_DISPLAY_TTY".into())
        .or_insert_with(|| "1".into());
    config::validate_display(&existing)?;
    config::validate_cage_args(existing.get("BTS_CAGE_ARGS").unwrap())?;
    Ok(existing)
}

fn require_display_migration_before_upgrade(
    cli: &Cli,
    selected: &BTreeSet<Component>,
) -> Result<()> {
    if !selected.contains(&Component::Display) {
        return Ok(());
    }
    let path = rooted(&cli.root, "/etc/bts/display.env");
    let values = fs::read_to_string(&path)
        .with_context(|| format!("Could not read {}", path.display()))
        .and_then(|contents| config::parse_environment(&contents));
    values.and_then(|values| config::validate_display(&values)).with_context(|| {
        "Display configuration must be migrated before upgrade. Run 'sudo bts-install configure display' with the terminal endpoint, stable terminal ID and suggested name"
    })
}

fn resolve_core_http(cli: &Cli, local_core_selected: bool) -> Result<String> {
    let value = match &cli.core_http_url {
        Some(value) => value.clone(),
        None if local_core_selected => bts_compat::LOCAL_CORE_HTTP_URL.into(),
        None if interactive(cli) => prompt("Remote Core HTTP URL", "")?,
        None => bail!("This component requires --core-http-url in non-interactive mode."),
    };
    config::validate_http_url(&value, "Core HTTP URL")?;
    Ok(value)
}

fn resolve_core_websocket(cli: &Cli, local_core_selected: bool) -> Result<String> {
    let value = match &cli.core_ws_url {
        Some(value) => value.clone(),
        None if local_core_selected => bts_compat::LOCAL_CORE_WEBSOCKET_URL.into(),
        None if interactive(cli) => prompt("Remote Core WebSocket URL", "")?,
        None => bail!("This component requires --core-ws-url in non-interactive mode."),
    };
    config::validate_websocket_url(&value)?;
    Ok(value)
}

async fn configure_component(cli: &Cli, component: Component) -> Result<()> {
    let config_name = component
        .config_name()
        .context("The CLI component has no service configuration")?;
    let path = rooted(&cli.root, &format!("/etc/bts/{config_name}"));
    let existing = fs::read_to_string(&path)
        .ok()
        .and_then(|value| config::parse_environment(&value).ok())
        .unwrap_or_default();
    let previous = existing.clone();
    let values = match component {
        Component::Display => resolve_display_configuration(cli, existing, None)?,
        Component::Telephony => {
            resolve_telephony_configuration(cli, existing, false, true)?
        }
        Component::Core => {
            ensure!(
                interactive(cli),
                "Non-interactive Core configuration is not available without an existing configuration file."
            );
            let bind = prompt(
                "Core bind address",
                existing
                    .get("BTS_CORE_BIND")
                    .map(String::as_str)
                    .unwrap_or("0.0.0.0:3100"),
            )?;
            bind.parse::<std::net::SocketAddr>()
                .context("Core bind address is invalid")?;
            BTreeMap::from([("BTS_CORE_BIND".into(), bind)])
        }
        Component::Addons => {
            ensure!(
                interactive(cli) || cli.core_http_url.is_some() || cli.core_ws_url.is_some(),
                "Non-interactive Addons configuration requires --core-http-url or --core-ws-url."
            );
            let http = match &cli.core_http_url {
                Some(value) => value.clone(),
                None if interactive(cli) => prompt(
                    "Core HTTP URL",
                    existing
                        .get("BTS_CORE_HTTP_URL")
                        .map(String::as_str)
                        .unwrap_or(bts_compat::LOCAL_CORE_HTTP_URL),
                )?,
                None => existing
                    .get("BTS_CORE_HTTP_URL")
                    .cloned()
                    .context("BTS_CORE_HTTP_URL is not configured")?,
            };
            let websocket = match &cli.core_ws_url {
                Some(value) => value.clone(),
                None if interactive(cli) => prompt(
                    "Core WebSocket URL",
                    existing
                        .get("BTS_CORE_WS_URL")
                        .map(String::as_str)
                        .unwrap_or(bts_compat::LOCAL_CORE_WEBSOCKET_URL),
                )?,
                None => existing
                    .get("BTS_CORE_WS_URL")
                    .cloned()
                    .context("BTS_CORE_WS_URL is not configured")?,
            };
            config::validate_http_url(&http, "BTS_CORE_HTTP_URL")?;
            config::validate_websocket_url(&websocket)?;
            BTreeMap::from([
                ("BTS_CORE_HTTP_URL".into(), http),
                ("BTS_CORE_WS_URL".into(), websocket),
                (
                    "BTS_ADDON_DATA_ROOT".into(),
                    existing
                        .get("BTS_ADDON_DATA_ROOT")
                        .cloned()
                        .unwrap_or_else(|| "/var/lib/bts/addons".into()),
                ),
            ])
        }
        Component::Cli => unreachable!("CLI has no service configuration"),
    };
    let changed = previous != values;
    if cli.dry_run {
        if !cli.quiet && !cli.json {
            println!(
                "Would write {} configuration.\n{}",
                component,
                config::redact(&config::render_environment(&values))
            );
        }
        return Ok(());
    }
    if !changed {
        if !cli.quiet && !cli.json {
            println!("{} configuration is already current.", component);
        }
        return Ok(());
    }
    config::write_secure(&path, &values)?;
    secure_config_ownership(cli, &path, component)?;
    let mut restart_error = None;
    if !cli.no_start && cli.root == Path::new("/") && component.unit().is_some() {
        let unit = component.unit().expect("checked service component");
        let restart = systemctl(&mut RealSystem, &cli.root, "restart", &[unit]);
        let restart = match restart {
            Ok(()) => {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                systemctl(&mut RealSystem, &cli.root, "is-active", &[unit]).with_context(|| {
                    format!("{component} did not remain active after configuration")
                })
            }
            Err(error) => Err(error),
        };
        if let Err(error) = restart {
            if component == Component::Telephony {
                restart_error = Some(error);
            } else {
                return Err(error);
            }
        }
    }
    if !cli.quiet && !cli.json {
        let palette = terminal_palette(false, false);
        println!(
            "{}",
            palette.success(&format!("✓ {component} configuration saved."))
        );
        if let Some(error) = restart_error {
            println!(
                "{}",
                palette.warning(&format!(
                    "! bts-telephony did not become ready after restart: {error}"
                ))
            );
            println!("  Configuration was saved; external services may still need setup.");
            println!("  See {TELEPHONY_SETUP_GUIDE}");
            println!("  Run: sudo bts-install doctor");
        } else if !cli.no_start && cli.root == Path::new("/") && component.unit().is_some() {
            println!(
                "{}",
                palette.success(&format!("✓ bts-{component} restarted."))
            );
        } else {
            println!("  {}", palette.dim(&path.display().to_string()));
        }
    }
    Ok(())
}

fn component_configuration_is_valid(root: &Path, component: Component) -> bool {
    let Some(name) = component.config_name() else {
        return true;
    };
    let Ok(text) = fs::read_to_string(root.join("etc/bts").join(name)) else {
        return false;
    };
    let Ok(values) = config::parse_environment(&text) else {
        return false;
    };
    match component {
        Component::Telephony => {
            config::validate_telephony(&values).is_ok()
                && values.get("BTS_CORE_URL").is_some_and(|url| {
                    config::validate_http_url(url, "BTS_CORE_URL").is_ok()
                })
        }
        _ => true,
    }
}

async fn telephony_startup_ready(
    values: &BTreeMap<String, String>,
    local_core_selected: bool,
) -> bool {
    let ari_ready = probe_ari(values).await == AriProbe::Accepted;
    let core_ready =
        local_core_selected || probe_core(&values["BTS_CORE_URL"]).await == CoreProbe::Reachable;
    let tts_ready = probe_tts(values).await == TtsProbe::Rendered;
    ari_ready && core_ready && tts_ready
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AriProbe {
    Accepted,
    AuthenticationFailed,
    HttpError(reqwest::StatusCode),
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TtsProbe {
    Rendered,
    HttpError(reqwest::StatusCode),
    InvalidResponse,
    UnreadableResponse,
    Unreachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreProbe {
    Reachable,
    HttpError(reqwest::StatusCode),
    Unreachable,
}

async fn probe_ari(values: &BTreeMap<String, String>) -> AriProbe {
    let url = values.get("BTS_ARI_URL").unwrap();
    let user = values.get("BTS_ARI_USERNAME").unwrap();
    let password = values.get("BTS_ARI_PASSWORD").unwrap();
    let endpoint = format!("{}/ari/api-docs/resources.json", url.trim_end_matches('/'));
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return AriProbe::Unreachable;
    };
    match client
        .get(endpoint)
        .basic_auth(user, Some(password))
        .send()
        .await
    {
        Ok(response)
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                || response.status() == reqwest::StatusCode::FORBIDDEN =>
        {
            AriProbe::AuthenticationFailed
        }
        Ok(response) if !response.status().is_success() => AriProbe::HttpError(response.status()),
        Ok(_) => AriProbe::Accepted,
        Err(_) => AriProbe::Unreachable,
    }
}

async fn probe_tts(values: &BTreeMap<String, String>) -> TtsProbe {
    let endpoint = values
        .get("BTS_KOKORO_URL")
        .map(String::as_str)
        .unwrap_or(DEFAULT_KOKORO_URL);
    let speed = values
        .get("BTS_KOKORO_SPEED")
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1.05);
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    else {
        return TtsProbe::Unreachable;
    };
    let response = match client
        .post(endpoint)
        .json(&serde_json::json!({
            "model": values.get("BTS_KOKORO_MODEL").map(String::as_str).unwrap_or("kokoro"),
            "voice": values.get("BTS_KOKORO_VOICE").map(String::as_str).unwrap_or("bf_emma"),
            "input": "Welcome to Bansleben Telephone Services.",
            "response_format": "wav",
            "speed": speed,
        }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return TtsProbe::Unreachable,
    };
    if !response.status().is_success() {
        return TtsProbe::HttpError(response.status());
    }
    let Ok(bytes) = response.bytes().await else {
        return TtsProbe::UnreadableResponse;
    };
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        TtsProbe::Rendered
    } else {
        TtsProbe::InvalidResponse
    }
}

async fn probe_core(endpoint: &str) -> CoreProbe {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return CoreProbe::Unreachable;
    };
    let health = format!("{}/health", endpoint.trim_end_matches('/'));
    match client.get(health).send().await {
        Ok(response) if response.status().is_success() => CoreProbe::Reachable,
        Ok(response) => CoreProbe::HttpError(response.status()),
        Err(_) => CoreProbe::Unreachable,
    }
}

async fn extend_remote_diagnostics(
    cli: &Cli,
    state: Option<&InstallerState>,
    report: &mut diagnostics::DoctorReport,
) {
    let Some(state) = state else { return };
    if state.release_channel == bts_install::LOCAL_RELEASE_CHANNEL {
        report.diagnostics.push(diagnostics::Diagnostic {
            component: None,
            severity: diagnostics::Severity::Ok,
            message: "Installed from a verified local release; online availability check skipped."
                .into(),
            suggested_action: None,
        });
    } else {
        match ReleaseClient::new(
            state.repository.clone(),
            state.release_channel.clone(),
            None,
        ) {
            Ok(client) => match client.fetch_manifest().await {
                Ok((manifest, _)) => report.diagnostics.push(diagnostics::Diagnostic {
                    component: None,
                    severity: diagnostics::Severity::Ok,
                    message: format!(
                        "Release manifest {} is compatible.",
                        manifest.release_version
                    ),
                    suggested_action: None,
                }),
                Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                    component: None,
                    severity: diagnostics::Severity::Warning,
                    message: format!("Release manifest could not be checked: {error}"),
                    suggested_action: Some(
                        "Check network access and the configured release repository.".into(),
                    ),
                }),
            },
            Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                component: None,
                severity: diagnostics::Severity::Error,
                message: format!("Release client configuration is invalid: {error}"),
                suggested_action: Some(
                    "Re-run installation with a valid --repository and --track or --release."
                        .into(),
                ),
            }),
        }
    }

    if cli.root == Path::new("/") && state.installed_components.contains(&Component::Core) {
        let url = format!(
            "{}{}",
            bts_compat::LOCAL_CORE_HTTP_URL.trim_end_matches('/'),
            bts_compat::CORE_API_DISCOVERY_PATH
        );
        let runtime: Result<bts_protocol::ApiDiscovery> = async {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()?;
            let response = client.get(url).send().await?.error_for_status()?;
            Ok(response.json().await?)
        }
        .await;
        match runtime {
            Ok(discovery) => {
                let installed = state
                    .component_versions
                    .get(&Component::Core)
                    .unwrap_or(&state.installed_version);
                report
                    .diagnostics
                    .push(diagnostics::runtime_version_diagnostic(
                        Component::Core,
                        installed,
                        &discovery.product_version.to_string(),
                    ));
            }
            Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                component: Some(Component::Core),
                severity: diagnostics::Severity::Error,
                message: format!("Core runtime version could not be verified: {error}"),
                suggested_action: Some("Run: sudo systemctl restart bts-core.service".into()),
            }),
        }
    }

    if cli.root == Path::new("/") && state.installed_components.contains(&Component::Addons) {
        let endpoint = format!(
            "{}{}",
            bts_compat::LOCAL_CORE_HTTP_URL.trim_end_matches('/'),
            bts_compat::CORE_ADDONS_PATH
        );
        let manifests = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(anyhow::Error::from);
        let manifests: Result<Vec<bts_protocol::addons::v2::AddonManifest>> = match manifests {
            Ok(client) => async {
                Ok(client
                    .get(endpoint)
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?)
            }
            .await,
            Err(error) => Err(error),
        };
        match manifests {
            Ok(manifests) => {
                let registered: BTreeSet<_> = manifests
                    .iter()
                    .map(|manifest| manifest.id.as_str())
                    .collect();
                for (id, name) in [
                    ("clock", "clock"),
                    ("weather", "weather"),
                    ("message", "clear-display"),
                ] {
                    let present = registered.contains(id);
                    report.diagnostics.push(diagnostics::Diagnostic {
                        component: Some(Component::Addons),
                        severity: if present {
                            diagnostics::Severity::Ok
                        } else {
                            diagnostics::Severity::Error
                        },
                        message: if present {
                            format!("{name} addon is registered with Core.")
                        } else {
                            format!("{name} addon is installed but not registered with Core.")
                        },
                        suggested_action: (!present)
                            .then(|| "Run: sudo systemctl restart bts-addons.service".into()),
                    });
                }
                let entries = manifests
                    .iter()
                    .flat_map(|manifest| &manifest.menu)
                    .collect::<Vec<_>>();
                let unique_digits = entries
                    .iter()
                    .map(|entry| entry.digit)
                    .collect::<BTreeSet<_>>();
                report.diagnostics.push(diagnostics::Diagnostic {
                    component: Some(Component::Telephony),
                    severity: if entries.len() == unique_digits.len() && !entries.is_empty() {
                        diagnostics::Severity::Ok
                    } else {
                        diagnostics::Severity::Error
                    },
                    message: if entries.len() == unique_digits.len() && !entries.is_empty() {
                        format!("Telephony menu contains {} unique addon entries.", entries.len())
                    } else {
                        "Telephony menu is empty or contains duplicate digits.".into()
                    },
                    suggested_action: (entries.len() != unique_digits.len() || entries.is_empty())
                        .then(|| "Review addon menu assignments, then restart bts-addons.service.".into()),
                });
            }
            Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                component: Some(Component::Addons),
                severity: diagnostics::Severity::Error,
                message: format!("Core addon registration could not be verified: {error}"),
                suggested_action: Some("Run: sudo systemctl restart bts-addons.service".into()),
            }),
        }
    }

    if state.installed_components.contains(&Component::Display) {
        let result =
            read_component_configuration(&cli.root, Component::Display).and_then(|values| {
                let url = values
                    .get("BTS_CORE_WS_URL")
                    .context("BTS_CORE_WS_URL is not configured")?;
                config::validate_display(&values)?;
                Ok(url
                    .replace("ws://", "http://")
                    .replace("wss://", "https://")
                    .replace(bts_compat::CORE_TERMINALS_WEBSOCKET_PATH, "/health"))
            });
        match result {
            Ok(url) => {
                let response = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(3))
                    .build()
                    .map(|client| client.get(url));
                let reachable = match response {
                    Ok(request) => request
                        .send()
                        .await
                        .is_ok_and(|value| value.status().is_success()),
                    Err(_) => false,
                };
                report.diagnostics.push(diagnostics::Diagnostic {
                    component: Some(Component::Display),
                    severity: if reachable {
                        diagnostics::Severity::Ok
                    } else {
                        diagnostics::Severity::Error
                    },
                    message: if reachable {
                        "Configured Core endpoint is reachable.".into()
                    } else {
                        "Configured Core endpoint is unreachable.".into()
                    },
                    suggested_action: (!reachable)
                        .then(|| "Run: sudo bts-install configure display".into()),
                });
            }
            Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                component: Some(Component::Display),
                severity: if diagnostics::is_permission_denied(&error) {
                    diagnostics::Severity::Warning
                } else {
                    diagnostics::Severity::Error
                },
                message: if diagnostics::is_permission_denied(&error) {
                    "Display endpoint check requires access to protected configuration.".into()
                } else {
                    error.to_string()
                },
                suggested_action: Some(if diagnostics::is_permission_denied(&error) {
                    "Run: sudo bts-install doctor for protected endpoint checks.".into()
                } else {
                    "Run: sudo bts-install configure display".into()
                }),
            }),
        }
    }

    if state.installed_components.contains(&Component::Telephony) {
        let result =
            read_component_configuration(&cli.root, Component::Telephony).and_then(|values| {
                config::validate_telephony(&values)?;
                config::validate_http_url(
                    values
                        .get("BTS_CORE_URL")
                        .context("BTS_CORE_URL is not configured")?,
                    "BTS_CORE_URL",
                )?;
                Ok(values)
            });
        match result {
            Ok(values) => {
                let core_endpoint = &values["BTS_CORE_URL"];
                match probe_core(core_endpoint).await {
                    CoreProbe::Reachable => report.diagnostics.push(diagnostics::Diagnostic {
                        component: Some(Component::Telephony),
                        severity: diagnostics::Severity::Ok,
                        message: format!("BTS Core reachable.\n  Endpoint: {core_endpoint}"),
                        suggested_action: None,
                    }),
                    CoreProbe::HttpError(status) => report.diagnostics.push(diagnostics::Diagnostic {
                        component: Some(Component::Telephony),
                        severity: diagnostics::Severity::Error,
                        message: format!(
                            "BTS Core responded with HTTP {status}.\n  Endpoint: {core_endpoint}"
                        ),
                        suggested_action: Some(telephony_diagnostic_action()),
                    }),
                    CoreProbe::Unreachable => report.diagnostics.push(diagnostics::Diagnostic {
                        component: Some(Component::Telephony),
                        severity: diagnostics::Severity::Error,
                        message: format!("BTS Core unreachable.\n  Endpoint: {core_endpoint}"),
                        suggested_action: Some(telephony_diagnostic_action()),
                    }),
                }

                let ari_endpoint = &values["BTS_ARI_URL"];
                let ari_username = &values["BTS_ARI_USERNAME"];
                match probe_ari(&values).await {
                    AriProbe::Accepted => {
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Ok,
                            message: format!(
                                "Asterisk ARI reachable.\n  Endpoint: {ari_endpoint}"
                            ),
                            suggested_action: None,
                        });
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Ok,
                            message: format!(
                                "ARI credentials accepted.\n  Username: {ari_username}"
                            ),
                            suggested_action: None,
                        });
                    }
                    AriProbe::AuthenticationFailed => {
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Ok,
                            message: format!(
                                "Asterisk ARI reachable.\n  Endpoint: {ari_endpoint}"
                            ),
                            suggested_action: None,
                        });
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Error,
                            message: format!(
                                "Asterisk ARI authentication failed.\n  Endpoint: {ari_endpoint}\n  Username: {ari_username}"
                            ),
                            suggested_action: Some(telephony_diagnostic_action()),
                        });
                    }
                    AriProbe::HttpError(status) => report.diagnostics.push(diagnostics::Diagnostic {
                        component: Some(Component::Telephony),
                        severity: diagnostics::Severity::Error,
                        message: format!(
                            "Asterisk ARI responded with HTTP {status}.\n  Endpoint: {ari_endpoint}"
                        ),
                        suggested_action: Some(telephony_diagnostic_action()),
                    }),
                    AriProbe::Unreachable => report.diagnostics.push(diagnostics::Diagnostic {
                        component: Some(Component::Telephony),
                        severity: diagnostics::Severity::Error,
                        message: format!(
                            "Asterisk ARI unreachable.\n  Endpoint: {ari_endpoint}"
                        ),
                        suggested_action: Some(telephony_diagnostic_action()),
                    }),
                }

                let tts_endpoint = values
                    .get("BTS_KOKORO_URL")
                    .map(String::as_str)
                    .unwrap_or(DEFAULT_KOKORO_URL);
                match probe_tts(&values).await {
                    TtsProbe::Rendered => {
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Ok,
                            message: format!(
                                "Kokoro TTS reachable.\n  Endpoint: {tts_endpoint}"
                            ),
                            suggested_action: None,
                        });
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Ok,
                            message: "Test speech rendered successfully.".into(),
                            suggested_action: None,
                        });
                    }
                    TtsProbe::HttpError(status) => {
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Ok,
                            message: format!(
                                "Kokoro TTS reachable.\n  Endpoint: {tts_endpoint}"
                            ),
                            suggested_action: None,
                        });
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Error,
                            message: format!(
                                "Kokoro TTS returned HTTP {status}.\n  Endpoint: {tts_endpoint}"
                            ),
                            suggested_action: Some(telephony_diagnostic_action()),
                        });
                    }
                    TtsProbe::InvalidResponse | TtsProbe::UnreadableResponse => {
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Ok,
                            message: format!(
                                "Kokoro TTS reachable.\n  Endpoint: {tts_endpoint}"
                            ),
                            suggested_action: None,
                        });
                        report.diagnostics.push(diagnostics::Diagnostic {
                            component: Some(Component::Telephony),
                            severity: diagnostics::Severity::Error,
                            message: format!(
                                "Kokoro TTS responded but returned an unusable TTS response.\n  Endpoint: {tts_endpoint}"
                            ),
                            suggested_action: Some(telephony_diagnostic_action()),
                        });
                    }
                    TtsProbe::Unreachable => report.diagnostics.push(diagnostics::Diagnostic {
                        component: Some(Component::Telephony),
                        severity: diagnostics::Severity::Error,
                        message: format!(
                            "Kokoro TTS unreachable.\n  Endpoint: {tts_endpoint}"
                        ),
                        suggested_action: Some(telephony_diagnostic_action()),
                    }),
                }
            }
            Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                component: Some(Component::Telephony),
                severity: if diagnostics::is_permission_denied(&error) {
                    diagnostics::Severity::Warning
                } else {
                    diagnostics::Severity::Error
                },
                message: if diagnostics::is_permission_denied(&error) {
                    "Telephony service checks require access to protected configuration.".into()
                } else {
                    format!("Telephony configuration is invalid: {error}")
                },
                suggested_action: Some(if diagnostics::is_permission_denied(&error) {
                    "Run: sudo bts-install doctor for protected Telephony checks.".into()
                } else {
                    telephony_diagnostic_action()
                }),
            }),
        }
    }
    report.healthy = !report
        .diagnostics
        .iter()
        .any(|item| item.severity == diagnostics::Severity::Error);
}

fn telephony_diagnostic_action() -> String {
    format!(
        "See {TELEPHONY_SETUP_GUIDE}; then run: sudo bts-install doctor"
    )
}

fn read_component_configuration(
    root: &Path,
    component: Component,
) -> Result<BTreeMap<String, String>> {
    let config_name = component
        .config_name()
        .context("The CLI component has no service configuration")?;
    let path = rooted(root, &format!("/etc/bts/{config_name}"));
    config::parse_environment(
        &fs::read_to_string(&path).with_context(|| format!("Could not read {}", path.display()))?,
    )
}

fn prepare_display_host(
    cli: &Cli,
    system: &mut impl SystemAdapter,
    state: &mut InstallerState,
) -> Result<()> {
    if !cli.yes && !cli.dry_run && interactive(cli) {
        ensure!(
            confirm("Display will take control of tty1 and disable its login prompt. Continue")?,
            "Display installation was cancelled."
        );
    }
    if cli.root == Path::new("/") {
        let groups = ["video", "render", "input", "seat"]
            .into_iter()
            .filter(|group| {
                system
                    .output("getent", &["group".into(), (*group).into()])
                    .is_ok()
            })
            .collect::<Vec<_>>();
        if !groups.is_empty() {
            system.run(
                "usermod",
                &["-aG".into(), groups.join(","), "bts-display".into()],
            )?;
        }
        systemctl(system, &cli.root, "enable", &["seatd.service"])?;
        systemctl(
            system,
            &cli.root,
            "disable",
            &["--now", "getty@tty1.service"],
        )?;
        systemctl(system, &cli.root, "mask", &["getty@tty1.service"])?;
    }
    state.tty1_managed = true;
    Ok(())
}

fn migrate_legacy_voice_assets(cli: &Cli) -> Result<()> {
    // These exact names were created by BTS's former static prompt generator.
    // Do not scan or remove any other Asterisk sounds: operators may own them.
    const LEGACY_BTS_PROMPTS: &[&str] = &[
        "welcome.wav",
        "press-0-clear.wav",
        "press-2-time.wav",
        "press-3-weather.wav",
        "press-4-clear.wav",
        "configuration.wav",
        "press-1-change-terminal.wav",
        "press-star-return.wav",
        "press-0-configuration.wav",
        "press-hash-confirm.wav",
        "no-terminals-online.wav",
        "select-terminal.wav",
        "target-selected.wav",
        "target-unavailable.wav",
        "invalid-selection.wav",
        "returned-to-addon.wav",
    ];
    let directory = rooted(&cli.root, "/var/lib/asterisk/sounds/en/bts");
    for name in LEGACY_BTS_PROMPTS {
        let path = directory.join(name);
        if path.is_file() {
            fs::remove_file(&path)
                .with_context(|| format!("Could not remove legacy BTS prompt {}", path.display()))?;
        }
    }
    Ok(())
}

fn restore_tty1(cli: &Cli, system: &mut impl SystemAdapter) -> Result<()> {
    if cli.root == Path::new("/") {
        systemctl(system, &cli.root, "unmask", &["getty@tty1.service"])?;
        systemctl(
            system,
            &cli.root,
            "enable",
            &["--now", "getty@tty1.service"],
        )?;
    }
    Ok(())
}

fn remove_component(cli: &Cli, component: Component, purge: bool) -> Result<()> {
    let current = rooted(
        &cli.root,
        &format!("/usr/lib/bts/components/{component}/current"),
    );
    fs::remove_file(current).ok();
    if component == Component::Cli {
        fs::remove_file(rooted(&cli.root, "/usr/bin/btscli")).ok();
    }
    if let Some(unit) = component.unit() {
        fs::remove_file(rooted(
            &cli.root,
            &format!("/usr/lib/systemd/system/{unit}"),
        ))
        .ok();
    }
    if purge && component.config_name().is_some() {
        let config_name = component
            .config_name()
            .expect("checked configured component");
        fs::remove_file(rooted(&cli.root, &format!("/etc/bts/{config_name}"))).ok();
    }
    if purge && component == Component::Telephony {
        for path in [
            "/var/cache/bts/voice",
            "/var/lib/asterisk/sounds/en/bts-generated",
        ] {
            let path = rooted(&cli.root, path);
            if path.is_dir() {
                fs::remove_dir_all(&path).with_context(|| {
                    format!("Could not remove BTS-owned voice data {}", path.display())
                })?;
            }
        }
    }
    Ok(())
}

fn persist_uninstall_state(
    state_path: &Path,
    state: &mut Option<InstallerState>,
    next: InstallerState,
) -> Result<()> {
    if next.installed_components.is_empty() {
        fs::remove_file(state_path).ok();
        *state = None;
    } else {
        next.write_atomic(state_path)?;
        *state = Some(next);
    }
    Ok(())
}

fn activation_id(version: &str, checksum: &str) -> String {
    format!(
        "{}-{}",
        version.trim_start_matches('v'),
        &checksum[..checksum.len().min(12)]
    )
}

fn secure_config_ownership(cli: &Cli, path: &Path, component: Component) -> Result<()> {
    if cli.root == Path::new("/") {
        let owner = if component == Component::Display {
            "root:bts-display"
        } else {
            "root:bts"
        };
        let status = std::process::Command::new("chown")
            .arg(owner)
            .arg(path)
            .status()?;
        ensure!(
            status.success(),
            "Could not set secure service ownership on {}.",
            path.display()
        );
    }
    Ok(())
}

fn confirm_plan(cli: &Cli, plan: &InstallationPlan) -> Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string_pretty(plan)?);
    } else if !cli.quiet {
        if cli.dry_run {
            println!(
                "Resolved plan: {}",
                if plan.actions.is_empty() {
                    "no changes are required".to_owned()
                } else {
                    plan.actions
                        .iter()
                        .map(|action| format!("\n  - {action:?}"))
                        .collect::<String>()
                }
            );
        } else {
            println!(
                "{}",
                human_plan(plan, INSTALLER_VERSION, terminal_palette(false, false))
            );
        }
    }
    if cli.dry_run || plan.actions.is_empty() || cli.yes {
        return Ok(());
    }
    ensure!(
        interactive(cli),
        "Host changes require --yes in non-interactive mode."
    );
    ensure!(confirm("Apply this plan")?, "Operation cancelled.");
    Ok(())
}

fn choose_configuration_component(
    requested: Option<Component>,
    state: &InstallerState,
    cli: &Cli,
) -> Result<Component> {
    if let Some(component) = requested {
        ensure!(
            state.installed_components.contains(&component),
            "{} is not installed.",
            component
        );
        return Ok(component);
    }
    ensure!(
        interactive(cli),
        "Non-interactive configure requires a component."
    );
    println!(
        "Installed components: {}",
        join_components(&state.installed_components)
    );
    let value = prompt("Component to configure", "")?;
    let component = value.parse()?;
    ensure!(
        state.installed_components.contains(&component),
        "{} is not installed.",
        component
    );
    Ok(component)
}

fn select_upgrade_components(
    state: &InstallerState,
    requested: &[Component],
) -> Result<BTreeSet<Component>> {
    if requested.is_empty() {
        return Ok(state.installed_components.clone());
    }
    let selected: BTreeSet<_> = requested.iter().copied().collect();
    for component in &selected {
        ensure!(
            state.installed_components.contains(component),
            "Cannot upgrade {} because it is not installed.",
            component
        );
    }
    Ok(selected)
}

fn print_status(report: &diagnostics::StatusReport, json: bool, quiet: bool) -> Result<()> {
    if quiet {
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    let palette = terminal_palette(json, quiet);
    println!(
        "{}",
        palette.accent(&format!("BTS {} (installer {})",
        report
            .installed_version
            .as_deref()
            .unwrap_or("not installed"),
        report.installer_version
        ))
    );
    for item in &report.components {
        println!(
            "  {:<10} {}{}{}",
            item.component,
            if item.installed {
                "installed"
            } else {
                "not installed"
            },
            item.version
                .as_ref()
                .map(|value| format!(" at {value}"))
                .unwrap_or_default(),
            item.configured_endpoint
                .as_ref()
                .map(|value| format!(", endpoint {value}"))
                .unwrap_or_default()
        );
    }
    Ok(())
}

fn print_doctor(report: &diagnostics::DoctorReport, json: bool, quiet: bool) -> Result<()> {
    if quiet {
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    let palette = terminal_palette(json, quiet);
    for item in &report.diagnostics {
        let marker = match item.severity {
            diagnostics::Severity::Ok => palette.success("✓"),
            diagnostics::Severity::Warning => palette.warning("!"),
            diagnostics::Severity::Error => palette.error("✗"),
        };
        println!(
            "{} {}",
            marker,
            item.message
        );
        if let Some(action) = &item.suggested_action {
            println!("  {}", palette.dim(action));
        }
    }
    Ok(())
}

fn print_licence(cli: &Cli) -> Result<()> {
    if !io::stdout().is_terminal() || cli.quiet {
        print!("{LICENCE}");
        return Ok(());
    }
    if let Ok(pager) = std::env::var("PAGER") {
        let mut child = std::process::Command::new(pager)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(LICENCE.as_bytes())?;
        ensure!(child.wait()?.success(), "Pager failed.");
    } else {
        print!("{LICENCE}");
    }
    Ok(())
}

fn prompt(label: &str, default: &str) -> Result<String> {
    print!(
        "{label}{}: ",
        if default.is_empty() {
            "".into()
        } else {
            format!(" [{default}]")
        }
    );
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim();
    Ok(if value.is_empty() {
        default.into()
    } else {
        value.into()
    })
}
fn confirm(label: &str) -> Result<bool> {
    Ok(matches!(
        prompt(&format!("{label} [y/N]"), "")?
            .to_ascii_lowercase()
            .as_str(),
        "y" | "yes"
    ))
}
fn interactive(cli: &Cli) -> bool {
    !cli.quiet && !cli.json && io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn terminal_palette(json: bool, quiet: bool) -> Palette {
    Palette::new(
        !json
            && !quiet
            && io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none(),
    )
}
fn require_root_or_alternate(root: &Path) -> Result<()> {
    if root == Path::new("/") {
        ensure!(unsafe { libc::geteuid() } == 0, "Run bts-install as root.");
    }
    Ok(())
}
fn rooted(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}
fn timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
fn join_components(values: &BTreeSet<Component>) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use bts_install::{platform::Architecture, system::RecordingSystem};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn serve_once(response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(response).await.unwrap();
        });
        format!("http://{address}")
    }

    fn telephony_values(ari_url: String, kokoro_url: String) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("BTS_ARI_URL".into(), ari_url),
            ("BTS_ARI_USERNAME".into(), "bts".into()),
            ("BTS_ARI_PASSWORD".into(), "never-print-this".into()),
            ("BTS_CORE_URL".into(), "http://127.0.0.1:3100".into()),
            ("BTS_KOKORO_URL".into(), kokoro_url),
        ])
    }

    #[test]
    fn upgrade_defaults_to_installed_and_rejects_others() {
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        state.installed_components.insert(Component::Core);
        assert_eq!(
            select_upgrade_components(&state, &[]).unwrap(),
            [Component::Core].into()
        );
        assert!(select_upgrade_components(&state, &[Component::Display]).is_err());
    }

    #[test]
    fn version_warranty_and_quiet_contracts_are_offline() {
        assert!(bts_install::warranty_notice().contains("NO WARRANTY"));
        assert!(bts_install::COPYRIGHT.contains("BTS contributors"));
        assert_eq!(INSTALLER_VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn telephony_component_reconciliation_includes_runtime_filesystem_access() {
        let mut system = RecordingSystem::default();
        system.outputs.insert("stat".into(), "asterisk".into());
        system
            .outputs
            .insert("getent".into(), "asterisk:x:995:".into());
        system.outputs.insert("id".into(), "bts".into());

        let changed = reconcile_component_runtime_access(
            &mut system,
            Path::new("/"),
            Component::Telephony,
            Path::new("/srv/asterisk/sounds/custom/bts-generated"),
        )
        .unwrap();

        assert!(changed);
        assert!(system.commands.iter().any(|(program, arguments)| {
            program == "usermod" && arguments == &["-aG", "asterisk", "bts"]
        }));
        assert!(system.commands.iter().any(|(program, arguments)| {
            program == "install"
                && arguments.last().map(String::as_str)
                    == Some("/srv/asterisk/sounds/custom/bts-generated")
        }));
    }

    #[test]
    fn display_configuration_requires_identity_and_uses_terminal_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let cli = Cli::parse([
            "bts-install",
            "install",
            "display",
            "--root",
            root.path().to_str().unwrap(),
            "--yes",
        ])
        .unwrap();
        assert!(ensure_default_configuration(&cli, Component::Display, false).is_err());
        assert!(ensure_default_configuration(&cli, Component::Display, true).is_err());
        let cli = Cli::parse([
            "bts-install",
            "install",
            "display",
            "--root",
            root.path().to_str().unwrap(),
            "--yes",
            "--terminal-id",
            "bedroom-display",
            "--terminal-name",
            "Bedroom",
        ])
        .unwrap();
        ensure_default_configuration(&cli, Component::Display, true).unwrap();
        let contents = fs::read_to_string(root.path().join("etc/bts/display.env")).unwrap();
        assert!(contents.contains("ws://127.0.0.1:3100/api/v1/terminals/ws"));
        assert!(contents.contains("BTS_TERMINAL_ID=\"bedroom-display\""));
        assert!(contents.contains("BTS_TERMINAL_NAME=\"Bedroom\""));
        assert!(contents.contains("BTS_CAGE_ARGS=\"-m last\""));
    }

    #[test]
    fn telephony_install_uses_protected_secret_input_and_preserves_valid_configuration() {
        let root = tempfile::tempdir().unwrap();
        let secret = root.path().join("telephony-secret.env");
        fs::write(&secret, "BTS_ARI_PASSWORD=first-secret\n").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        let cli = Cli::parse([
            "bts-install",
            "install",
            "server",
            "--root",
            root.path().to_str().unwrap(),
            "--secret-file",
            secret.to_str().unwrap(),
            "--yes",
        ])
        .unwrap();
        ensure_default_configuration(&cli, Component::Telephony, true).unwrap();
        let path = root.path().join("etc/bts/telephony.env");
        let first = config::parse_environment(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(first["BTS_ARI_PASSWORD"], "first-secret");
        assert_eq!(first["BTS_CORE_URL"], "http://127.0.0.1:3100");
        assert_eq!(first["BTS_ARI_URL"], DEFAULT_ARI_URL);
        assert_eq!(first["BTS_KOKORO_URL"], DEFAULT_KOKORO_URL);
        assert_eq!(first["BTS_KOKORO_VOICE"], "bf_emma");
        assert_eq!(first["BTS_KOKORO_MODEL_VERSION"], "0.6.0");

        fs::write(&secret, "BTS_ARI_PASSWORD=replacement\n").unwrap();
        ensure_default_configuration(&cli, Component::Telephony, true).unwrap();
        let preserved = config::parse_environment(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(preserved["BTS_ARI_PASSWORD"], "first-secret");
    }

    #[test]
    fn telephony_install_requires_non_empty_ari_password() {
        let root = tempfile::tempdir().unwrap();
        let secret = root.path().join("telephony-secret.env");
        fs::write(&secret, "BTS_ARI_PASSWORD=\n").unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        let cli = Cli::parse([
            "bts-install",
            "install",
            "telephony",
            "--root",
            root.path().to_str().unwrap(),
            "--secret-file",
            secret.to_str().unwrap(),
            "--yes",
        ])
        .unwrap();
        let error = ensure_default_configuration(&cli, Component::Telephony, true).unwrap_err();
        assert!(error.to_string().contains("BTS_ARI_PASSWORD is not configured"));
    }

    #[test]
    fn telephony_install_persists_remote_services_without_requiring_reachability() {
        let root = tempfile::tempdir().unwrap();
        let secret = root.path().join("telephony-secret.env");
        fs::write(
            &secret,
            concat!(
                "BTS_ARI_URL=http://asterisk.lan:8088\n",
                "BTS_ARI_USERNAME=operator\n",
                "BTS_ARI_PASSWORD=remote-secret\n",
                "BTS_KOKORO_URL=http://speech.lan:8880/v1/audio/speech\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o600)).unwrap();
        let cli = Cli::parse([
            "bts-install",
            "install",
            "telephony",
            "--root",
            root.path().to_str().unwrap(),
            "--secret-file",
            secret.to_str().unwrap(),
            "--core-http-url",
            "http://core.lan:3100",
            "--yes",
        ])
        .unwrap();

        ensure_default_configuration(&cli, Component::Telephony, false).unwrap();

        let contents = fs::read_to_string(root.path().join("etc/bts/telephony.env")).unwrap();
        let values = config::parse_environment(&contents).unwrap();
        assert_eq!(values["BTS_ARI_URL"], "http://asterisk.lan:8088");
        assert_eq!(
            values["BTS_KOKORO_URL"],
            "http://speech.lan:8880/v1/audio/speech"
        );
        assert_eq!(values["BTS_ARI_PASSWORD"], "remote-secret");
        assert!(!config::redact(&contents).contains("remote-secret"));
    }

    #[test]
    fn non_interactive_telephony_install_rejects_missing_protected_input() {
        let root = tempfile::tempdir().unwrap();
        let cli = Cli::parse([
            "bts-install",
            "install",
            "telephony",
            "--root",
            root.path().to_str().unwrap(),
            "--core-http-url",
            "http://core.lan:3100",
            "--yes",
        ])
        .unwrap();

        let error = ensure_default_configuration(&cli, Component::Telephony, false).unwrap_err();

        assert!(error.to_string().contains("--secret-file or --secret-fd"));
        assert!(!root.path().join("etc/bts/telephony.env").exists());
    }

    #[tokio::test]
    async fn telephony_probes_distinguish_authentication_and_invalid_audio() {
        let ari_url = serve_once(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n").await;
        let kokoro_url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 9\r\n\r\nnot audio",
        )
        .await;
        let values = telephony_values(ari_url, kokoro_url);

        let ari_result = probe_ari(&values).await;
        assert_eq!(ari_result, AriProbe::AuthenticationFailed);
        assert_eq!(probe_tts(&values).await, TtsProbe::InvalidResponse);
        assert!(!format!("{ari_result:?}").contains("never-print-this"));
    }

    #[tokio::test]
    async fn telephony_probes_distinguish_unreachable_services() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unreachable = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let values = telephony_values(unreachable.clone(), unreachable);

        assert_eq!(probe_ari(&values).await, AriProbe::Unreachable);
        assert_eq!(probe_tts(&values).await, TtsProbe::Unreachable);
    }

    #[tokio::test]
    async fn telephony_startup_readiness_requires_rendered_tts() {
        let ari_url = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await;
        let kokoro_url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 9\r\n\r\nnot audio",
        )
        .await;
        let values = telephony_values(ari_url, kokoro_url);
        assert!(!telephony_startup_ready(&values, true).await);
    }

    #[tokio::test]
    async fn doctor_reports_ari_authentication_and_invalid_kokoro_separately() {
        let root = tempfile::tempdir().unwrap();
        let core_url = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        let ari_url = serve_once(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n").await;
        let kokoro_url = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 9\r\n\r\nnot audio",
        )
        .await;
        let mut values = telephony_values(ari_url, kokoro_url);
        values.insert("BTS_CORE_URL".into(), core_url);
        let path = root.path().join("etc/bts/telephony.env");
        config::write_secure(&path, &values).unwrap();
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        state.installed_components.insert(Component::Telephony);
        state.release_channel = bts_install::LOCAL_RELEASE_CHANNEL.into();
        let cli = Cli::parse([
            "bts-install",
            "--root",
            root.path().to_str().unwrap(),
            "doctor",
        ])
        .unwrap();
        let mut report = diagnostics::DoctorReport {
            schema_version: diagnostics::OUTPUT_SCHEMA_VERSION,
            healthy: true,
            diagnostics: Vec::new(),
        };

        extend_remote_diagnostics(&cli, Some(&state), &mut report).await;

        let output = report
            .diagnostics
            .iter()
            .map(|item| item.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("BTS Core reachable"));
        assert!(output.contains("Asterisk ARI authentication failed"));
        assert!(output.contains("unusable TTS response"));
        assert!(!output.contains("never-print-this"));
        assert!(!report.healthy);
    }

    #[tokio::test]
    async fn doctor_reports_unreachable_ari_and_kokoro_separately() {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unreachable = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let mut values = telephony_values(unreachable.clone(), unreachable.clone());
        values.insert("BTS_CORE_URL".into(), unreachable);
        config::write_secure(&root.path().join("etc/bts/telephony.env"), &values).unwrap();
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        state.installed_components.insert(Component::Telephony);
        state.release_channel = bts_install::LOCAL_RELEASE_CHANNEL.into();
        let cli = Cli::parse([
            "bts-install",
            "--root",
            root.path().to_str().unwrap(),
            "doctor",
        ])
        .unwrap();
        let mut report = diagnostics::DoctorReport {
            schema_version: diagnostics::OUTPUT_SCHEMA_VERSION,
            healthy: true,
            diagnostics: Vec::new(),
        };

        extend_remote_diagnostics(&cli, Some(&state), &mut report).await;

        let output = report
            .diagnostics
            .iter()
            .map(|item| item.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("BTS Core unreachable"));
        assert!(output.contains("Asterisk ARI unreachable"));
        assert!(output.contains("Kokoro TTS unreachable"));
    }

    #[test]
    fn legacy_voice_migration_removes_only_proven_bts_assets() {
        let root = tempfile::tempdir().unwrap();
        let sounds = root.path().join("var/lib/asterisk/sounds/en/bts");
        fs::create_dir_all(&sounds).unwrap();
        fs::write(sounds.join("press-0-clear.wav"), b"legacy").unwrap();
        fs::write(sounds.join("press-3-weather.wav"), b"legacy").unwrap();
        fs::write(sounds.join("operator-custom.wav"), b"custom").unwrap();
        let cli = Cli::parse([
            "bts-install",
            "install",
            "server",
            "--root",
            root.path().to_str().unwrap(),
            "--yes",
        ])
        .unwrap();
        migrate_legacy_voice_assets(&cli).unwrap();
        assert!(!sounds.join("press-0-clear.wav").exists());
        assert!(!sounds.join("press-3-weather.wav").exists());
        assert!(sounds.join("operator-custom.wav").exists());
    }

    #[test]
    fn display_upgrade_requires_explicit_legacy_migration() {
        let root = tempfile::tempdir().unwrap();
        let config_directory = root.path().join("etc/bts");
        fs::create_dir_all(&config_directory).unwrap();
        fs::write(
            config_directory.join("display.env"),
            "BTS_CORE_WS_URL=ws://core:3100/api/v1/events/ws\n",
        )
        .unwrap();
        let cli = Cli::parse([
            "bts-install",
            "upgrade",
            "display",
            "--root",
            root.path().to_str().unwrap(),
            "--yes",
        ])
        .unwrap();
        let selected = BTreeSet::from([Component::Display]);
        assert!(require_display_migration_before_upgrade(&cli, &selected).is_err());

        fs::write(
            config_directory.join("display.env"),
            concat!(
                "BTS_CORE_WS_URL=ws://core:3100/api/v1/terminals/ws\n",
                "BTS_TERMINAL_ID=bedroom-display\n",
                "BTS_TERMINAL_NAME=Bedroom\n",
            ),
        )
        .unwrap();
        require_display_migration_before_upgrade(&cli, &selected).unwrap();
    }

    #[test]
    fn tty1_is_stopped_on_takeover_and_started_on_restore() {
        let mut cli = Cli::parse(["bts-install", "status"]).unwrap();
        cli.yes = true;
        let mut system = RecordingSystem::default();
        let mut state = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);

        prepare_display_host(&cli, &mut system, &mut state).unwrap();
        assert!(system.commands.contains(&(
            "systemctl".into(),
            vec![
                "disable".into(),
                "--now".into(),
                "getty@tty1.service".into()
            ]
        )));

        restore_tty1(&cli, &mut system).unwrap();
        assert!(system.commands.contains(&(
            "systemctl".into(),
            vec!["enable".into(), "--now".into(), "getty@tty1.service".into()]
        )));
    }

    #[test]
    fn uninstall_updates_in_memory_state() {
        let root = tempfile::tempdir().unwrap();
        let state_path = root.path().join("var/lib/bts-install/state.json");
        let installed = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        let mut state = Some(installed);

        persist_uninstall_state(
            &state_path,
            &mut state,
            InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64),
        )
        .unwrap();

        assert!(state.is_none());

        let mut remaining = InstallerState::new("0.3.0", Platform::Debian, Architecture::X86_64);
        remaining.installed_components.insert(Component::Core);
        persist_uninstall_state(&state_path, &mut state, remaining).unwrap();

        assert!(
            state
                .as_ref()
                .is_some_and(|value| value.installed_components.contains(&Component::Core))
        );
        assert!(state_path.exists());
    }

    #[test]
    fn migration_writes_authoritative_component_files_then_removes_shared_file() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("etc/bts");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("bts.env"),
            concat!(
                "BTS_CORE_BIND=127.0.0.1:3100\n",
                "BTS_CORE_HTTP_URL=http://core:3100\n",
                "BTS_CORE_WS_URL=ws://core:3100/api/v1/events/ws\n",
            ),
        )
        .unwrap();
        fs::write(
            directory.join("addons.env"),
            "BTS_ADDON_DATA_ROOT=/srv/bts/addons\n",
        )
        .unwrap();
        let cli = Cli::parse([
            "bts-install",
            "status",
            "--root",
            root.path().to_str().unwrap(),
        ])
        .unwrap();

        migrate_legacy_configuration(&cli, &[Component::Core, Component::Addons].into()).unwrap();

        assert!(!directory.join("bts.env").exists());
        let core =
            config::parse_environment(&fs::read_to_string(directory.join("core.env")).unwrap())
                .unwrap();
        let addons =
            config::parse_environment(&fs::read_to_string(directory.join("addons.env")).unwrap())
                .unwrap();
        assert_eq!(core["BTS_CORE_BIND"], "127.0.0.1:3100");
        assert_eq!(addons["BTS_ADDON_DATA_ROOT"], "/srv/bts/addons");
        assert_eq!(addons["BTS_CORE_HTTP_URL"], "http://core:3100");
    }

    #[test]
    fn purge_removes_only_the_selected_component_configuration() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("etc/bts");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("core.env"), "BTS_CORE_BIND=0.0.0.0:3100\n").unwrap();
        fs::write(
            directory.join("addons.env"),
            "BTS_CORE_HTTP_URL=http://core:3100\n",
        )
        .unwrap();
        let mut cli = Cli::parse(["bts-install", "status"]).unwrap();
        cli.root = root.path().into();

        remove_component(&cli, Component::Addons, true).unwrap();

        assert!(directory.join("core.env").exists());
        assert!(!directory.join("addons.env").exists());
    }
}
