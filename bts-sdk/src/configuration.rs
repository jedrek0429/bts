use std::{error::Error, fmt, time::Duration};

use reqwest::Url;

const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Explicit, process-local SDK configuration.
#[derive(Debug, Clone)]
pub struct CoreApiConfiguration {
    base_url: Url,
    request_timeout: Duration,
}

impl CoreApiConfiguration {
    pub fn new(base_url: impl AsRef<str>) -> Result<Self, ConfigurationError> {
        let mut base_url = Url::parse(base_url.as_ref())
            .map_err(|error| ConfigurationError::new("core base URL", error.to_string()))?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(ConfigurationError::new(
                "core base URL",
                "scheme must be http or https",
            ));
        }
        if base_url.host_str().is_none() {
            return Err(ConfigurationError::new(
                "core base URL",
                "a host is required",
            ));
        }
        if !base_url.username().is_empty() || base_url.password().is_some() {
            return Err(ConfigurationError::new(
                "core base URL",
                "embedded credentials are not permitted",
            ));
        }
        if base_url.query().is_some() || base_url.fragment().is_some() {
            return Err(ConfigurationError::new(
                "core base URL",
                "query strings and fragments are not permitted",
            ));
        }
        if base_url.path() != "/" {
            return Err(ConfigurationError::new(
                "core base URL",
                "a path prefix is not supported",
            ));
        }
        base_url.set_path("/");
        Ok(Self {
            base_url,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        })
    }

    pub fn with_request_timeout(
        mut self,
        request_timeout: Duration,
    ) -> Result<Self, ConfigurationError> {
        if request_timeout.is_zero() {
            return Err(ConfigurationError::new(
                "request timeout",
                "must be greater than zero",
            ));
        }
        self.request_timeout = request_timeout;
        Ok(self)
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationError {
    field: &'static str,
    detail: String,
}

impl ConfigurationError {
    pub(crate) fn new(field: &'static str, detail: impl Into<String>) -> Self {
        Self {
            field,
            detail: detail.into(),
        }
    }

    pub fn field(&self) -> &'static str {
        self.field
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {}", self.field, self.detail)
    }
}

impl Error for ConfigurationError {}
