use std::collections::BTreeSet;

use bts_sdk::{CoreApi, RenameTerminalRequest, SdkError, UpdateTerminalTagsRequest};

use crate::{
    cli::{TerminalCommand, TerminalTagCommand},
    commands::CommandResult,
};

pub async fn execute(api: &CoreApi, command: &TerminalCommand) -> Result<CommandResult, SdkError> {
    match command {
        TerminalCommand::List => api.terminals().await.map(CommandResult::TerminalList),
        TerminalCommand::Show { terminal } => {
            api.terminal(terminal).await.map(CommandResult::Terminal)
        }
        TerminalCommand::Rename { terminal, name } => api
            .rename_terminal(terminal, &RenameTerminalRequest { name: name.clone() })
            .await
            .map(CommandResult::TerminalMutation),
        TerminalCommand::Tag { command } => match command {
            TerminalTagCommand::Add { terminal, tags } => api
                .update_terminal_tags(
                    terminal,
                    &UpdateTerminalTagsRequest {
                        add: tags.iter().cloned().collect(),
                        remove: BTreeSet::new(),
                    },
                )
                .await
                .map(CommandResult::TerminalMutation),
            TerminalTagCommand::Remove { terminal, tags } => api
                .update_terminal_tags(
                    terminal,
                    &UpdateTerminalTagsRequest {
                        add: BTreeSet::new(),
                        remove: tags.iter().cloned().collect(),
                    },
                )
                .await
                .map(CommandResult::TerminalMutation),
        },
        TerminalCommand::Forget { terminal } => api
            .forget_terminal(terminal)
            .await
            .map(CommandResult::TerminalDeletion),
    }
}
