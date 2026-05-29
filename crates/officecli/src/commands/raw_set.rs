use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Modify raw XML or PDF content stream
#[derive(Args)]
pub struct RawSetCommand {
    pub file: String,
    pub part_path: String,
    pub xpath: String,
    pub action: String,
    #[arg(long)]
    pub xml: Option<String>,
}

pub fn handle_raw_set(cmd: RawSetCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;
    handler.raw_set(&cmd.part_path, &cmd.xpath, &cmd.action, cmd.xml.as_deref())?;
    handler.save()?;
    Ok("OK".to_string())
}
