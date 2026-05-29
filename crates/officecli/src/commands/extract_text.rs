use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Extract document text with offset→path mapping for AI agent positioning.
#[derive(Args)]
pub struct ExtractTextCommand {
    /// Document file path
    pub file: String,

    /// Include offset→path mapping
    #[arg(long)]
    pub with_offsets: bool,
}

pub fn handle_extract_text(
    cmd: ExtractTextCommand,
    format: OutputFormat,
) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;

    if cmd.with_offsets {
        let map = handler.extract_text_with_offsets()?;
        match format {
            OutputFormat::Text => {
                let mut result = String::new();
                result.push_str(&map.full_text);
                result.push_str("\n\n--- Offset→Path Mapping ---\n");
                for span in &map.spans {
                    result.push_str(&format!(
                        "  [{}..{}] → {} ({})\n",
                        span.start, span.end, span.path, span.element_type
                    ));
                }
                Ok(result)
            }
            OutputFormat::Json => Ok(serde_json::to_string_pretty(&map)?),
        }
    } else {
        let text = handler.view_as_text(handler_common::ViewOptions::default())?;
        match format {
            OutputFormat::Text => Ok(text),
            OutputFormat::Json => {
                Ok(serde_json::json!({"text": text, "format": handler.format_name()}).to_string())
            }
        }
    }
}
