use clap::Args;
use handler_common::{HandlerError, InsertPosition, OutputFormat};
use std::collections::HashMap;

/// Insert a new element (paragraph, table, slide, image) into the document
#[derive(Args)]
pub struct AddCommand {
    /// Document file path
    pub file: String,

    /// Parent path where to add
    #[arg(long)]
    pub parent: String,

    /// Element type to add
    #[arg(long)]
    pub type_name: String,

    /// Position: index number, "after:/path", or "before:/path"
    #[arg(long)]
    pub position: Option<String>,

    /// Properties (key=value pairs)
    #[arg(long, num_args = 1..)]
    pub properties: Vec<String>,
}

pub fn handle_add(cmd: AddCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;

    let position = parse_position(cmd.position.as_deref());
    let properties = parse_properties(&cmd.properties);

    let new_path = handler.add(&cmd.parent, &cmd.type_name, position, &properties)?;
    handler.save()?;

    match format {
        OutputFormat::Text => Ok(format!("Created: {}", new_path)),
        OutputFormat::Json => Ok(serde_json::json!({"path": new_path}).to_string()),
    }
}

pub fn parse_position(input: Option<&str>) -> InsertPosition {
    match input {
        None => InsertPosition::Append,
        Some(s) => {
            if let Some(idx) = s.parse::<usize>().ok() {
                InsertPosition::AtIndex(idx)
            } else if let Some(rest) = s.strip_prefix("after:") {
                InsertPosition::AfterElement(rest.to_string())
            } else if let Some(rest) = s.strip_prefix("before:") {
                InsertPosition::BeforeElement(rest.to_string())
            } else {
                InsertPosition::Append
            }
        }
    }
}

fn parse_properties(props: &[String]) -> HashMap<String, String> {
    props
        .iter()
        .filter_map(|p| {
            let parts: Vec<&str> = p.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect()
}
