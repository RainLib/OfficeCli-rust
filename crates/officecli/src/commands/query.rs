use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Find all elements of a given type (paragraph, table, image, page, text-block)
#[derive(Args)]
pub struct QueryCommand {
    /// Document file path
    pub file: String,

    /// CSS-like selector (e.g. "p[@style=Normal]", "shape[@id=5]")
    pub selector: String,
}

pub fn handle_query(cmd: QueryCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let nodes = handler.query(&cmd.selector)?;

    match format {
        OutputFormat::Text => {
            let lines: Vec<String> = nodes
                .iter()
                .map(|n| format!("{} ({})", n.path, n.element_type))
                .collect();
            Ok(lines.join("\n"))
        }
        OutputFormat::Json => Ok(serde_json::to_string_pretty(&nodes)?),
    }
}
