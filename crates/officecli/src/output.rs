use handler_common::OutputFormat;

/// Format output for CLI display.
pub struct OutputFormatter;

impl OutputFormatter {
    pub fn format_text(content: &str, format: OutputFormat) -> String {
        match format {
            OutputFormat::Text => content.to_string(),
            OutputFormat::Json => serde_json::json!({"result": content}).to_string(),
        }
    }

    pub fn format_error(error: &str, format: OutputFormat) -> String {
        match format {
            OutputFormat::Text => format!("Error: {}", error),
            OutputFormat::Json => serde_json::json!({"error": error}).to_string(),
        }
    }

    pub fn format_success(message: &str, format: OutputFormat) -> String {
        match format {
            OutputFormat::Text => message.to_string(),
            OutputFormat::Json => serde_json::json!({"result": message}).to_string(),
        }
    }
}
