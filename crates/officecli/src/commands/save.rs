use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Persist changes back to the original file
#[derive(Args)]
pub struct SaveCommand {
    pub file: String,
}

pub fn handle_save(cmd: SaveCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;
    handler.save()?;
    let file_name = std::path::Path::new(&cmd.file)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&cmd.file);
    Ok(format!("{} is already saved to disk.", file_name))
}
