use clap::Args;
use handler_common::{HandlerError, OutputFormat};
use std::collections::HashMap;

/// Delete an element at a specified path
#[derive(Args)]
pub struct RemoveCommand {
    pub file: String,
    pub path: String,
    /// Excel cell only: shift surrounding cells left or up to fill the gap.
    #[arg(long, value_name = "DIRECTION")]
    pub shift: Option<String>,
    /// Modifier properties (C#-compatible; e.g. revision.author=Ada).
    #[arg(long = "prop", num_args = 1..)]
    pub prop: Vec<String>,
}

pub fn handle_remove(cmd: RemoveCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;
    let properties: HashMap<String, String> = cmd
        .prop
        .iter()
        .filter_map(|property| {
            property
                .split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect();
    let mut properties = properties;
    if let Some(shift) = cmd.shift {
        let extension = std::path::Path::new(&cmd.file)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        if !matches!(extension.as_str(), "xlsx" | "xlsm") {
            return Err(HandlerError::InvalidArgument(
                "--shift is supported only for Excel cell paths (e.g. /Sheet1/B5)".to_string(),
            ));
        }
        properties.insert("shift".to_string(), shift);
    }
    let warning = handler.remove_with_properties(&cmd.path, &properties)?;
    handler.save()?;
    match warning {
        Some(w) => Ok(format!("Removed (warning: {})", w)),
        None => Ok("Removed".to_string()),
    }
}
