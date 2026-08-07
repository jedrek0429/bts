use bts_sdk::{CoreApi, CoreStateResource, SdkError};

pub async fn execute(api: &CoreApi) -> Result<CoreStateResource, SdkError> {
    api.state().await
}
