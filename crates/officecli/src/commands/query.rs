use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Find all elements of a given type (paragraph, table, image, page, text-block)
#[derive(Args)]
pub struct QueryCommand {
    /// Document file path
    pub file: String,

    /// CSS-like selector (e.g. "p[@style=Normal]", "shape[@id=5]")
    pub selector: String,

    /// Filter results to elements containing this text (case-insensitive substring).
    /// Use r"..." or r'...' for a case-insensitive regular expression.
    #[arg(long)]
    pub find: Option<String>,
}

pub fn handle_query(cmd: QueryCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let nodes = handler.query(&cmd.selector)?;
    let nodes = if let Some(find) = cmd.find.as_deref() {
        let mut filtered = Vec::with_capacity(nodes.len());
        for node in nodes {
            let matches = match node.text.as_deref() {
                Some(text) => handler_common::matches_text_filter(text, find).map_err(|error| {
                    HandlerError::InvalidArgument(format!(
                        "invalid regex pattern in '{}': {}",
                        find, error
                    ))
                })?,
                None => false,
            };
            if matches {
                filtered.push(node);
            }
        }
        filtered
    } else {
        nodes
    };

    match format {
        OutputFormat::Text => {
            let lines: Vec<String> = nodes
                .iter()
                .map(|n| format!("{} ({})", n.path, n.element_type))
                .collect();
            Ok(lines.join("\n"))
        }
        OutputFormat::Json => crate::commands::nodes_json_envelope(&nodes),
    }
}
