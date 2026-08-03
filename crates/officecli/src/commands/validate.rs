use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Check document structure for errors or issues
#[derive(Args)]
pub struct ValidateCommand {
    pub file: String,
}

pub fn handle_validate_with_status(
    cmd: ValidateCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let errors = handler.validate()?;
    let valid = errors.is_empty();
    let output = match format {
        OutputFormat::Text => {
            if valid {
                "Validation passed: no errors found.".to_string()
            } else {
                let mut lines = vec![format!("Found {} validation error(s):", errors.len())];
                for error in errors {
                    lines.push(format!("  [{}] {}", error.error_type, error.description));
                    if let Some(path) = error.path {
                        lines.push(format!("    Path: {}", path));
                    }
                    if let Some(part) = error.part {
                        lines.push(format!("    Part: {}", part));
                    }
                }
                lines.join("\n")
            }
        }
        OutputFormat::Json => {
            crate::commands::json_data_envelope(serde_json::to_value(&errors)?, valid)
        }
    };
    Ok((output, valid))
}
