use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, ensure};

use crate::cli::SecretInput;

pub fn validate_websocket_url(value: &str) -> Result<()> {
    validate_websocket_url_for_path(value, bts_compat::CORE_EVENTS_WEBSOCKET_PATH)
}

pub fn validate_terminal_websocket_url(value: &str) -> Result<()> {
    validate_websocket_url_for_path(value, bts_compat::CORE_TERMINALS_WEBSOCKET_PATH)?;
    let path = value.split(['?', '#']).next().unwrap_or(value);
    ensure!(
        path.ends_with(bts_compat::CORE_TERMINALS_WEBSOCKET_PATH),
        "Core URL must identify the published {} endpoint.",
        bts_compat::CORE_TERMINALS_WEBSOCKET_PATH
    );
    Ok(())
}

fn validate_websocket_url_for_path(value: &str, endpoint: &str) -> Result<()> {
    ensure!(
        value.starts_with("ws://") || value.starts_with("wss://"),
        "Core URL must use ws:// or wss://."
    );
    ensure!(
        value.contains(endpoint),
        "Core URL must identify the published {} endpoint.",
        endpoint
    );
    ensure!(
        !value.chars().any(char::is_whitespace),
        "Core URL must not contain whitespace."
    );
    Ok(())
}

pub fn validate_display(values: &BTreeMap<String, String>) -> Result<()> {
    validate_terminal_websocket_url(
        values
            .get("BTS_CORE_WS_URL")
            .context("BTS_CORE_WS_URL is not configured")?,
    )?;
    bts_protocol::TerminalId::new(
        values
            .get("BTS_TERMINAL_ID")
            .context("BTS_TERMINAL_ID is not configured")?,
    )
    .context("BTS_TERMINAL_ID is invalid")?;
    bts_protocol::TerminalName::new(
        values
            .get("BTS_TERMINAL_NAME")
            .context("BTS_TERMINAL_NAME is not configured")?,
    )
    .context("BTS_TERMINAL_NAME is invalid")?;
    Ok(())
}

pub fn validate_http_url(value: &str, name: &str) -> Result<()> {
    ensure!(
        value.starts_with("http://") || value.starts_with("https://"),
        "{name} must use http:// or https://."
    );
    ensure!(
        !value.chars().any(char::is_whitespace),
        "{name} must not contain whitespace."
    );
    Ok(())
}

pub fn validate_cage_args(value: &str) -> Result<()> {
    ensure!(
        !value.chars().any(char::is_control),
        "Cage arguments must not contain control characters."
    );
    ensure!(
        !value
            .split_whitespace()
            .any(|word| word.trim_matches(['\'', '"']) == "--"),
        "Cage arguments must not contain the '--' command delimiter."
    );
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
        }
    }
    ensure!(
        !escaped && quote.is_none(),
        "Cage arguments contain an incomplete escape or quote."
    );
    Ok(())
}

pub fn parse_environment(contents: &str) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .with_context(|| format!("Configuration line {} is missing '='.", index + 1))?;
        ensure!(
            !key.is_empty()
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'),
            "Configuration key '{key}' is invalid."
        );
        ensure!(
            !value.contains('\0') && !value.contains('\n'),
            "Configuration value for {key} is invalid."
        );
        values.insert(key.into(), parse_value(value)?);
    }
    Ok(values)
}

pub fn render_environment(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(key, value)| {
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            format!("{key}=\"{escaped}\"\n")
        })
        .collect()
}

fn parse_value(value: &str) -> Result<String> {
    if !(value.starts_with('"') || value.ends_with('"')) {
        return Ok(value.into());
    }
    ensure!(
        value.len() >= 2 && value.starts_with('"') && value.ends_with('"'),
        "Configuration contains an unterminated quoted value."
    );
    let mut output = String::new();
    let mut escaped = false;
    for character in value[1..value.len() - 1].chars() {
        if escaped {
            output.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            output.push(character);
        }
    }
    ensure!(!escaped, "Configuration contains an incomplete escape.");
    Ok(output)
}

pub fn write_secure(path: &Path, values: &BTreeMap<String, String>) -> Result<()> {
    let parent = path.parent().context("Configuration path has no parent")?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755))?;
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o640)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o640))?;
        file.write_all(render_environment(values).as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn read_secret_input(input: &SecretInput) -> Result<BTreeMap<String, String>> {
    let mut contents = String::new();
    match input {
        SecretInput::File(path) => {
            let metadata = fs::metadata(path)
                .with_context(|| format!("Could not inspect {}", path.display()))?;
            ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "Secret file {} must not be accessible to group or other users.",
                path.display()
            );
            fs::File::open(path)?.read_to_string(&mut contents)?;
        }
        SecretInput::FileDescriptor(fd) => {
            ensure!(*fd >= 0, "Secret file descriptor must be non-negative.");
            let path = format!("/proc/self/fd/{fd}");
            fs::File::open(path)?.read_to_string(&mut contents)?;
        }
    }
    let values = parse_environment(&contents)?;
    ensure!(
        values.contains_key("BTS_ARI_PASSWORD"),
        "Secure input must contain BTS_ARI_PASSWORD."
    );
    Ok(values)
}

pub fn redact(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.starts_with("BTS_ARI_PASSWORD=") {
                "BTS_ARI_PASSWORD=[REDACTED]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn validate_telephony(values: &BTreeMap<String, String>) -> Result<()> {
    let url = values
        .get("BTS_ARI_URL")
        .context("BTS_ARI_URL is not configured")?;
    validate_http_url(url, "BTS_ARI_URL")?;
    ensure!(
        values
            .get("BTS_ARI_USERNAME")
            .is_some_and(|value| !value.is_empty()),
        "BTS_ARI_USERNAME is not configured."
    );
    ensure!(
        values
            .get("BTS_ARI_PASSWORD")
            .is_some_and(|value| !value.is_empty() && value != "CHANGE_ME"),
        "BTS_ARI_PASSWORD is not configured."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn secure_configuration_has_restrictive_permissions_and_redaction() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("telephony.env");
        let values = BTreeMap::from([("BTS_ARI_PASSWORD".into(), "secret".into())]);
        write_secure(&path, &values).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(!redact(&fs::read_to_string(path).unwrap()).contains("secret"));
    }

    #[test]
    fn validates_display_and_telephony_configuration() {
        assert!(validate_websocket_url("ws://core:3100/api/v1/events/ws").is_ok());
        assert!(validate_terminal_websocket_url("ws://core:3100/api/v1/terminals/ws").is_ok());
        assert!(validate_terminal_websocket_url("ws://core:3100/api/v1/events/ws").is_err());
        assert!(
            validate_terminal_websocket_url("ws://core:3100/api/v1/terminals/ws/other").is_err()
        );
        assert!(validate_websocket_url("http://core").is_err());
        validate_display(&BTreeMap::from([
            (
                "BTS_CORE_WS_URL".into(),
                "ws://core:3100/api/v1/terminals/ws".into(),
            ),
            ("BTS_TERMINAL_ID".into(), "bedroom-display".into()),
            ("BTS_TERMINAL_NAME".into(), "Bedroom".into()),
        ]))
        .unwrap();
        let values = BTreeMap::from([
            ("BTS_ARI_URL".into(), "http://asterisk:8088".into()),
            ("BTS_ARI_USERNAME".into(), "bts".into()),
            ("BTS_ARI_PASSWORD".into(), "secret".into()),
        ]);
        validate_telephony(&values).unwrap();
        assert!(validate_cage_args("-m extend -- bts-display").is_err());
        validate_cage_args("-m extend -s").unwrap();
        assert!(validate_cage_args("-m 'unterminated").is_err());
    }

    #[test]
    fn environment_round_trip_preserves_secret_characters() {
        let values = BTreeMap::from([(
            "BTS_ARI_PASSWORD".into(),
            "spaces # quotes \" and \\slashes".into(),
        )]);
        assert_eq!(
            parse_environment(&render_environment(&values)).unwrap(),
            values
        );
    }

    #[test]
    fn protected_secret_file_is_accepted_and_exposed_file_is_rejected() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("secrets.env");
        fs::write(&path, "BTS_ARI_PASSWORD=secret\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let values = read_secret_input(&SecretInput::File(path.clone())).unwrap();
        assert_eq!(values.get("BTS_ARI_PASSWORD").unwrap(), "secret");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_secret_input(&SecretInput::File(path)).is_err());
    }
}
