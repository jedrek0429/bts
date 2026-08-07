use clap::{ArgAction, Parser, Subcommand};

use crate::config::{ColourMode, OutputMode};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "btscli",
    version,
    about = "Administer BTS Core resources",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[arg(long, global = true, value_name = "URL")]
    pub core: Option<String>,

    #[arg(long, global = true, value_enum)]
    pub output: Option<OutputMode>,

    #[arg(long, global = true, value_name = "DURATION")]
    pub timeout: Option<String>,

    #[arg(long, global = true)]
    pub quiet: bool,

    #[arg(
        short = 'v',
        global = true,
        action = ArgAction::Count,
        conflicts_with = "quiet"
    )]
    pub verbosity: u8,

    #[arg(long, global = true, value_enum)]
    pub colour: Option<ColourMode>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Report Core availability and compatibility.
    Status,
    /// Inspect Core state.
    State {
        #[command(subcommand)]
        command: StateCommand,
    },
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::State {
                command: StateCommand::Show,
            } => "state",
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub enum StateCommand {
    /// Show the current Core state.
    Show,
}
