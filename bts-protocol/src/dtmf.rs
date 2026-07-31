//! Platform-reserved DTMF controls and addon menu keys.

use std::{error::Error, fmt};

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservedDtmfAction {
    SessionConfiguration,
    CancelOrBack,
    ConfirmOrCompleteInput,
}

impl ReservedDtmfAction {
    pub const fn digit(self) -> char {
        match self {
            Self::SessionConfiguration => '0',
            Self::CancelOrBack => '*',
            Self::ConfirmOrCompleteInput => '#',
        }
    }

    pub const fn from_digit(digit: char) -> Option<Self> {
        match digit {
            '0' => Some(Self::SessionConfiguration),
            '*' => Some(Self::CancelOrBack),
            '#' => Some(Self::ConfirmOrCompleteInput),
            _ => None,
        }
    }
}

/// A DTMF digit which may be bound by an addon menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DtmfMenuKey(char);

impl DtmfMenuKey {
    pub fn new(digit: char) -> Result<Self, DtmfMenuKeyError> {
        if let Some(action) = ReservedDtmfAction::from_digit(digit) {
            Err(DtmfMenuKeyError::Reserved { digit, action })
        } else if digit.is_ascii_digit() {
            Ok(Self(digit))
        } else {
            Err(DtmfMenuKeyError::Invalid { digit })
        }
    }

    pub const fn digit(self) -> char {
        self.0
    }
}

impl fmt::Display for DtmfMenuKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl TryFrom<char> for DtmfMenuKey {
    type Error = DtmfMenuKeyError;

    fn try_from(value: char) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for DtmfMenuKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let digit = char::deserialize(deserializer)?;
        Self::new(digit).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DtmfMenuKeyError {
    Reserved {
        digit: char,
        action: ReservedDtmfAction,
    },
    Invalid {
        digit: char,
    },
}

impl fmt::Display for DtmfMenuKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reserved { digit, action } => write!(
                formatter,
                "DTMF key {digit:?} is reserved for the platform action {action:?}"
            ),
            Self::Invalid { digit } => write!(formatter, "{digit:?} is not a DTMF menu key"),
        }
    }
}

impl Error for DtmfMenuKeyError {}
