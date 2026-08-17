use std::{
    ffi::OsString,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{Context, Result, ensure};
use bts_install::{
    INSTALLER_VERSION, LOCAL_RELEASE_CHANNEL,
    cli::{Cli, Command},
    release::ReleaseClient,
    self_update::{SelfUpdateOutcome, target_is_newer, update_from_manifest},
    state::InstallerState,
};

fn main() {
    if let Err(error) = bootstrap() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

fn bootstrap() -> Result<()> {
    let cli = Cli::parse(std::env::args())?;

    if matches!(cli.command, Command::Upgrade(_)) && cli.release_dir.is_none() {
        normalise_upgrade_source(&cli)?;
    }

    match cli.command {
        Command::SelfUpdate => run_self_update(&cli),
        Command::Install { .. } | Command::Add(_) | Command::Upgrade(_) => {
            preflight_release_operation(&cli)?;
            legacy::invoke();
            Ok(())
        }
        _ => {
            legacy::invoke();
            Ok(())
        }
    }
}

fn run_self_update(cli: &Cli) -> Result<()> {
    require_root()?;
    let executable = std::env::current_exe().context("Could not resolve the running installer")?;
    let runtime = tokio::runtime::Runtime::new()?;
    let client = release_client(cli)?;
    let outcome = runtime.block_on(bts_install::self_update::self_update(&client, &executable))?;

    if !cli.quiet {
        match outcome {
            SelfUpdateOutcome::Current { version } => {
                println!("bts-install {version} is already current for the selected release.");
            }
            SelfUpdateOutcome::Updated { from, to } => {
                println!("Updated bts-install from {from} to {to}.");
            }
        }
    }
    Ok(())
}

fn preflight_release_operation(cli: &Cli) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let client = release_client(cli)?;
    let (manifest, urls) = runtime.block_on(client.fetch_manifest())?;

    if cli.release_dir.is_some() || !target_is_newer(&manifest)? {
        return Ok(());
    }

    if cli.dry_run {
        if !cli.quiet {
            println!(
                "Preflight: target release {} contains a newer bts-install than {}. The installer would update itself before applying host changes.",
                manifest.release_version, INSTALLER_VERSION
            );
        }
        return Ok(());
    }

    require_root()?;
    let executable = std::env::current_exe().context("Could not resolve the running installer")?;
    let outcome = runtime.block_on(update_from_manifest(&client, &manifest, &urls, &executable))?;
    if outcome.changed() {
        if !cli.quiet
            && let SelfUpdateOutcome::Updated { from, to } = outcome
        {
            println!(
                "Updated bts-install from {from} to {to} before continuing with the requested operation."
            );
        }
        reexec_current(&[])?;
    }
    Ok(())
}

fn normalise_upgrade_source(cli: &Cli) -> Result<()> {
    if cli.repository_selected && cli.channel_selected {
        return Ok(());
    }
    let state_path = rooted(&cli.root, "/var/lib/bts-install/state.json");
    let state =
        InstallerState::load(&state_path)?.context("No managed BTS installation exists.")?;

    if state.release_channel == LOCAL_RELEASE_CHANNEL {
        ensure!(
            cli.release_dir.is_some(),
            "This installation uses a local release source. Supply --release-dir explicitly for upgrade."
        );
        return Ok(());
    }

    let mut additional = Vec::new();
    if !cli.repository_selected {
        additional.push(OsString::from("--repository"));
        additional.push(OsString::from(&state.repository));
    }
    if !cli.channel_selected {
        additional.push(OsString::from(
            if state.release_pinned || state.release_channel.starts_with('v') {
                "--release"
            } else {
                "--track"
            },
        ));
        additional.push(OsString::from(&state.release_channel));
    }
    if !additional.is_empty() {
        reexec_current(&additional)?;
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

fn reexec_current(additional: &[OsString]) -> Result<()> {
    let executable = std::env::current_exe().context("Could not resolve the running installer")?;
    let mut command = ProcessCommand::new(executable);
    command.args(std::env::args_os().skip(1));
    command.args(additional);
    let error = command.exec();
    Err(error).context("Could not re-execute bts-install")
}

fn require_root() -> Result<()> {
    ensure!(unsafe { libc::geteuid() } == 0, "Run bts-install as root.");
    Ok(())
}

fn rooted(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}

mod legacy {
    pub(super) fn invoke() {
        main();
    }

    include!("main.rs");
}
