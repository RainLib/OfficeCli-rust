use clap::Args;
use handler_common::{HandlerError, OutputFormat, ViewOptions};

/// Display document content in various modes (text, outline, annotated, html, svg, pdf, forms)
#[derive(Args)]
pub struct ViewCommand {
    /// Document file path
    pub file: String,

    /// C#-compatible positional view mode. Takes precedence over `--mode`.
    #[arg(value_name = "MODE")]
    pub positional_mode: Option<String>,

    /// View mode: text, annotated, outline, stats, issues, html, svg, screenshot, pdf, forms
    #[arg(short, long, default_value = "text")]
    pub mode: String,

    /// Excel cell range to display (for example `Sheet1!A1:C10`)
    #[arg(long)]
    pub range: Option<String>,

    /// Start line number
    #[arg(long = "start", visible_alias = "start-line")]
    pub start_line: Option<usize>,

    /// End line number
    #[arg(long = "end", visible_alias = "end-line")]
    pub end_line: Option<usize>,

    /// Max lines to display
    #[arg(long)]
    pub max_lines: Option<usize>,

    /// Column filter, comma-separated (Excel only, e.g. A,B,C)
    #[arg(long)]
    pub cols: Option<String>,

    /// Page filter (e.g. 1, 2-5, 1,3,5). html mode: default=all. screenshot mode: default=1
    #[arg(long)]
    pub page: Option<String>,

    /// Open output in browser (html / svg modes)
    #[arg(long)]
    pub browser: bool,

    /// Output file path (screenshot mode; defaults to a temp file)
    #[arg(long, short)]
    pub out: Option<String>,

    /// Screenshot viewport width in pixels (default 1600)
    #[arg(long, default_value_t = 1600)]
    pub screenshot_width: u32,

    /// Screenshot viewport height in pixels (default 1200)
    #[arg(long, default_value_t = 1200)]
    pub screenshot_height: u32,

    /// Tile slides into an N-column thumbnail grid (screenshot mode, pptx only; 0 = off)
    #[arg(long, default_value_t = 0)]
    pub grid: u32,

    /// Screenshot rendering path (docx only): auto, native, html
    #[arg(long, default_value = "auto")]
    pub render: String,

    /// stats mode (docx only): also report total page count via Word repagination
    #[arg(long)]
    pub page_count: bool,

    /// Issue type filter (for issues mode)
    #[arg(long)]
    pub r#type: Option<String>,

    /// Limit number of results (for issues mode)
    #[arg(long)]
    pub limit: Option<usize>,
}

pub fn handle_view(cmd: ViewCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let mode = cmd.positional_mode.as_deref().unwrap_or(&cmd.mode);

    // pdf mode: export to PDF via exporter plugin
    if mode.eq_ignore_ascii_case("pdf") {
        return handle_view_pdf(&cmd, format);
    }

    let opts = ViewOptions {
        range: cmd.range.clone(),
        start_line: cmd.start_line,
        end_line: cmd.end_line,
        max_lines: cmd.max_lines,
        cols: cmd
            .cols
            .as_ref()
            .map(|c| c.split(',').map(|s| s.to_string()).collect()),
        page: cmd.page.clone(),
        lazy_load: false,
    };

    let browser = cmd.browser;
    let issue_type = cmd.r#type.as_deref();
    let issue_limit = cmd.limit;

    match format {
        OutputFormat::Text => match mode {
            "text" | "t" => handler.view_as_text(opts),
            "annotated" | "a" => handler.view_as_annotated(opts),
            "outline" | "o" => handler.view_as_outline(),
            "stats" | "s" => handler.view_as_stats(),
            "issues" | "i" => {
                let issues = handler.view_as_issues(issue_type, issue_limit)?;
                let lines: Vec<String> = issues
                    .iter()
                    .map(|i| format!("[{:?}] {}: {}", i.severity, i.issue_type, i.description))
                    .collect();
                Ok(lines.join("\n"))
            }
            "html" | "h" => {
                let html = handler.view_as_html(opts)?;
                if browser {
                    open_html_in_browser(&html, &cmd.file)?;
                }
                Ok(html)
            }
            "svg" | "g" => {
                let svg = handler.view_as_svg()?;
                if browser {
                    open_svg_in_browser(&svg, &cmd.file)?;
                }
                Ok(svg)
            }
            "screenshot" | "p" => handle_screenshot(handler.as_ref(), &cmd),
            "forms" | "f" => handler.view_as_forms(),
            other => Err(HandlerError::UnsupportedMode(format!(
                "view mode '{}' not supported. Available: text, annotated, outline, stats, issues, html, svg, screenshot, pdf, forms",
                other
            ))),
        },
        OutputFormat::Json => {
            let json_val = match mode {
                "text" | "t" => handler.view_as_text_json(opts)?,
                "outline" | "o" => handler.view_as_outline_json()?,
                "stats" | "s" => handler.view_as_stats_json()?,
                "issues" | "i" => {
                    let issues = handler.view_as_issues(issue_type, issue_limit)?;
                    serde_json::to_value(&issues)
                        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?
                }
                "annotated" | "a" => {
                    serde_json::json!({ "annotated": handler.view_as_annotated(opts)? })
                }
                "html" | "h" => {
                    serde_json::json!({ "html": handler.view_as_html(opts)? })
                }
                "svg" | "g" => {
                    serde_json::json!({ "svg": handler.view_as_svg()? })
                }
                "screenshot" | "p" => {
                    let result = handle_screenshot(handler.as_ref(), &cmd)?;
                    serde_json::json!({ "screenshot": result })
                }
                "forms" | "f" => {
                    let forms = handler.view_as_forms()?;
                    serde_json::json!({ "forms": forms })
                }
                other => {
                    return Err(HandlerError::UnsupportedMode(format!(
                        "view mode '{}' not supported. Available: text, annotated, outline, stats, issues, html, svg, screenshot, pdf, forms",
                        other
                    )))
                }
            };
            Ok(json_val.to_string())
        }
    }
}

/// Handle `view -m pdf` — export document to PDF.
fn handle_view_pdf(cmd: &ViewCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let pdf_path = cmd.out.clone().unwrap_or_else(|| {
        std::path::Path::new(&cmd.file)
            .with_extension("pdf")
            .to_string_lossy()
            .to_string()
    });

    // For docx/pptx/xlsx: use the handler's HTML preview and a headless browser
    // to render to PDF. This mirrors the C# exporter plugin approach.
    let handler = crate::open_handler(&cmd.file, false)?;
    let opts = ViewOptions {
        page: cmd.page.clone(),
        ..Default::default()
    };
    let html = handler.view_as_html(opts)?;

    // Write HTML to temp file
    let temp_dir = std::env::temp_dir();
    let html_path = temp_dir.join(format!(
        "officecli_pdf_{}.html",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::write(&html_path, &html)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to write temp HTML: {}", e)))?;

    // Use headless browser to print to PDF
    let result = crate::screenshot::capture_pdf(
        &html_path.to_string_lossy(),
        &pdf_path,
        cmd.screenshot_width,
        cmd.screenshot_height,
    )
    .map_err(|e| HandlerError::OperationFailed(e))?;

    let _ = std::fs::remove_file(&html_path);

    Ok(format!("PDF exported: {}", result))
}

/// Handle `view -m screenshot` — render HTML, then capture to PNG via headless browser.
fn handle_screenshot(
    handler: &dyn handler_common::DocumentHandler,
    cmd: &ViewCommand,
) -> Result<String, HandlerError> {
    let opts = ViewOptions {
        range: cmd.range.clone(),
        start_line: cmd.start_line,
        end_line: cmd.end_line,
        max_lines: cmd.max_lines,
        cols: cmd
            .cols
            .as_ref()
            .map(|c| c.split(',').map(|s| s.to_string()).collect()),
        page: cmd.page.clone(),
        lazy_load: false,
    };

    // Step 1: Render HTML
    let html = handler.view_as_html(opts)?;

    // Step 2: Write to temp file
    let temp_dir = std::env::temp_dir();
    let html_path = temp_dir.join(format!(
        "officecli_screenshot_{}.html",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::write(&html_path, &html)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to write temp HTML: {}", e)))?;

    // Step 3: Determine output path
    let out_path = cmd.out.clone().unwrap_or_else(|| {
        let stem = std::path::Path::new(&cmd.file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("screenshot");
        temp_dir
            .join(format!("officecli_screenshot_{}.png", stem))
            .to_string_lossy()
            .to_string()
    });

    // Step 4: Capture screenshot
    let result = crate::screenshot::capture(
        &html_path.to_string_lossy(),
        &out_path,
        cmd.screenshot_width,
        cmd.screenshot_height,
    )
    .map_err(|e| HandlerError::OperationFailed(e))?;

    // Clean up temp HTML
    let _ = std::fs::remove_file(&html_path);

    Ok(format!(
        "Screenshot saved: {} (backend: {})",
        result.output_path, result.backend
    ))
}

/// Write HTML to a temp file and open in the default browser.
fn open_html_in_browser(html: &str, source_file: &str) -> Result<(), HandlerError> {
    let stem = std::path::Path::new(source_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("preview");
    let temp_dir = std::env::temp_dir();
    let html_path = temp_dir.join(format!(
        "officecli_preview_{}_{}.html",
        stem,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::write(&html_path, html)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to write temp HTML: {}", e)))?;
    println!("{}", html_path.display());
    open_path_in_browser(&html_path);
    Ok(())
}

/// Write SVG to a temp file and open in the default browser.
fn open_svg_in_browser(svg: &str, source_file: &str) -> Result<(), HandlerError> {
    let stem = std::path::Path::new(source_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("preview");
    let temp_dir = std::env::temp_dir();
    let svg_path = temp_dir.join(format!(
        "officecli_slide_{}_{}.svg",
        stem,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::write(&svg_path, svg)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to write temp SVG: {}", e)))?;
    println!("{}", svg_path.display());
    open_path_in_browser(&svg_path);
    Ok(())
}

/// Open a file path in the system's default browser/viewer.
fn open_path_in_browser(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", &path.to_string_lossy()])
            .spawn();
    }
}
