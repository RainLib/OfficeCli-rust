use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Delete an element at a specified path
#[derive(Args)]
pub struct RemoveCommand {
    pub file: String,
    pub path: String,
}

pub fn handle_remove(cmd: RemoveCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;
    let warning = handler.remove(&cmd.path)?;
    handler.save()?;
    match warning {
        Some(w) => Ok(format!("Removed (warning: {})", w)),
        None => Ok("Removed".to_string()),
    }
}
