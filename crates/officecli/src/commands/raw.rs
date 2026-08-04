use clap::Args;
use handler_common::{HandlerError, OutputFormat, RawOptions};

/// View raw XML or PDF content stream of a document part
#[derive(Args)]
pub struct RawCommand {
    pub file: String,
    #[arg(default_value = "/document")]
    pub part_path: String,
    #[arg(long = "start", alias = "start-row")]
    pub start_row: Option<usize>,
    #[arg(long = "end", alias = "end-row")]
    pub end_row: Option<usize>,
    #[arg(long)]
    pub cols: Option<String>,
}

pub fn handle_raw(cmd: RawCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let part_path = normalize_logical_part_path(&cmd.file, &cmd.part_path);
    let opts = RawOptions {
        start_row: cmd.start_row,
        end_row: cmd.end_row,
        cols: cmd
            .cols
            .map(|c| c.split(',').map(|s| s.to_string()).collect()),
    };
    handler.raw(&part_path, opts)
}

pub(crate) fn normalize_logical_part_path(file: &str, part: &str) -> String {
    let extension = std::path::Path::new(file)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match (extension.as_str(), part) {
        ("docx" | "docm", "/styles") => "word/styles.xml".to_string(),
        // Keep semantic resource paths for the relationship-aware DOCX
        // handler: raw-set can lazily create these parts during dump replay,
        // and /document accepts binary relationship payloads.
        (
            "docx" | "docm",
            "/document" | "/settings" | "/numbering" | "/footnotes" | "/endnotes" | "/webSettings",
        ) => part.to_string(),
        _ => part.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_logical_part_path;

    #[test]
    fn keeps_pptx_semantic_parts_for_relationship_aware_handler_resolution() {
        assert_eq!(
            normalize_logical_part_path("deck.pptx", "/presentation"),
            "/presentation"
        );
        assert_eq!(normalize_logical_part_path("deck.pptx", "/theme"), "/theme");
    }
}
