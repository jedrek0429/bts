use std::io::{self, Write};

use bts_sdk::{CoreOperationalStatus, CoreStateResource, CoreStatusResource, DisplayState};
use chrono::Local;

use crate::{
    commands::CommandResult,
    config::{ColourMode, OutputMode, ResolvedConfiguration},
    error::CliError,
};

const ANSI_BOLD: &str = "\u{1b}[1m";
const ANSI_GREEN: &str = "\u{1b}[32m";
const ANSI_RED: &str = "\u{1b}[31m";
const ANSI_RESET: &str = "\u{1b}[0m";

pub struct OutputStreams<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
    pub stdout_is_terminal: bool,
    pub stderr_is_terminal: bool,
}

pub fn write_success(
    streams: &mut OutputStreams<'_>,
    configuration: &ResolvedConfiguration,
    result: &CommandResult,
) -> io::Result<()> {
    if configuration.output == OutputMode::Json {
        match result {
            CommandResult::Status(value) => {
                serde_json::to_writer(&mut streams.stdout, value).map_err(io::Error::other)?
            }
            CommandResult::State(value) => {
                serde_json::to_writer(&mut streams.stdout, value).map_err(io::Error::other)?
            }
        }
        return writeln!(streams.stdout);
    }
    if configuration.quiet {
        return Ok(());
    }
    let colour = colour_enabled(
        configuration.colour,
        configuration.output,
        streams.stdout_is_terminal,
    );
    match result {
        CommandResult::Status(value) => write_human_status(streams.stdout, value, colour),
        CommandResult::State(value) => write_human_state(streams.stdout, value, colour),
    }
}

pub fn write_error(
    streams: &mut OutputStreams<'_>,
    output: OutputMode,
    colour: ColourMode,
    verbosity: u8,
    error: &CliError,
) {
    if output == OutputMode::Json {
        let _ = serde_json::to_writer(&mut streams.stderr, &error.json_error());
        let _ = writeln!(streams.stderr);
        return;
    }
    let colour = colour_enabled(colour, output, streams.stderr_is_terminal);
    let prefix = styled("Error:", ANSI_RED, colour);
    let _ = writeln!(streams.stderr, "{prefix} {}", error.concise_message());
    if verbosity > 0
        && let Some(detail) = error.verbose_detail()
    {
        let _ = writeln!(streams.stderr, "Detail: {detail}");
    }
}

fn write_human_status(
    output: &mut dyn Write,
    status: &CoreStatusResource,
    colour: bool,
) -> io::Result<()> {
    let heading = styled("Core status", ANSI_BOLD, colour);
    let (operational_status, status_colour) = match status.status {
        CoreOperationalStatus::Ready => ("ready", ANSI_GREEN),
        CoreOperationalStatus::Degraded => ("degraded", ANSI_RED),
    };
    let operational_status = styled(operational_status, status_colour, colour);
    writeln!(output, "{heading}: {operational_status}")?;
    writeln!(output, "Version: {}", status.product_version)?;
    writeln!(
        output,
        "Administrative API: v{}",
        status.administrative_api_version
    )?;
    writeln!(
        output,
        "Started: {}",
        status.started_at.with_timezone(&Local).to_rfc3339()
    )
}

fn write_human_state(
    output: &mut dyn Write,
    state: &CoreStateResource,
    colour: bool,
) -> io::Result<()> {
    writeln!(output, "{}", styled("Core state", ANSI_BOLD, colour))?;
    writeln!(
        output,
        "Captured: {}",
        state.captured_at.with_timezone(&Local).to_rfc3339()
    )?;
    writeln!(output, "Display: {}", display_summary(&state.state.display))?;
    match &state.state.display_lease {
        Some(lease) => writeln!(
            output,
            "Display lease: {} at priority {}",
            lease.owner, lease.priority
        )?,
        None => writeln!(output, "Display lease: none")?,
    }
    writeln!(
        output,
        "Terminals: {} registered, {} online",
        state.terminals.registered, state.terminals.online
    )?;
    writeln!(output, "Groups: {}", state.terminals.groups)
}

fn display_summary(display: &DisplayState) -> String {
    match display {
        DisplayState::Clock { time, date, .. } => format!("clock — {time}, {date}"),
        DisplayState::Weather {
            location,
            temperature,
            condition,
            ..
        } => format!("weather — {location}, {temperature}, {condition}"),
        DisplayState::Message { title, body } => format!("message — {title}: {body}"),
        DisplayState::Blank => "blank".to_owned(),
    }
}

fn colour_enabled(mode: ColourMode, output: OutputMode, is_terminal: bool) -> bool {
    if output == OutputMode::Json {
        return false;
    }
    match mode {
        ColourMode::Auto => is_terminal,
        ColourMode::Always => true,
        ColourMode::Never => false,
    }
}

fn styled(value: &str, style: &str, enabled: bool) -> String {
    if enabled {
        format!("{style}{value}{ANSI_RESET}")
    } else {
        value.to_owned()
    }
}
