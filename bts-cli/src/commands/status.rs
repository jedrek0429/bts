use bts_sdk::{CoreApi, CoreStatusResource, SdkError};

pub async fn execute(api: &CoreApi) -> Result<CoreStatusResource, SdkError> {
    api.status().await
}
