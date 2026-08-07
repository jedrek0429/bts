use std::{error::Error, fmt, io};

use bts_sdk::SdkError;
use serde_json::{Value, json};

use crate::config::ConfigurationError;

#[derive(Debug)]
pub enum CliError {
    Configuration(ConfigurationError),
    Confirmation(String),
    Sdk(SdkError),
    Output(io::Error),
}

impl CliError {
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Configuration(_) | Self::Confirmation(_) | Self::Output(_) => 2,
            Self::Sdk(error) => match error {
                SdkError::Configuration(_) | SdkError::InvalidRequest(_) => 2,
                SdkError::Transport(_) | SdkError::Timeout { .. } => 3,
                SdkError::IncompatibleApi { .. }
                | SdkError::IncompatibleApiResponse(_)
                | SdkError::MalformedResponse { .. } => 4,
                SdkError::NotFound(_) => 5,
                SdkError::AmbiguousReference(_) => 6,
                SdkError::Conflict(_) | SdkError::Rejected(_) => 7,
                SdkError::ServerFailure(_) => 8,
            },
        }
    }

    pub fn concise_message(&self) -> String {
        match self {
            Self::Configuration(error) => error.to_string(),
            Self::Confirmation(message) => message.clone(),
            Self::Output(_) => "could not write command output".to_owned(),
            Self::Sdk(error) => match error {
                SdkError::Configuration(error) => error.to_string(),
                SdkError::Transport(_) => "Core is unavailable".to_owned(),
                SdkError::Timeout { .. } => "Core request timed out".to_owned(),
                SdkError::IncompatibleApi { .. } | SdkError::IncompatibleApiResponse(_) => {
                    "Core uses an incompatible administrative API".to_owned()
                }
                SdkError::MalformedResponse { .. } => {
                    "Core returned an invalid administrative response".to_owned()
                }
                SdkError::InvalidRequest(error)
                | SdkError::NotFound(error)
                | SdkError::AmbiguousReference(error)
                | SdkError::Conflict(error)
                | SdkError::Rejected(error)
                | SdkError::ServerFailure(error) => error.message.clone(),
            },
        }
    }

    pub fn json_error(&self) -> Value {
        if let Self::Sdk(error) = self
            && let Some(error) = error.administrative_error()
        {
            return json!({ "error": error });
        }
        let (category, code) = match self {
            Self::Configuration(_) | Self::Sdk(SdkError::Configuration(_)) => {
                ("invalid_input", "invalid_configuration")
            }
            Self::Confirmation(_) => ("invalid_input", "invalid_usage"),
            Self::Output(_) => ("invalid_input", "output_failure"),
            Self::Sdk(SdkError::Transport(_)) => ("unavailable", "core_unavailable"),
            Self::Sdk(SdkError::Timeout { .. }) => ("unavailable", "core_timeout"),
            Self::Sdk(SdkError::IncompatibleApi { .. }) => {
                ("incompatible_api", "unsupported_administrative_api")
            }
            Self::Sdk(SdkError::MalformedResponse { .. }) => {
                ("incompatible_api", "malformed_response")
            }
            Self::Sdk(SdkError::InvalidRequest(_))
            | Self::Sdk(SdkError::NotFound(_))
            | Self::Sdk(SdkError::AmbiguousReference(_))
            | Self::Sdk(SdkError::Conflict(_))
            | Self::Sdk(SdkError::Rejected(_))
            | Self::Sdk(SdkError::IncompatibleApiResponse(_))
            | Self::Sdk(SdkError::ServerFailure(_)) => {
                unreachable!("structured administrative errors returned above")
            }
        };
        json!({
            "error": {
                "category": category,
                "code": code,
                "message": self.concise_message()
            }
        })
    }

    pub fn verbose_detail(&self) -> Option<String> {
        match self {
            Self::Sdk(error) if error.administrative_error().is_none() => {
                let detail = error.to_string();
                (detail != self.concise_message()).then_some(detail)
            }
            Self::Output(error) => Some(error.to_string()),
            _ => None,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.concise_message().fmt(formatter)
    }
}

impl Error for CliError {}
