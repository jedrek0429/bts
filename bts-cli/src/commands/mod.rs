mod group;
mod state;
mod status;
mod terminal;

use bts_sdk::{
    CoreApi, CoreStateResource, CoreStatusResource, DeletionResponse, GroupListResource,
    GroupResource, MutationResponse, SdkError, TerminalListResource, TerminalResource,
};

use crate::cli::Command;

pub enum CommandResult {
    Status(CoreStatusResource),
    State(CoreStateResource),
    TerminalList(TerminalListResource),
    Terminal(TerminalResource),
    TerminalMutation(MutationResponse<TerminalResource>),
    TerminalDeletion(DeletionResponse<TerminalResource>),
    GroupList(GroupListResource),
    Group(GroupResource),
    GroupMutation(MutationResponse<GroupResource>),
    GroupDeletion(DeletionResponse<GroupResource>),
}

pub async fn execute(api: &CoreApi, command: &Command) -> Result<CommandResult, SdkError> {
    match command {
        Command::Status => status::execute(api).await.map(CommandResult::Status),
        Command::State { .. } => state::execute(api).await.map(CommandResult::State),
        Command::Terminal { command } => terminal::execute(api, command).await,
        Command::Group { command } => group::execute(api, command).await,
    }
}
