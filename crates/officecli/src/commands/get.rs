use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// The maximum node nesting callers may request.  This keeps `get --depth`
/// bounded on malformed or deliberately deeply nested Office XML.
const MAX_GET_DEPTH: usize = 128;

/// Retrieve a specific element at a path with its content and metadata
#[derive(Args)]
pub struct GetCommand {
    /// Document file path
    pub file: String,

    /// Path to the element (e.g. /body/p[1], /slide[1]/shape[2]). Defaults to root.
    pub path: Option<String>,

    /// Depth of children to return
    #[arg(short, long, default_value = "1")]
    pub depth: usize,

    /// Extract the node's backing binary payload (picture, OLE, or media) to this file
    #[arg(long)]
    pub save: Option<String>,

    /// Watch server port used only with the `selected` pseudo-path.
    #[arg(long)]
    pub port: Option<u16>,

    /// Watch document id used only with the `selected` pseudo-path.
    #[arg(long)]
    pub id: Option<String>,
}

pub fn handle_get(cmd: GetCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let path = cmd.path.as_deref().unwrap_or("/");
    let depth = cmd.depth.min(MAX_GET_DEPTH);
    if path.eq_ignore_ascii_case("selected") {
        let id = cmd
            .id
            .unwrap_or_else(|| crate::commands::default_id(&cmd.file));
        let selection = crate::commands::get_json(
            crate::commands::resolve_port(cmd.port),
            &format!("/{id}/selection"),
        )?;
        let paths = selection
            .get("paths")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                HandlerError::OperationFailed("invalid watch selection response".to_string())
            })?;
        let handler = crate::open_handler(&cmd.file, false)?;
        let nodes = paths
            .iter()
            .filter_map(serde_json::Value::as_str)
            .filter_map(|path| handler.get(path, depth).ok())
            .collect::<Vec<_>>();
        return match format {
            OutputFormat::Text => nodes
                .iter()
                .map(|node| format_node_text(node, depth))
                .collect::<Result<Vec<_>, _>>()
                .map(|items| items.join("\n")),
            OutputFormat::Json => serde_json::to_string_pretty(&nodes).map_err(HandlerError::from),
        };
    }
    let handler = crate::open_handler(&cmd.file, false)?;
    let mut node = handler.get(path, depth)?;

    if let Some(save_path) = cmd.save.as_deref() {
        // Keep the CLI contract friendly for scripts: a nested output directory
        // need not already exist.  The handler still determines whether the
        // requested node actually has a backing binary part.
        if let Some(parent) = std::path::Path::new(save_path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|err| {
                    HandlerError::OperationFailed(format!(
                        "failed to create destination directory '{}': {}",
                        parent.display(),
                        err
                    ))
                })?;
            }
        }

        let binary = handler.try_extract_binary(path, save_path)?.ok_or_else(|| {
            HandlerError::OperationFailed(format!(
                "node at '{}' has no binary payload to extract (only picture, OLE, media, or embedded nodes can be saved)",
                path
            ))
        })?;
        node.format.insert(
            "savedTo".to_string(),
            Some(serde_json::Value::String(save_path.to_string())),
        );
        node.format.insert(
            "savedBytes".to_string(),
            Some(serde_json::Value::from(binary.byte_count)),
        );
        node.format.insert(
            "savedContentType".to_string(),
            Some(serde_json::Value::String(binary.content_type)),
        );
    }

    match format {
        OutputFormat::Text => format_node_text(&node, depth),
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
