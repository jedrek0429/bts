//! Thin command-line policy over the typed `bts-sdk` administrative API.

pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod output;

use bts_sdk::{CoreApi, CoreApiConfiguration};
use cli::Cli;
use config::{Environment, ResolvedConfiguration};
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
