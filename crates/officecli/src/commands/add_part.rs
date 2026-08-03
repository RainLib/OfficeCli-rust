/// `add-part` command — create a new document part and return its relationship ID.
use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Create a new document part and return its relationship ID for use with raw-set.
///
/// Supported part types:
///   Word: chart, header, footer
///   PPT/Excel: chart
#[derive(Args)]
pub struct AddPartCommand {
    /// Document file path
    pub file: String,

    /// Parent part path (e.g. / for document root, /Sheet1 for Excel sheet, /slide[0] for PPT slide)
    pub parent: String,

    /// Part type to create. Word: chart, header, footer. PPT/Excel: chart
    #[arg(long, alias = "type")]
    pub part_type: String,
}

pub fn handle_add_part(cmd: AddPartCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;

    let (rel_id, part_path) = handler.add_part(&cmd.parent, &cmd.part_type, None)?;

    let message = format!(
        "Created {} part: relId={} path={}",
        cmd.part_type, rel_id, part_path
    );

    match format {
        OutputFormat::Json => {
            let mut extensions = serde_json::Map::new();
            extensions.insert("relId".to_string(), serde_json::Value::String(rel_id));
            extensions.insert("partPath".to_string(), serde_json::Value::String(part_path));
            extensions.insert("type".to_string(), serde_json::Value::String(cmd.part_type));
            Ok(crate::commands::json_text_envelope(&message, extensions))
        }
        OutputFormat::Text => Ok(message),
    }
}
