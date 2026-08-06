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
    plan::{Action, InstallationPlan},
    platform::{Platform, detect_host},
    release::ReleaseClient,
    state::InstallerState,
    system::{RealSystem, SystemAdapter, create_service_account, systemctl},
};

const LICENCE: &str = include_str!("../../LICENSE");

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
                execute_plan(&cli, &plan, &mut next, platform, architecture).await?;
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
                execute_plan(&cli, &plan, &mut next, platform, architecture).await?;
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
                execute_plan(&cli, &plan, &mut next, platform, architecture).await?;
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
                execute_plan(&cli, &plan, &mut next, platform, architecture).await?;
                next.installed_components = plan.after.clone();
                persist_uninstall_state(&state_path, &mut state, next)?;
            }
        }
        _ => unreachable!(),
    }

    if !cli.quiet
        && !cli.dry_run
        && let Some(state) = state
    {
        println!(
            "BTS {} is reconciled with components: {}.\nRun 'bts-install doctor' to check the installation.",
            state.installed_version,
            join_components(&state.installed_components)
        );
    }
    Ok(())
}

async fn execute_plan(
    cli: &Cli,
    plan: &InstallationPlan,
    state: &mut InstallerState,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
) -> Result<()> {
    let added: Vec<_> = plan.after.difference(&plan.before).copied().collect();
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
    if !added.is_empty() {
        let (manifest, urls) = release_client(cli)?.fetch_manifest().await?;
        for component in added {
            let asset = manifest.select(component, platform, architecture)?;
            install_component(
                cli,
                &mut system,
                &manifest.release_version,
                component,
                asset,
                &urls,
            )
            .await?;
            if component == Component::Display {
                prepare_display_host(cli, &mut system, state)?;
            }
            ensure_default_configuration(cli, component, plan.after.contains(&Component::Core))?;
            systemctl(&mut system, &cli.root, "enable", &[component.unit()])?;
            if !cli.no_start && cli.root == Path::new("/") {
                systemctl(&mut system, &cli.root, "start", &[component.unit()])?;
            }
        }
        state.installed_version = manifest.release_version;
        for component in &plan.after {
            if !plan.before.contains(component) {
                state
                    .component_versions
                    .insert(*component, state.installed_version.clone());
            }
        }
        state.installer_version = INSTALLER_VERSION.into();
        state.platform = platform;
        state.architecture = architecture;
        record_release_source(cli, state);
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

fn record_release_source(cli: &Cli, state: &mut InstallerState) {
    state.repository = cli.repository.clone();
    state.release_channel = if cli.release_dir.is_some() {
        bts_install::LOCAL_RELEASE_CHANNEL.into()
    } else {
        cli.channel.clone()
    };
}

async fn install_component(
    cli: &Cli,
    system: &mut RealSystem,
    version: &str,
    component: Component,
    asset: &ComponentAsset,
    urls: &BTreeMap<String, String>,
) -> Result<()> {
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
        activation::activate(&cli.root, component, &activation_id)?;
        return Ok(());
    }
    if !cli.quiet {
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
    let bundle = temporary.path().join(component.binary());
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
    activation::activate(&cli.root, component, &activation_id)?;
    Ok(())
}

async fn upgrade(
    cli: &Cli,
    state: &mut InstallerState,
    selected: &BTreeSet<Component>,
    platform: Platform,
    architecture: bts_install::platform::Architecture,
) -> Result<()> {
    let (manifest, urls) = release_client(cli)?.fetch_manifest().await?;
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
    let actions: Vec<_> = pending
        .keys()
        .flat_map(|component| {
            [
                Action::Download {
                    component: *component,
                },
                Action::Stage {
                    component: *component,
                },
                Action::StopService {
                    unit: component.unit().into(),
                },
                Action::Activate {
                    component: *component,
                },
                Action::StartService {
                    unit: component.unit().into(),
                },
            ]
        })
        .collect();
    let plan = InstallationPlan {
        role: state.selected_role,
        before: state.installed_components.clone(),
        after: state.installed_components.clone(),
        actions,
    };
    confirm_plan(cli, &plan)?;
    if cli.dry_run {
        return Ok(());
    }
    let client = release_client(cli)?;
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
        let bundle = temporary.path().join(component.binary());
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
        let mut stopped = Vec::new();
        for (component, _) in &staged {
            if let Err(error) = systemctl(&mut system, &cli.root, "stop", &[component.unit()]) {
                for unit in stopped {
                    let _ = systemctl(&mut system, &cli.root, "start", &[unit]);
                }
                bail!("Could not stop {} safely: {error}", component);
            }
            stopped.push(component.unit());
        }
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
    if !cli.no_start && cli.root == Path::new("/") {
        for (component, _) in &staged {
            let started = systemctl(&mut system, &cli.root, "start", &[component.unit()])
                .and_then(|()| systemctl(&mut system, &cli.root, "is-active", &[component.unit()]));
            if started.is_err() {
                let rollback = activations.iter().rev().try_for_each(activation::rollback);
                restart_restored_services(cli, &mut system, &activations);
                bail!(
                    "{} failed its activation health check; rollback {}.",
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
    state.installed_version = manifest.release_version;
    for component in selected {
        state
            .component_versions
            .insert(*component, state.installed_version.clone());
    }
    state.installer_version = INSTALLER_VERSION.into();
    record_release_source(cli, state);
    if !pending.is_empty() {
        state.updated_at = Some(timestamp());
    }
    Ok(())
}

fn restart_restored_services(
    cli: &Cli,
    system: &mut RealSystem,
    activations: &[activation::Activation],
) {
    if cli.root != Path::new("/") {
        return;
    }
    for restored in activations {
        let _ = systemctl(system, &cli.root, "restart", &[restored.component.unit()]);
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
    for entry in fs::read_dir(release.join("systemd"))? {
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
    let path = rooted(&cli.root, &format!("/etc/bts/{}", component.config_name()));
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
            let mut values = existing;
            values
                .entry("BTS_ARI_URL".into())
                .or_insert_with(|| "http://localhost:8088".into());
            values
                .entry("BTS_ARI_USERNAME".into())
                .or_insert_with(|| "bts".into());
            if !values.contains_key("BTS_CORE_URL") {
                values.insert(
                    "BTS_CORE_URL".into(),
                    resolve_core_http(cli, local_core_selected)?,
                );
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
    };
    config::write_secure(&path, &values)?;
    secure_config_ownership(cli, &path, component)
}

fn migrate_legacy_configuration(cli: &Cli, installed: &BTreeSet<Component>) -> Result<()> {
    let Some(migration) = config::plan_legacy_environment_migration(&cli.root, installed)? else {
        return Ok(());
    };
    for (component, values) in migration {
        let path = rooted(&cli.root, &format!("/etc/bts/{}", component.config_name()));
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
    let path = rooted(&cli.root, &format!("/etc/bts/{}", component.config_name()));
    let existing = fs::read_to_string(&path)
        .ok()
        .and_then(|value| config::parse_environment(&value).ok())
        .unwrap_or_default();
    let values = match component {
        Component::Display => resolve_display_configuration(cli, existing, None)?,
        Component::Telephony => {
            let mut values = if let Some(input) = &cli.secret_input {
                config::read_secret_input(input)?
            } else {
                ensure!(
                    io::stdin().is_terminal(),
                    "Non-interactive Telephony configuration requires --secret-file or --secret-fd."
                );
                let url = prompt(
                    "ARI URL",
                    existing
                        .get("BTS_ARI_URL")
                        .map(String::as_str)
                        .unwrap_or("http://localhost:8088"),
                )?;
                let user = prompt(
                    "ARI username",
                    existing
                        .get("BTS_ARI_USERNAME")
                        .map(String::as_str)
                        .unwrap_or("bts"),
                )?;
                let core_url = match &cli.core_http_url {
                    Some(value) => value.clone(),
                    None => prompt(
                        "Core HTTP URL",
                        existing
                            .get("BTS_CORE_URL")
                            .map(String::as_str)
                            .unwrap_or("http://127.0.0.1:3100"),
                    )?,
                };
                let password = rpassword::prompt_password("ARI password: ")?;
                let confirmation = rpassword::prompt_password("Confirm ARI password: ")?;
                ensure!(password == confirmation, "ARI passwords did not match.");
                BTreeMap::from([
                    ("BTS_ARI_URL".into(), url),
                    ("BTS_ARI_USERNAME".into(), user),
                    ("BTS_ARI_PASSWORD".into(), password),
                    ("BTS_CORE_URL".into(), core_url),
                ])
            };
            values
                .entry("BTS_ARI_URL".into())
                .or_insert_with(|| "http://localhost:8088".into());
            values
                .entry("BTS_ARI_USERNAME".into())
                .or_insert_with(|| "bts".into());
            values.entry("BTS_CORE_URL".into()).or_insert_with(|| {
                cli.core_http_url
                    .clone()
                    .unwrap_or_else(|| "http://127.0.0.1:3100".into())
            });
            config::validate_telephony(&values)?;
            config::validate_http_url(values.get("BTS_CORE_URL").unwrap(), "BTS_CORE_URL")?;
            verify_ari(&values).await?;
            values
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
    };
    if cli.dry_run {
        if !cli.quiet {
            println!(
                "Would write {} configuration.\n{}",
                component,
                config::redact(&config::render_environment(&values))
            );
        }
        return Ok(());
    }
    config::write_secure(&path, &values)?;
    secure_config_ownership(cli, &path, component)?;
    if cli.root == Path::new("/") {
        systemctl(
            &mut RealSystem,
            &cli.root,
            "try-restart",
            &[component.unit()],
        )?;
    }
    if !cli.quiet {
        println!("Updated {component} configuration at {}.", path.display());
    }
    Ok(())
}

async fn verify_ari(values: &BTreeMap<String, String>) -> Result<()> {
    let url = values.get("BTS_ARI_URL").unwrap();
    let user = values.get("BTS_ARI_USERNAME").unwrap();
    let password = values.get("BTS_ARI_PASSWORD").unwrap();
    let endpoint = format!("{}/ari/api-docs/resources.json", url.trim_end_matches('/'));
    match reqwest::Client::new()
        .get(endpoint)
        .basic_auth(user, Some(password))
        .send()
        .await
    {
        Ok(response)
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                || response.status() == reqwest::StatusCode::FORBIDDEN =>
        {
            bail!(
                "ARI authentication was rejected. Check the username and password, then run 'bts-install configure telephony' again."
            )
        }
        Ok(response) if response.status().is_server_error() => bail!(
            "ARI endpoint returned {}. Check Asterisk ARI configuration.",
            response.status()
        ),
        Ok(_) => Ok(()),
        Err(error) if error.is_connect() || error.is_timeout() => bail!(
            "ARI endpoint is unreachable. Check Asterisk and the configured address; configuration was not changed."
        ),
        Err(error) => bail!("ARI configuration could not be verified: {error}"),
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
                    "Re-run installation with a valid --repository and --channel.".into(),
                ),
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
                severity: diagnostics::Severity::Error,
                message: error.to_string(),
                suggested_action: Some("Run: sudo bts-install configure display".into()),
            }),
        }
    }

    if state.installed_components.contains(&Component::Telephony) {
        let result =
            read_component_configuration(&cli.root, Component::Telephony).and_then(|values| {
                config::validate_telephony(&values)?;
                Ok(values)
            });
        match result {
            Ok(values) => match verify_ari(&values).await {
                Ok(()) => report.diagnostics.push(diagnostics::Diagnostic {
                    component: Some(Component::Telephony),
                    severity: diagnostics::Severity::Ok,
                    message: "ARI endpoint and credentials were verified.".into(),
                    suggested_action: None,
                }),
                Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                    component: Some(Component::Telephony),
                    severity: diagnostics::Severity::Error,
                    message: error.to_string(),
                    suggested_action: Some("Run: sudo bts-install configure telephony".into()),
                }),
            },
            Err(error) => report.diagnostics.push(diagnostics::Diagnostic {
                component: Some(Component::Telephony),
                severity: diagnostics::Severity::Error,
                message: error.to_string(),
                suggested_action: Some("Run: sudo bts-install configure telephony".into()),
            }),
        }
    }
    report.healthy = !report
        .diagnostics
        .iter()
        .any(|item| item.severity == diagnostics::Severity::Error);
}

fn read_component_configuration(
    root: &Path,
    component: Component,
) -> Result<BTreeMap<String, String>> {
    let path = rooted(root, &format!("/etc/bts/{}", component.config_name()));
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
    fs::remove_file(rooted(
        &cli.root,
        &format!("/usr/lib/systemd/system/{}", component.unit()),
    ))
    .ok();
    if purge {
        fs::remove_file(rooted(
            &cli.root,
            &format!("/etc/bts/{}", component.config_name()),
        ))
        .ok();
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
        println!(
            "Plan: {}",
            if plan.actions.is_empty() {
                "no changes are required".to_owned()
            } else {
                plan.actions
                    .iter()
                    .map(|action| format!("\n  - {action:?}"))
                    .collect::<String>()
            }
        );
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
    println!(
        "BTS {} (installer {})",
        report
            .installed_version
            .as_deref()
            .unwrap_or("not installed"),
        report.installer_version
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
    for item in &report.diagnostics {
        println!(
            "{} {}",
            match item.severity {
                diagnostics::Severity::Ok => "✓",
                diagnostics::Severity::Warning => "!",
                diagnostics::Severity::Error => "✗",
            },
            item.message
        );
        if let Some(action) = &item.suggested_action {
            println!("  {action}");
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
