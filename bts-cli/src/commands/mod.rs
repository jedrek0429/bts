mod state;
mod status;

use bts_sdk::{CoreApi, CoreStateResource, CoreStatusResource, SdkError};

use crate::cli::Command;

pub enum CommandResult {
    Status(CoreStatusResource),
    State(CoreStateResource),
}

pub async fn execute(api: &CoreApi, command: &Command) -> Result<CommandResult, SdkError> {
    match command {
        Command::Status => status::execute(api).await.map(CommandResult::Status),
        Command::State { .. } => state::execute(api).await.map(CommandResult::State),
    }
}
