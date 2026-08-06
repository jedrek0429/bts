use std::{collections::BTreeSet, error::Error, fmt, time::Duration};

use bts_protocol::{AdministrativeError, AdministrativeErrorCategory};

/// Typed SDK failures without command-line presentation policy.
#[derive(Debug)]
pub enum SdkError {
    Configuration(super::ConfigurationError),
    Transport(reqwest::Error),
    Timeout {
        timeout: Duration,
        source: reqwest::Error,
    },
    IncompatibleApi {
        core_product: String,
        sdk_supported: BTreeSet<u16>,
        core_current: u16,
        core_supported: BTreeSet<u16>,
    },
    IncompatibleApiResponse(AdministrativeError),
    MalformedResponse {
        status: Option<u16>,
        detail: String,
    },
    InvalidRequest(AdministrativeError),
    NotFound(AdministrativeError),
    AmbiguousReference(AdministrativeError),
    Conflict(AdministrativeError),
    Rejected(AdministrativeError),
    ServerFailure(AdministrativeError),
}

impl SdkError {
    pub(crate) fn from_administrative(error: AdministrativeError) -> Self {
        match error.category {
            AdministrativeErrorCategory::InvalidInput => Self::InvalidRequest(error),
            AdministrativeErrorCategory::NotFound => Self::NotFound(error),
            AdministrativeErrorCategory::AmbiguousReference => Self::AmbiguousReference(error),
            AdministrativeErrorCategory::Conflict => Self::Conflict(error),
            AdministrativeErrorCategory::Rejected => Self::Rejected(error),
            AdministrativeErrorCategory::IncompatibleApi => Self::IncompatibleApiResponse(error),
            AdministrativeErrorCategory::ServerFailure => Self::ServerFailure(error),
        }
    }

    pub fn administrative_error(&self) -> Option<&AdministrativeError> {
        match self {
            Self::InvalidRequest(error)
            | Self::NotFound(error)
            | Self::AmbiguousReference(error)
            | Self::Conflict(error)
            | Self::Rejected(error)
            | Self::IncompatibleApiResponse(error)
            | Self::ServerFailure(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for SdkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(error) => error.fmt(formatter),
            Self::Transport(error) => write!(formatter, "Core transport failed: {error}"),
            Self::Timeout { timeout, .. } => {
                write!(formatter, "Core request exceeded {timeout:?}")
            }
            Self::IncompatibleApi {
                core_product,
                sdk_supported,
                core_current,
                core_supported,
            } => write!(
                formatter,
                "administrative API is incompatible: expected bts-core, received {core_product:?}; SDK supports {sdk_supported:?}, while the server's current version is {core_current} and it supports {core_supported:?}"
            ),
            Self::MalformedResponse {
                status: Some(status),
                detail,
            } => write!(
                formatter,
                "Core returned malformed HTTP {status} content: {detail}"
            ),
            Self::MalformedResponse {
                status: None,
                detail,
            } => write!(formatter, "Core returned malformed API metadata: {detail}"),
            Self::InvalidRequest(error)
            | Self::NotFound(error)
            | Self::AmbiguousReference(error)
            | Self::Conflict(error)
            | Self::Rejected(error)
            | Self::IncompatibleApiResponse(error)
            | Self::ServerFailure(error) => error.message.fmt(formatter),
        }
    }
}

impl Error for SdkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Configuration(error) => Some(error),
            Self::Transport(error) | Self::Timeout { source: error, .. } => Some(error),
            _ => None,
        }
    }
}

impl From<super::ConfigurationError> for SdkError {
    fn from(error: super::ConfigurationError) -> Self {
        Self::Configuration(error)
    }
}
