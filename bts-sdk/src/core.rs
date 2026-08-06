use std::{collections::BTreeSet, time::Duration};

use bts_protocol::{
    AdministrativeErrorResponse, ApiDiscovery, CoreStateResource, CoreStatusResource,
    core::{CORE_API_DISCOVERY_PATH, CORE_API_VERSION},
};
use reqwest::{Client, Url, header};
use semver::Version;
use serde::de::DeserializeOwned;

use crate::{CoreApiConfiguration, SdkError};

const SDK_NAME: &str = "bts-sdk";
const SDK_VERSION_HEADER: &str = "x-bts-sdk-version";
const SDK_API_VERSION_HEADER: &str = "x-bts-administrative-api-version";

/// Metadata attached to SDK HTTP requests and exposed to integrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkMetadata {
    pub name: &'static str,
    pub version: Version,
    pub supported_administrative_api_versions: BTreeSet<u16>,
}

/// Typed entry point for Core-owned administrative resources.
#[derive(Clone)]
pub struct CoreApi {
    http: Client,
    base_url: Url,
    request_timeout: Duration,
}

impl CoreApi {
    pub fn new(configuration: CoreApiConfiguration) -> Result<Self, SdkError> {
        let metadata = Self::metadata();
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            SDK_VERSION_HEADER,
            header::HeaderValue::from_str(&metadata.version.to_string()).map_err(|error| {
                SdkError::Configuration(crate::ConfigurationError::new(
                    "SDK version metadata",
                    error.to_string(),
                ))
            })?,
        );
        headers.insert(
            SDK_API_VERSION_HEADER,
            header::HeaderValue::from_str(&CORE_API_VERSION.to_string()).map_err(|error| {
                SdkError::Configuration(crate::ConfigurationError::new(
                    "SDK API version metadata",
                    error.to_string(),
                ))
            })?,
        );
        let http = Client::builder()
            .user_agent(format!("{}/{}", metadata.name, metadata.version))
            .default_headers(headers)
            .build()
            .map_err(|error| {
                SdkError::Configuration(crate::ConfigurationError::new(
                    "HTTP transport",
                    error.to_string(),
                ))
            })?;
        Ok(Self {
            http,
            base_url: configuration.base_url().clone(),
            request_timeout: configuration.request_timeout(),
        })
    }

    pub fn metadata() -> SdkMetadata {
        SdkMetadata {
            name: SDK_NAME,
            version: Version::parse(env!("CARGO_PKG_VERSION"))
                .expect("the workspace package version is valid SemVer"),
            supported_administrative_api_versions: BTreeSet::from([CORE_API_VERSION]),
        }
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub async fn discover(&self) -> Result<ApiDiscovery, SdkError> {
        let url = self.join_root_path(CORE_API_DISCOVERY_PATH)?;
        self.get(url).await
    }

    pub async fn status(&self) -> Result<CoreStatusResource, SdkError> {
        let discovery = self.discover_compatible().await?;
        let url = self.join_administrative_path(&discovery, "status")?;
        self.get(url).await
    }

    pub async fn state(&self) -> Result<CoreStateResource, SdkError> {
        let discovery = self.discover_compatible().await?;
        let url = self.join_administrative_path(&discovery, "state")?;
        self.get(url).await
    }

    /// Discovers Core and verifies that its advertised current API can be used.
    pub async fn discover_compatible(&self) -> Result<ApiDiscovery, SdkError> {
        let discovery = self.discover().await?;
        let sdk_supported = Self::metadata().supported_administrative_api_versions;
        if discovery.product != "bts-core"
            || discovery.administrative_api.current != CORE_API_VERSION
            || !discovery
                .administrative_api
                .supported
                .contains(&CORE_API_VERSION)
        {
            return Err(SdkError::IncompatibleApi {
                core_product: discovery.product,
                sdk_supported,
                core_current: discovery.administrative_api.current,
                core_supported: discovery.administrative_api.supported,
            });
        }
        Ok(discovery)
    }

    fn join_root_path(&self, path: &str) -> Result<Url, SdkError> {
        self.base_url
            .join(path.trim_start_matches('/'))
            .map_err(|error| SdkError::MalformedResponse {
                status: None,
                detail: format!("invalid API path {path:?}: {error}"),
            })
    }

    fn join_administrative_path(
        &self,
        discovery: &ApiDiscovery,
        resource: &str,
    ) -> Result<Url, SdkError> {
        let base_path = discovery.administrative_api.base_path.trim_end_matches('/');
        if !base_path.starts_with('/')
            || base_path.starts_with("//")
            || base_path.contains('?')
            || base_path.contains('#')
        {
            return Err(SdkError::MalformedResponse {
                status: None,
                detail: "discovery contains an invalid administrative base path".to_owned(),
            });
        }
        self.join_root_path(&format!("{base_path}/{resource}"))
    }

    async fn get<T: DeserializeOwned>(&self, url: Url) -> Result<T, SdkError> {
        let response = self
            .http
            .get(url)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|error| self.request_error(error))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| self.request_error(error))?;
        if status.is_success() {
            return serde_json::from_slice(&bytes).map_err(|error| SdkError::MalformedResponse {
                status: Some(status.as_u16()),
                detail: error.to_string(),
            });
        }
        let response =
            serde_json::from_slice::<AdministrativeErrorResponse>(&bytes).map_err(|error| {
                SdkError::MalformedResponse {
                    status: Some(status.as_u16()),
                    detail: error.to_string(),
                }
            })?;
        Err(SdkError::from_administrative(response.error))
    }

    fn request_error(&self, error: reqwest::Error) -> SdkError {
        if error.is_timeout() {
            SdkError::Timeout {
                timeout: self.request_timeout,
                source: error,
            }
        } else {
            SdkError::Transport(error)
        }
    }
}
