use bts_sdk::{GroupId, GroupName, GroupReference, TerminalName, TerminalReference, TerminalTag};
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

    /// Confirm destructive operations without prompting.
    #[arg(long, global = true)]
    pub yes: bool,

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
    /// Inspect or administer terminals.
    Terminal {
        #[command(subcommand)]
        command: TerminalCommand,
    },
    /// Inspect or administer terminal groups.
    Group {
        #[command(subcommand)]
        command: GroupCommand,
    },
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::State {
                command: StateCommand::Show,
            } => "state",
            Self::Terminal { .. } => "terminal",
            Self::Group { .. } => "group",
        }
    }
}

#[derive(Debug, Clone, Subcommand)]
pub enum TerminalCommand {
    /// List registered terminals.
    List,
    /// Show one registered terminal.
    Show { terminal: TerminalReference },
    /// Change a terminal's display name.
    Rename {
        terminal: TerminalReference,
        name: TerminalName,
    },
    /// Add or remove terminal tags.
    Tag {
        #[command(subcommand)]
        command: TerminalTagCommand,
    },
    /// Forget an offline terminal definition.
    Forget { terminal: TerminalReference },
}

#[derive(Debug, Clone, Subcommand)]
pub enum TerminalTagCommand {
    /// Add one or more tags.
    Add {
        terminal: TerminalReference,
        #[arg(required = true, num_args = 1..)]
        tags: Vec<TerminalTag>,
    },
    /// Remove one or more tags.
    Remove {
        terminal: TerminalReference,
        #[arg(required = true, num_args = 1..)]
        tags: Vec<TerminalTag>,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum GroupCommand {
    /// List terminal groups.
    List,
    /// Show one terminal group.
    Show { group: GroupReference },
    /// Create a terminal group.
    Create {
        id: GroupId,
        #[arg(long)]
        name: GroupName,
    },
    /// Change a group's display name.
    Rename {
        group: GroupReference,
        name: GroupName,
    },
    /// Add terminals to a group.
    Add {
        group: GroupReference,
        #[arg(required = true, num_args = 1..)]
        terminals: Vec<TerminalReference>,
    },
    /// Remove terminals from a group.
    Remove {
        group: GroupReference,
        #[arg(required = true, num_args = 1..)]
        terminals: Vec<TerminalReference>,
    },
    /// Delete a terminal group without deleting its members.
    Delete { group: GroupReference },
}

#[derive(Debug, Clone, Subcommand)]
pub enum StateCommand {
    /// Show the current Core state.
    Show,
}
