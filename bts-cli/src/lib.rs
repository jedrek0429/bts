//! Thin command-line policy over the typed `bts-sdk` administrative API.

pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod output;

use bts_sdk::{CoreApi, CoreApiConfiguration};
use cli::{Cli, Command, GroupCommand, TerminalCommand};
use config::{Environment, OutputMode, ResolvedConfiguration};
use error::CliError;
use output::OutputStreams;

/// Resolve configuration, execute one command and render its process result.
pub async fn execute(cli: Cli, environment: &Environment, mut streams: OutputStreams<'_>) -> u8 {
    let output_hint = config::resolve_output(&cli, environment).ok();
    let colour_hint = output_hint
        .and_then(|output| config::resolve_colour(&cli, environment, output).ok())
        .unwrap_or(config::ColourMode::Never);
    let configuration = match ResolvedConfiguration::resolve(&cli, environment) {
        Ok(configuration) => configuration,
        Err(error) => {
            let error = CliError::Configuration(error);
            output::write_error(
                &mut streams,
                output_hint.unwrap_or_default(),
                colour_hint,
                cli.verbosity,
                &error,
            );
            return error.exit_code();
        }
    };

    if configuration.verbosity > 0 {
        let _ = writeln!(
            streams.stderr,
            "Requesting {} from Core",
            cli.command.name()
        );
    }
    if configuration.verbosity > 1 {
        let _ = writeln!(
            streams.stderr,
            "Core: {}; timeout: {:?}",
            configuration.core_url, configuration.timeout
        );
    }

    let sdk_configuration = match CoreApiConfiguration::new(&configuration.core_url)
        .and_then(|value| value.with_request_timeout(configuration.timeout))
    {
        Ok(configuration) => configuration,
        Err(error) => {
            let error = CliError::Sdk(error.into());
            output::write_error(
                &mut streams,
                configuration.output,
                configuration.colour,
                configuration.verbosity,
                &error,
            );
            return error.exit_code();
        }
    };
    let api = match CoreApi::new(sdk_configuration) {
        Ok(api) => api,
        Err(error) => {
            let error = CliError::Sdk(error);
            output::write_error(
                &mut streams,
                configuration.output,
                configuration.colour,
                configuration.verbosity,
                &error,
            );
            return error.exit_code();
        }
    };

    if let Err(error) = confirm_destructive(&api, &cli, &configuration, &mut streams).await {
        output::write_error(
            &mut streams,
            configuration.output,
            configuration.colour,
            configuration.verbosity,
            &error,
        );
        return error.exit_code();
    }

    match commands::execute(&api, &cli.command).await {
        Ok(result) => {
            if let Err(error) = output::write_success(&mut streams, &configuration, &result) {
                let error = CliError::Output(error);
                output::write_error(
                    &mut streams,
                    configuration.output,
                    configuration.colour,
                    configuration.verbosity,
                    &error,
                );
                return error.exit_code();
            }
            0
        }
        Err(error) => {
            let error = CliError::Sdk(error);
            output::write_error(
                &mut streams,
                configuration.output,
                configuration.colour,
                configuration.verbosity,
                &error,
            );
            error.exit_code()
        }
    }
}

async fn confirm_destructive(
    api: &CoreApi,
    cli: &Cli,
    configuration: &ResolvedConfiguration,
    streams: &mut OutputStreams<'_>,
) -> Result<(), CliError> {
    let prompt = match &cli.command {
        Command::Terminal {
            command: TerminalCommand::Forget { terminal },
        } => {
            let resource = api.terminal(terminal).await.map_err(CliError::Sdk)?;
            format!(
                "Forget terminal {} ({})? It may register again later. [y/N] ",
                resource.name, resource.id
            )
        }
        Command::Group {
            command: GroupCommand::Delete { group },
        } => {
            let resource = api.group(group).await.map_err(CliError::Sdk)?;
            format!(
                "Delete terminal group {} ({})? Its terminals will not be deleted. [y/N] ",
                resource.name, resource.id
            )
        }
        _ => return Ok(()),
    };
    if cli.yes {
        return Ok(());
    }
    if !streams.stdin_is_terminal || configuration.output == OutputMode::Json || configuration.quiet
    {
        return Err(CliError::Confirmation(
            "destructive operation requires --yes when prompting is unavailable".to_owned(),
        ));
    }
    write!(streams.stderr, "{prompt}")
        .and_then(|()| streams.stderr.flush())
        .map_err(CliError::Output)?;
    let mut reply = String::new();
    streams
        .stdin
        .read_line(&mut reply)
        .map_err(CliError::Output)?;
    if matches!(reply.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CliError::Confirmation(
            "destructive operation was not confirmed".to_owned(),
        ))
    }
}
