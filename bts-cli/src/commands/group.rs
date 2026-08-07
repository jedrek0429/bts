use std::collections::BTreeSet;

use bts_sdk::{
    CoreApi, CreateGroupRequest, RenameGroupRequest, SdkError, UpdateGroupMembersRequest,
};

use crate::{cli::GroupCommand, commands::CommandResult};

pub async fn execute(api: &CoreApi, command: &GroupCommand) -> Result<CommandResult, SdkError> {
    match command {
        GroupCommand::List => api.groups().await.map(CommandResult::GroupList),
        GroupCommand::Show { group } => api.group(group).await.map(CommandResult::Group),
        GroupCommand::Create { id, name } => api
            .create_group(&CreateGroupRequest {
                id: id.clone(),
                name: name.clone(),
            })
            .await
            .map(CommandResult::Group),
        GroupCommand::Rename { group, name } => api
            .rename_group(group, &RenameGroupRequest { name: name.clone() })
            .await
            .map(CommandResult::GroupMutation),
        GroupCommand::Add { group, terminals } => api
            .update_group_members(
                group,
                &UpdateGroupMembersRequest {
                    add: terminals.iter().cloned().collect(),
                    remove: BTreeSet::new(),
                },
            )
            .await
            .map(CommandResult::GroupMutation),
        GroupCommand::Remove { group, terminals } => api
            .update_group_members(
                group,
                &UpdateGroupMembersRequest {
                    add: BTreeSet::new(),
                    remove: terminals.iter().cloned().collect(),
                },
            )
            .await
            .map(CommandResult::GroupMutation),
        GroupCommand::Delete { group } => api
            .delete_group(group)
            .await
            .map(CommandResult::GroupDeletion),
    }
}
