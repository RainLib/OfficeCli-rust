use clap::Args;
use handler_common::HandlerError;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Read or update the small per-user OfficeCLI compatibility configuration.
#[derive(Args)]
pub struct ConfigCommand {
    /// Configuration key: autoUpdate or log
    pub key: String,
    /// New value; omit to read the current value. `log clear` clears the log file.
    pub value: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppConfig {
    #[serde(default = "default_auto_update")]
    auto_update: bool,
    #[serde(default)]
    log: bool,
    #[serde(default)]
    last_update_check: Option<String>,
    #[serde(default)]
    latest_version: Option<String>,
    #[serde(default)]
    installed_binary_version: Option<String>,
    #[serde(default)]
    last_skill_refresh_version: Option<String>,
}

const fn default_auto_update() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            auto_update: true,
            log: false,
            last_update_check: None,
            latest_version: None,
            installed_binary_version: None,
            last_skill_refresh_version: None,
        }
    }
}

pub fn handle_config(command: ConfigCommand) -> Result<String, HandlerError> {
    let key = command.key.to_ascii_lowercase();
    if key == "log"
        && command
            .value
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("clear"))
    {
        std::fs::remove_file(config_dir().join("officecli.log")).ok();
        return Ok("Log cleared.".to_string());
    }
    let mut config = load_config()?;
    match command.value {
        None => match key.as_str() {
            "autoupdate" => Ok(config.auto_update.to_string()),
            "log" => Ok(config.log.to_string()),
            _ => Err(unknown_key(&command.key)),
        },
        Some(value) => {
            match key.as_str() {
                "autoupdate" => config.auto_update = is_truthy(&value),
                "log" => config.log = is_truthy(&value),
                _ => return Err(unknown_key(&command.key)),
            }
            save_config(&config)?;
            Ok(format!("{} = {}", command.key, value))
        }
    }
}

fn unknown_key(key: &str) -> HandlerError {
    HandlerError::InvalidArgument(format!(
        "Unknown config key: {}. Available: autoUpdate, log, log clear",
        key
    ))
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on" | "y"
    )
}

fn config_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".officecli")
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn load_config() -> Result<AppConfig, HandlerError> {
    let path = config_path();
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let source = std::fs::read_to_string(&path).map_err(|error| {
        HandlerError::OperationFailed(format!("reading {}: {}", path.display(), error))
    })?;
    Ok(serde_json::from_str(&source).unwrap_or_else(|_| AppConfig::default()))
}

fn save_config(config: &AppConfig) -> Result<(), HandlerError> {
    let directory = config_dir();
    std::fs::create_dir_all(&directory).map_err(|error| {
        HandlerError::OperationFailed(format!("creating {}: {}", directory.display(), error))
    })?;
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    std::fs::write(config_path(), content)
        .map_err(|error| HandlerError::OperationFailed(format!("saving config: {}", error)))
}
