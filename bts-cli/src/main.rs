use std::{ffi::OsString, io::IsTerminal, process::ExitCode};

use bts_cli::{cli::Cli, config::Environment, error::CliError, output::OutputStreams};
use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let cli = match Cli::try_parse_from(&arguments) {
        Ok(cli) => cli,
        Err(error) => {
            let exit_code = error.exit_code();
            if exit_code == 2 && requests_json(&arguments) {
                let document = serde_json::json!({
                    "error": {
                        "category": "invalid_input",
                        "code": "invalid_usage",
                        "message": "Invalid command-line arguments"
                    }
                });
                eprintln!("{document}");
            } else {
                let _ = error.print();
            }
            return ExitCode::from(u8::try_from(exit_code).unwrap_or(2));
        }
    };
    let environment = match Environment::from_process() {
        Ok(environment) => environment,
        Err(error) => {
            let error = CliError::Configuration(error);
            eprintln!("Error: {error}");
            return ExitCode::from(error.exit_code());
        }
    };
    let stdout_is_terminal = std::io::stdout().is_terminal();
    let stderr_is_terminal = std::io::stderr().is_terminal();
    let stdin_is_terminal = std::io::stdin().is_terminal();
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let streams = OutputStreams {
        stdin: &mut stdin,
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin_is_terminal,
        stdout_is_terminal,
        stderr_is_terminal,
    };
    ExitCode::from(bts_cli::execute(cli, &environment, streams).await)
}

fn requests_json(arguments: &[OsString]) -> bool {
    let mut explicit = None;
    let mut index = 0;
    while index < arguments.len() {
        let Some(argument) = arguments[index].to_str() else {
            index += 1;
            continue;
        };
        if argument == "--output" {
            explicit = arguments
                .get(index + 1)
                .and_then(|value| value.to_str())
                .map(|value| value == "json");
            index += 2;
            continue;
        }
        if let Some(value) = argument.strip_prefix("--output=") {
            explicit = Some(value == "json");
        }
        index += 1;
    }
    explicit.unwrap_or_else(|| std::env::var("BTSCLI_OUTPUT").is_ok_and(|value| value == "json"))
}
