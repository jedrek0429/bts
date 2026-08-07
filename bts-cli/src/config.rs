use std::{collections::BTreeMap, error::Error, fmt, time::Duration};

use clap::ValueEnum;

use crate::cli::Cli;

const DEFAULT_CORE_URL: &str = "http://127.0.0.1:3100";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const CONFIGURATION_KEYS: [&str; 5] = [
    "BTS_CORE_URL",
    "BTSCLI_OUTPUT",
    "BTSCLI_TIMEOUT",
    "BTSCLI_COLOUR",
    "NO_COLOR",
];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum OutputMode {
    #[default]
    Human,
    Json,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum ColourMode {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Default)]
pub struct Environment {
    values: BTreeMap<String, String>,
}

impl Environment {
    pub fn from_process() -> Result<Self, ConfigurationError> {
        let mut values = BTreeMap::new();
        for key in CONFIGURATION_KEYS {
            if let Some(value) = std::env::var_os(key) {
                let value = value
                    .into_string()
                    .map_err(|_| ConfigurationError::new(key, "must contain valid Unicode text"))?;
                values.insert(key.to_owned(), value);
            }
        }
        Ok(Self { values })
    }

    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfiguration {
    pub core_url: String,
    pub timeout: Duration,
    pub output: OutputMode,
    pub quiet: bool,
    pub verbosity: u8,
    pub colour: ColourMode,
}

impl ResolvedConfiguration {
    pub fn resolve(cli: &Cli, environment: &Environment) -> Result<Self, ConfigurationError> {
        let output = resolve_output(cli, environment)?;
        if cli.quiet && output == OutputMode::Json {
            return Err(ConfigurationError::new(
                "--quiet",
                "cannot be combined with JSON output",
            ));
        }

        let (core_url, core_url_field) = if let Some(value) = &cli.core {
            (value.clone(), "--core")
        } else if let Some(value) = environment.get("BTS_CORE_URL") {
            (value.to_owned(), "BTS_CORE_URL")
        } else {
            (DEFAULT_CORE_URL.to_owned(), "Core URL")
        };
        if core_url.is_empty() {
            return Err(ConfigurationError::new(core_url_field, "must not be empty"));
        }

        let timeout = match cli
            .timeout
            .as_deref()
            .or_else(|| environment.get("BTSCLI_TIMEOUT"))
        {
            Some(value) => parse_duration(value)?,
            None => DEFAULT_TIMEOUT,
        };

        let colour = resolve_colour(cli, environment, output)?;

        Ok(Self {
            core_url,
            timeout,
            output,
            quiet: cli.quiet,
            verbosity: cli.verbosity,
            colour,
        })
    }
}

pub fn resolve_colour(
    cli: &Cli,
    environment: &Environment,
    output: OutputMode,
) -> Result<ColourMode, ConfigurationError> {
    let explicit_colour = if let Some(colour) = cli.colour {
        Some(colour)
    } else if let Some(value) = environment.get("BTSCLI_COLOUR") {
        Some(parse_value::<ColourMode>("BTSCLI_COLOUR", value)?)
    } else {
        None
    };
    let colour = explicit_colour.unwrap_or_else(|| {
        if environment.contains("NO_COLOR") {
            ColourMode::Never
        } else {
            ColourMode::Auto
        }
    });
    Ok(if output == OutputMode::Json {
        ColourMode::Never
    } else {
        colour
    })
}

pub fn resolve_output(
    cli: &Cli,
    environment: &Environment,
) -> Result<OutputMode, ConfigurationError> {
    if let Some(output) = cli.output {
        Ok(output)
    } else if let Some(value) = environment.get("BTSCLI_OUTPUT") {
        parse_value("BTSCLI_OUTPUT", value)
    } else {
        Ok(OutputMode::Human)
    }
}

fn parse_value<T: ValueEnum + Copy>(
    field: &'static str,
    value: &str,
) -> Result<T, ConfigurationError> {
    T::from_str(value, false)
        .map_err(|_| ConfigurationError::new(field, format!("has invalid value {value:?}")))
}

fn parse_duration(value: &str) -> Result<Duration, ConfigurationError> {
    let (digits, multiplier) = if let Some(digits) = value.strip_suffix("ms") {
        (digits, 1_u64)
    } else if let Some(digits) = value.strip_suffix('s') {
        (digits, 1_000_u64)
    } else if let Some(digits) = value.strip_suffix('m') {
        (digits, 60_000_u64)
    } else {
        return Err(ConfigurationError::new(
            "timeout",
            "must be a positive integer followed by ms, s or m",
        ));
    };
    let amount = digits.parse::<u64>().ok().filter(|value| *value > 0);
    let milliseconds = amount.and_then(|amount| amount.checked_mul(multiplier));
    milliseconds
        .map(Duration::from_millis)
        .ok_or_else(|| ConfigurationError::new("timeout", "is outside the supported range"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationError {
    field: &'static str,
    detail: String,
}

impl ConfigurationError {
    pub fn new(field: &'static str, detail: impl Into<String>) -> Self {
        Self {
            field,
            detail: detail.into(),
        }
    }

    pub fn field(&self) -> &'static str {
        self.field
    }
}

impl fmt::Display for ConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {}", self.field, self.detail)
    }
}

impl Error for ConfigurationError {}
