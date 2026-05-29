use clap::Args;
use handler_common::{HandlerError, OutputFormat, ViewOptions};

/// Display document content in various modes (text, outline, annotated, html, svg)
#[derive(Args)]
pub struct ViewCommand {
    /// Document file path
    pub file: String,

    /// View mode: text, annotated, outline, stats, issues, html, svg, pdf
    #[arg(short, long, default_value = "text")]
    pub mode: String,

    /// Start line number
    #[arg(long)]
    pub start_line: Option<usize>,

    /// End line number
    #[arg(long)]
    pub end_line: Option<usize>,

    /// Max lines to display
    #[arg(long)]
    pub max_lines: Option<usize>,

    /// Column filter (for Excel)
    #[arg(long)]
    pub cols: Option<String>,

    /// Page number (for PDF / slide number for PowerPoint)
    #[arg(long)]
    pub page: Option<usize>,
}

pub fn handle_view(cmd: ViewCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let opts = ViewOptions {
        start_line: cmd.start_line,
        end_line: cmd.end_line,
        max_lines: cmd.max_lines,
        cols: cmd
            .cols
            .map(|c| c.split(',').map(|s| s.to_string()).collect()),
        page: cmd.page,
    };

    match cmd.mode.as_str() {
        "text" => handler.view_as_text(opts),
        "annotated" => handler.view_as_annotated(opts),
        "outline" => handler.view_as_outline(),
        "stats" => handler.view_as_stats(),
        "issues" => {
            let issues = handler.view_as_issues(None, None)?;
            let lines: Vec<String> = issues
                .iter()
                .map(|i| format!("[{:?}] {}: {}", i.severity, i.issue_type, i.description))
                .collect();
            Ok(lines.join("\n"))
        }
        "html" => handler.view_as_html(opts),
        "svg" => handler.view_as_svg(),
        other => Err(HandlerError::UnsupportedMode(format!(
            "view mode '{}' not supported by this format",
            other
        ))),
    }
}
