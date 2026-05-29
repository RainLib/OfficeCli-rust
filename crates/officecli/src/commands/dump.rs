use clap::Args;
use handler_common::HandlerError;

/// Export full document structure and content as JSON
#[derive(Args)]
pub struct DumpCommand {
    pub file: String,
    #[arg(long)]
    pub path: Option<String>,
}

pub fn handle_dump(
    cmd: DumpCommand,
    format: handler_common::OutputFormat,
) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;

    if let Some(path) = cmd.path {
        // Dump a specific node as JSON
        let node = handler.get(&path, 10)?;
        let json = serde_json::to_string_pretty(&node).map_err(|e| HandlerError::JsonError(e))?;
        Ok(json)
    } else {
        // Dump the entire document structure
        let root = handler.get("/", 3)?;
        let json = serde_json::to_string_pretty(&root).map_err(|e| HandlerError::JsonError(e))?;
        Ok(json)
    }
}
