use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Retrieve a specific element at a path with its content and metadata
#[derive(Args)]
pub struct GetCommand {
    /// Document file path
    pub file: String,

    /// Path to the element (e.g. /body/p[1], /slide[1]/shape[2])
    pub path: String,

    /// Depth of children to return
    #[arg(short, long, default_value = "1")]
    pub depth: usize,
}

pub fn handle_get(cmd: GetCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let node = handler.get(&cmd.path, cmd.depth)?;

    match format {
        OutputFormat::Text => format_node_text(&node, cmd.depth),
        OutputFormat::Json => Ok(serde_json::to_string_pretty(&node)?),
    }
}

fn format_node_text(
    node: &handler_common::DocumentNode,
    _depth: usize,
) -> Result<String, HandlerError> {
    let mut result = String::new();
    result.push_str(&format!("Path: {}\n", node.path));
    result.push_str(&format!("Type: {}\n", node.element_type));
    if let Some(text) = &node.text {
        result.push_str(&format!("Text: {}\n", text));
    }
    if let Some(style) = &node.style {
        result.push_str(&format!("Style: {}\n", style));
    }
    result.push_str(&format!("Child count: {}\n", node.child_count));
    Ok(result)
}
