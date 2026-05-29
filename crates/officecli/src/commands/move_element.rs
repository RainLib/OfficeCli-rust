use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Reorder an element within the document
#[derive(Args)]
pub struct MoveCommand {
    pub file: String,
    pub source: String,
    #[arg(long)]
    pub target: Option<String>,
    #[arg(long)]
    pub position: Option<String>,
}

pub fn handle_move(cmd: MoveCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;
    let pos = crate::commands::add::parse_position(cmd.position.as_deref());
    let result = handler.move_element(&cmd.source, cmd.target.as_deref(), pos)?;
    handler.save()?;
    Ok(format!("Moved to: {}", result))
}
