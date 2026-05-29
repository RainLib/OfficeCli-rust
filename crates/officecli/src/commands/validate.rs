use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Check document structure for errors or issues
#[derive(Args)]
pub struct ValidateCommand {
    pub file: String,
}

pub fn handle_validate(cmd: ValidateCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let errors = handler.validate()?;
    match format {
        OutputFormat::Text => {
            if errors.is_empty() {
                Ok("No validation errors".to_string())
            } else {
                let lines: Vec<String> = errors
                    .iter()
                    .map(|e| format!("{}: {}", e.error_type, e.description))
                    .collect();
                Ok(lines.join("\n"))
            }
        }
        OutputFormat::Json => Ok(serde_json::to_string_pretty(&errors)?),
    }
}
