use bts_sdk::{CoreApi, SdkError, SetAddonEnabledRequest};

use crate::{cli::AddonCommand, commands::CommandResult};

pub async fn execute(api: &CoreApi, command: &AddonCommand) -> Result<CommandResult, SdkError> {
    match command {
        AddonCommand::List => api.addons().await.map(CommandResult::AddonList),
        AddonCommand::Show { addon } => api.addon(addon).await.map(CommandResult::Addon),
        AddonCommand::Enable { addon } => api
            .set_addon_enabled(addon, &SetAddonEnabledRequest { enabled: true })
            .await
            .map(CommandResult::AddonMutation),
        AddonCommand::Disable { addon } => api
            .set_addon_enabled(addon, &SetAddonEnabledRequest { enabled: false })
            .await
            .map(CommandResult::AddonMutation),
    }
}
