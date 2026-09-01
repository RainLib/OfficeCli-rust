use clap::Args;
use handler_common::{DocumentHandler, HandlerError};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvertEngine {
    LibreOffice,
    Oxide,
    PdfText,
    Pdf2Docx,
}

impl std::str::FromStr for ConvertEngine {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "libreoffice" | "lo" => Ok(Self::LibreOffice),
            "oxide" => Ok(Self::Oxide),
            "pdf-text" | "pdf_text" | "text" => Ok(Self::PdfText),
            "pdf2docx" | "pdf-2-docx" | "pdf_2_docx" => Ok(Self::Pdf2Docx),
            other => Err(format!(
                "unknown engine '{}' (choose: libreoffice, oxide, pdf-text, pdf2docx)",
                other
            )),
        }
    }
}

/// Helper for MCP to parse engine string without needing FromStr in scope.
pub fn parse_engine(s: &str) -> Result<ConvertEngine, String> {
    s.parse()
}

/// Convert legacy Office, PDF, or semantic HTML to a supported document format
#[derive(Args)]
#[command(after_help = "\
SUPPORTED CONVERSIONS:
  .html -> .docx/.xlsx/.pptx/.pdf   Semantic conversion (pure Rust)
  .doc  -> .docx   Word legacy binary to modern OOXML
  .xls  -> .xlsx   Excel legacy binary to modern OOXML
  .ppt  -> .pptx   PowerPoint legacy binary to modern OOXML
  .pdf  -> .docx   PDF to Word (LibreOffice, with text fallback)
  .docx -> .docx   Re-save / normalize modern Word (same for .xlsx, .pptx)

Cross-family conversions other than PDF->DOCX are NOT supported.

CONVERSION ENGINES:
  libreoffice (default)                    oxide
  ─────────────────────                    ─────
  High fidelity (~1:1)                     Lower fidelity (via IR, may lose styles/headers/objects)
  Needs LibreOffice installed (~700MB)     Pure Rust, no external dependency
  Slower (process spawn overhead)          Fast (sub-second)
  Preserves formatting, images, tables     Preserves basic content and structure
  Supports PDF -> DOCX                     Same-family only (no PDF support)
  Falls back to extractable PDF text

  pdf-text                                 PDF-only, fastest, extractable text layer -> simple DOCX
  pdf2docx                                 PDF-only, layout parser via Python pdf2docx CLI

  Install LibreOffice:
    macOS:  brew install --cask libreoffice
    Ubuntu: sudo apt install libreoffice
    Windows: https://www.libreoffice.org/download/

  Install pdf2docx:
    python3 -m pip install pdf2docx
    or set OFFICECLI_PDF2DOCX_BIN=/path/to/pdf2docx

EXAMPLES:
  officecli convert old.doc                       Convert .doc -> .docx via LibreOffice (default)
  officecli convert old.xls -o report.xlsx        Convert with custom output name
  officecli convert old.ppt --force               Convert, overwrite existing output
  officecli convert input.pdf -o output.docx      Convert PDF to Word
  officecli convert input.pdf --engine pdf-text   Fast PDF text-layer conversion
  officecli convert input.pdf --engine pdf2docx   Convert PDF via Python pdf2docx
  officecli convert input.html -o output.docx     Semantic HTML to Word
  officecli convert input.html -o output.xlsx     Tables/content to worksheet cells
  officecli convert input.html -o output.pptx     Sections/headings to slides
  officecli convert input.html -o output.pdf      Semantic paginated PDF
  officecli convert old.doc --engine oxide        Convert via oxide (no LibreOffice needed)")]
pub struct ConvertCommand {
    /// Input file path (.html, .doc, .xls, .ppt, .docx, .xlsx, .pptx, .pdf)
    pub file: String,

    /// Output file path (required for HTML; otherwise defaults to the native family extension)
    #[arg(short, long)]
    pub output: Option<String>,

    /// Overwrite output file if it already exists
    #[arg(long)]
    pub force: bool,

    /// Conversion engine for non-HTML input; HTML always uses the in-process Rust converter
    #[arg(long, default_value = "libreoffice")]
    pub engine: ConvertEngine,
}

pub fn handle_convert(
    cmd: ConvertCommand,
    format: handler_common::OutputFormat,
) -> Result<String, HandlerError> {
    let input_path = PathBuf::from(&cmd.file);
    let input_ext = input_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if matches!(input_ext.as_str(), "html" | "htm") {
        return handle_html_convert(&cmd, format, &input_path, &input_ext);
    }

    // Determine target extension based on input family
    let target_ext = match input_ext.as_str() {
        "doc" | "docx" => "docx",
        "xls" | "xlsx" => "xlsx",
        "ppt" | "pptx" => "pptx",
        "pdf" => "docx",
        other => {
            return Err(HandlerError::UnsupportedMode(format!(
                "convert from '.{}' not supported (supported: .doc, .xls, .ppt, .docx, .xlsx, .pptx, .pdf)",
                other
            )));
        }
    };

    // Check input file exists
    if !input_path.exists() {
        return Err(HandlerError::OpenError(format!(
            "input file '{}' not found",
            cmd.file
        )));
    }

    // Derive output path
    let output_path = cmd
        .output
        .map(PathBuf::from)
        .unwrap_or_else(|| input_path.with_extension(target_ext));

    // Validate output extension matches input family
    let output_ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    validate_conversion(&input_ext, &output_ext, cmd.engine)?;

    // Prevent accidental overwrite
    if output_path.exists() && !cmd.force {
        return Err(HandlerError::OperationFailed(format!(
            "output file '{}' already exists; use --force to overwrite",
            output_path.display()
        )));
    }

    // If the target is a foreign extension that the built-in native handlers
    // don't speak, delegate to a plugin exporter before falling back to the
    // built-in engines (LibreOffice / oxide). This makes
    // `officecli convert foo.docx bar.html` route through whatever
    // exporter plugin is installed for `.html`.
    if !is_native_office_ext(&output_ext) && !is_native_office_ext(&input_ext) {
        // Both foreign: the user wants, say, .doc → .html. We can do this
        // in two hops (dump-reader to native, then exporter to target) if
        // both plugins are installed.
        if let Some(dump_result) = try_plugin_dump(&input_ext, &cmd.file)? {
            if let Some(export_result) = try_plugin_export(
                dump_result.target_family(),
                &output_ext,
                &dump_result.converted_path,
                &output_path,
            )? {
                // Best-effort cleanup of the intermediate native sibling.
                let _ = std::fs::remove_file(&dump_result.converted_path);
                return match format {
                    handler_common::OutputFormat::Text => Ok(format!(
                        "Converted '{}' -> '{}' via dump-reader '{}' then exporter '{}'",
                        cmd.file,
                        export_result.output_path,
                        dump_result.plugin_name,
                        export_result.plugin_name
                    )),
                    handler_common::OutputFormat::Json => Ok(serde_json::json!({
                        "input": cmd.file,
                        "output": export_result.output_path,
                        "from_format": input_ext,
                        "to_format": output_ext,
                        "engine": "plugin-pipeline",
                        "stages": [dump_result.plugin_name, export_result.plugin_name],
                    })
                    .to_string()),
                };
            }
            // No exporter for native→foreign here — fall back to error below.
            let _ = std::fs::remove_file(&dump_result.converted_path);
        }
        return Err(HandlerError::UnsupportedMode(format!(
            "cannot convert .{} → .{} via plugin (need dump-reader + exporter). \
             See plugins/plugin-protocol.md.",
            input_ext, output_ext
        )));
    }
    if !is_native_office_ext(&output_ext) && is_native_office_ext(&input_ext) {
        if let Some(export_result) =
            try_plugin_export(&input_ext, &output_ext, &cmd.file, &output_path)?
        {
            return match format {
                handler_common::OutputFormat::Text => Ok(format!(
                    "Converted '{}' -> '{}' via plugin '{}'",
                    cmd.file, export_result.output_path, export_result.plugin_name
                )),
                handler_common::OutputFormat::Json => Ok(serde_json::json!({
                    "input": cmd.file,
                    "output": export_result.output_path,
                    "from_format": input_ext,
                    "to_format": output_ext,
                    "engine": "plugin",
                    "plugin": export_result.plugin_name,
                })
                .to_string()),
            };
        }
        // No plugin — fall through to existing engines below.
    }

    // Foreign source → native target: try a dump-reader plugin first.
    // A `.doc → .docx` invocation should route to the installed dump-reader
    // before falling back to LibreOffice, since dump-readers preserve more
    // structure than LibreOffice's generic conversion.
    if !is_native_office_ext(&input_ext)
        && is_native_office_ext(&output_ext)
        && output_ext == family_from_foreign(&input_ext)
    {
        if let Some(dump_result) = try_plugin_dump(&input_ext, &cmd.file)? {
            // The dump-reader wrote to its own sibling path; if that's not
            // the same as `output_path`, copy across (or rename).
            if dump_result.converted_path != output_path.to_string_lossy() {
                std::fs::rename(&dump_result.converted_path, &output_path)
                    .or_else(|_| {
                        std::fs::copy(&dump_result.converted_path, &output_path).map(|_| ())
                    })
                    .map_err(|e| {
                        HandlerError::OperationFailed(format!(
                            "failed to move dump-reader output to '{}': {}",
                            output_path.display(),
                            e
                        ))
                    })?;
            }
            return match format {
                handler_common::OutputFormat::Text => Ok(format!(
                    "Converted '{}' -> '{}' via dump-reader plugin '{}' ({} items)",
                    cmd.file,
                    output_path.display(),
                    dump_result.plugin_name,
                    dump_result.items_replayed
                )),
                handler_common::OutputFormat::Json => Ok(serde_json::json!({
                    "input": cmd.file,
                    "output": output_path.to_string_lossy(),
                    "from_format": input_ext,
                    "to_format": output_ext,
                    "engine": "plugin-dump-reader",
                    "plugin": dump_result.plugin_name,
                    "items_replayed": dump_result.items_replayed,
                })
                .to_string()),
            };
        }
        // No dump-reader installed — fall through to LibreOffice.
    }

    // Perform conversion with selected engine
    let used_engine = match cmd.engine {
        ConvertEngine::LibreOffice if input_ext == "pdf" && output_ext == "docx" => {
            convert_pdf_to_docx(&cmd.file, &output_path, target_ext)?
        }
        ConvertEngine::PdfText if input_ext == "pdf" && output_ext == "docx" => {
            convert_pdf_text_to_docx(&cmd.file, &output_path)?;
            "pdf-text"
        }
        ConvertEngine::Pdf2Docx if input_ext == "pdf" && output_ext == "docx" => {
            convert_via_pdf2docx(&cmd.file, &output_path)?;
            "pdf2docx"
        }
        ConvertEngine::LibreOffice => {
            convert_via_libreoffice(&cmd.file, &output_path, target_ext)?;
            "libreoffice"
        }
        ConvertEngine::Oxide => {
            convert_via_oxide(&cmd.file, &output_path)?;
            "oxide"
        }
        ConvertEngine::PdfText => {
            return Err(HandlerError::UnsupportedMode(
                "--engine pdf-text is only supported for .pdf -> .docx".to_string(),
            ));
        }
        ConvertEngine::Pdf2Docx => {
            return Err(HandlerError::UnsupportedMode(
                "--engine pdf2docx is only supported for .pdf -> .docx".to_string(),
            ));
        }
    };

    match format {
        handler_common::OutputFormat::Text => Ok(format!(
            "Converted '{}' -> '{}' [{}]",
            cmd.file,
            output_path.display(),
            used_engine
        )),
        handler_common::OutputFormat::Json => Ok(serde_json::json!({
            "input": cmd.file,
            "output": output_path.to_string_lossy(),
            "from_format": input_ext,
            "to_format": target_ext,
            "engine": used_engine,
        })
        .to_string()),
    }
}

fn handle_html_convert(
    cmd: &ConvertCommand,
    format: handler_common::OutputFormat,
    input_path: &std::path::Path,
    input_ext: &str,
) -> Result<String, HandlerError> {
    if !input_path.exists() {
        return Err(HandlerError::OpenError(format!(
            "input file '{}' not found",
            cmd.file
        )));
    }
    let output_path = cmd.output.as_ref().map(PathBuf::from).ok_or_else(|| {
        HandlerError::InvalidArgument(
            "HTML conversion requires --output with .docx, .xlsx, .pptx, or .pdf extension"
                .to_string(),
        )
    })?;
    let output_ext = output_path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    validate_conversion(input_ext, &output_ext, cmd.engine)?;
    if output_path.exists() && !cmd.force {
        return Err(HandlerError::OperationFailed(format!(
            "output file '{}' already exists; use --force to overwrite",
            output_path.display()
        )));
    }
    let summary = super::html_convert::convert_html(input_path, &output_path, &output_ext)?;
    match format {
        handler_common::OutputFormat::Text => Ok(format!(
            "Converted '{}' -> '{}' [rust-html-semantic, fidelity={}, blocks={}, images={}/{}, warnings={}]",
            cmd.file,
            output_path.display(),
            summary.fidelity,
            summary.block_count,
            summary.embedded_image_count,
            summary.image_count,
            summary.warnings.len()
        )),
        handler_common::OutputFormat::Json => Ok(serde_json::json!({
            "input": cmd.file,
            "output": output_path.to_string_lossy(),
            "from_format": input_ext,
            "to_format": output_ext,
            "engine": "rust-html-semantic",
            "fidelity": summary.fidelity,
            "block_count": summary.block_count,
            "image_count": summary.image_count,
            "embedded_image_count": summary.embedded_image_count,
            "warnings": summary.warnings,
        })
        .to_string()),
    }
}

/// Convert PDF to DOCX.
///
/// A direct `writer_pdf_import --convert-to docx` places every line of text in
/// an absolutely-positioned, `behindDoc` text frame while leaving full-page
/// white VML rectangles in the normal z-order — Microsoft Word then renders
/// those white boxes on top of the behind-text frames, so the document opens
/// blank even though previews (which use a different renderer) show the text.
///
/// Routing through an intermediate MS Word 97 `.doc` makes LibreOffice
/// normalize the mixed VML/DrawingML shapes into a single, consistently-layered
/// DrawingML model that Word renders correctly. We therefore try the two-hop
/// bridge first, then fall back to the direct conversion, and finally to the
/// in-tree PDF text extractor.
fn convert_pdf_to_docx(
    input_file: &str,
    output_path: &std::path::Path,
    target_ext: &str,
) -> Result<&'static str, HandlerError> {
    // Preferred path: PDF -> .doc (MS Word 97) -> .docx, which Word renders
    // correctly. Only accept it when the result actually carries text.
    if convert_pdf_to_docx_via_doc(input_file, output_path).is_ok()
        && pdf_text_is_preserved(input_file, output_path)
    {
        return Ok("libreoffice-doc-bridge");
    }

    match convert_via_libreoffice(input_file, output_path, target_ext) {
        Ok(()) if pdf_text_is_preserved(input_file, output_path) => Ok("libreoffice"),
        Ok(()) => {
            convert_pdf_text_to_docx(input_file, output_path)?;
            Ok("pdf-text-fallback")
        }
        Err(lo_err) => match convert_pdf_text_to_docx(input_file, output_path) {
            Ok(()) => Ok("pdf-text-fallback"),
            Err(fallback_err) => Err(HandlerError::OperationFailed(format!(
                "PDF to DOCX failed; LibreOffice: {}; text fallback: {}",
                lo_err, fallback_err
            ))),
        },
    }
}

/// Two-hop PDF -> DOCX bridge: convert the PDF to an MS Word 97 `.doc` first,
/// then to `.docx`. Both hops run in an isolated temp directory so concurrent
/// conversions never collide on filenames; the temp directory is always
/// cleaned up before returning.
fn convert_pdf_to_docx_via_doc(
    input_file: &str,
    output_path: &std::path::Path,
) -> Result<(), HandlerError> {
    let soffice = find_soffice()?;

    let work_dir = unique_temp_path("officecli-pdf2docx");
    std::fs::create_dir_all(&work_dir).map_err(|e| {
        HandlerError::OperationFailed(format!("cannot create temp dir for PDF bridge: {}", e))
    })?;
    // Private soffice profile under the (auto-cleaned) work dir so both hops
    // are isolated from any concurrent conversion.
    let profile_dir = work_dir.join("lo-profile");

    let result = (|| {
        let stem = PathBuf::from(input_file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output")
            .to_string();

        // Hop 1: PDF -> MS Word 97 .doc (normalizes the shape/z-order model).
        run_soffice(
            &soffice,
            &profile_dir,
            &[
                "--headless".as_ref(),
                "--infilter=writer_pdf_import".as_ref(),
                "--convert-to".as_ref(),
                "doc:MS Word 97".as_ref(),
                "--outdir".as_ref(),
                work_dir.as_os_str(),
                input_file.as_ref(),
            ],
        )?;
        let doc_path = work_dir.join(format!("{}.doc", stem));
        if !doc_path.exists() {
            return Err(HandlerError::OperationFailed(
                "PDF bridge: soffice did not produce intermediate .doc".to_string(),
            ));
        }

        // Hop 2: .doc -> .docx.
        run_soffice(
            &soffice,
            &profile_dir,
            &[
                "--headless".as_ref(),
                "--convert-to".as_ref(),
                "docx".as_ref(),
                "--outdir".as_ref(),
                work_dir.as_os_str(),
                doc_path.as_os_str(),
            ],
        )?;
        let docx_path = work_dir.join(format!("{}.docx", stem));
        if !docx_path.exists() {
            return Err(HandlerError::OperationFailed(
                "PDF bridge: soffice did not produce intermediate .docx".to_string(),
            ));
        }

        // Move the result to the requested output path.
        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    HandlerError::OperationFailed(format!("cannot create output dir: {}", e))
                })?;
            }
        }
        std::fs::rename(&docx_path, output_path)
            .or_else(|_| std::fs::copy(&docx_path, output_path).map(|_| ()))
            .map_err(|e| {
                HandlerError::OperationFailed(format!(
                    "PDF bridge: failed to move result to '{}': {}",
                    output_path.display(),
                    e
                ))
            })
    })();

    let _ = std::fs::remove_dir_all(&work_dir);
    result
}

/// Run `soffice` with the given arguments, mapping spawn/exit failures into a
/// `HandlerError`. stdout/stderr are suppressed (piped) to keep the CLI quiet.
///
/// `profile_dir` is passed to soffice as `-env:UserInstallation`, giving this
/// invocation a private user profile. Without it, every soffice process shares
/// the default profile and serializes on its lock file, so concurrent
/// conversions block (or fail) on each other.
fn run_soffice(
    soffice: &str,
    profile_dir: &std::path::Path,
    args: &[&std::ffi::OsStr],
) -> Result<(), HandlerError> {
    let user_install = format!("-env:UserInstallation=file://{}", profile_dir.display());
    let status = std::process::Command::new(soffice)
        .arg(&user_install)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status()
        .map_err(|e| HandlerError::OperationFailed(format!("failed to run soffice: {}", e)))?;

    if !status.success() {
        return Err(HandlerError::OperationFailed(format!(
            "soffice conversion failed (exit code {})",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(())
}

/// Build a unique path under the system temp dir (process id + high-resolution
/// timestamp). Used for per-invocation soffice profiles and scratch dirs so
/// concurrent conversions never collide.
fn unique_temp_path(prefix: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "{}-{}-{}",
        prefix,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    p
}

fn pdf_text_is_preserved(input_file: &str, output_path: &std::path::Path) -> bool {
    let Some(output_path) = output_path.to_str() else {
        return false;
    };
    let Ok(handler) = docx_handler::WordHandler::open(output_path, false) else {
        return false;
    };
    let Ok(docx_text) = handler.view_as_text(handler_common::ViewOptions::default()) else {
        return false;
    };
    if docx_text.trim().is_empty() {
        return false;
    }

    let Ok(reader) = pdf_handler::reader::PdfReader::open(input_file) else {
        return true;
    };
    let expected: Vec<String> = extract_pdf_docx_paragraphs(&reader)
        .into_iter()
        .filter_map(|paragraph| match paragraph {
            DocxParagraph::Text(text) if !text.trim().is_empty() => Some(text),
            _ => None,
        })
        .collect();
    expected.is_empty() || expected.iter().all(|text| docx_text.contains(text))
}

#[derive(Debug)]
enum DocxParagraph {
    Text(String),
    PageBreak,
}

#[derive(Debug)]
struct PdfLineSegment {
    text: String,
    x: f32,
    right: f32,
    font_size: f32,
}

fn convert_pdf_text_to_docx(
    input_file: &str,
    output_path: &std::path::Path,
) -> Result<(), HandlerError> {
    let reader = pdf_handler::reader::PdfReader::open(input_file)?;
    let paragraphs = extract_pdf_docx_paragraphs(&reader);

    if !paragraphs
        .iter()
        .any(|p| matches!(p, DocxParagraph::Text(text) if !text.trim().is_empty()))
    {
        return Err(HandlerError::OperationFailed(
            "PDF contains no extractable text; OCR is required before converting to DOCX"
                .to_string(),
        ));
    }

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                HandlerError::OperationFailed(format!("cannot create output dir: {}", e))
            })?;
        }
    }

    write_text_docx(output_path, &paragraphs)
}

fn extract_pdf_docx_paragraphs(reader: &pdf_handler::reader::PdfReader) -> Vec<DocxParagraph> {
    let mut paragraphs = Vec::new();

    for page_num in 1..=reader.page_count() {
        if page_num > 1 && paragraphs.last().is_some() {
            paragraphs.push(DocxParagraph::PageBreak);
        }

        let Some(parsed) = reader.parse_page_text_blocks(page_num) else {
            continue;
        };

        let mut current_line: Vec<PdfLineSegment> = Vec::new();
        let mut current_y: Option<f32> = None;
        let mut current_tolerance = 2.0_f32;

        for block in parsed.text_blocks {
            if block.text.is_empty() {
                continue;
            }

            let tolerance = (block.bbox.height * 0.4).max(2.0);
            let starts_new_line = current_y
                .map(|y| (block.bbox.y - y).abs() > current_tolerance.max(tolerance))
                .unwrap_or(false);

            if starts_new_line {
                push_pdf_line(&mut paragraphs, &mut current_line);
                current_y = None;
                current_tolerance = 2.0;
            }

            current_y.get_or_insert(block.bbox.y);
            current_tolerance = current_tolerance.max(tolerance);
            current_line.push(PdfLineSegment {
                text: block.text,
                x: block.bbox.x,
                right: block.bbox.x + block.bbox.width,
                font_size: block.style.font_size.unwrap_or(block.bbox.height).max(1.0),
            });
        }

        push_pdf_line(&mut paragraphs, &mut current_line);
    }

    paragraphs
}

fn push_pdf_line(paragraphs: &mut Vec<DocxParagraph>, segments: &mut Vec<PdfLineSegment>) {
    if segments.is_empty() {
        return;
    }

    segments.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));

    let mut line = String::new();
    let mut last_right: Option<f32> = None;
    let mut last_font_size = 12.0_f32;

    for segment in segments.drain(..) {
        if let Some(right) = last_right {
            let gap = segment.x - right;
            let threshold = (last_font_size * 0.35).max(3.0);
            if gap > threshold && should_insert_space(&line, &segment.text) {
                line.push(' ');
            }
        }
        line.push_str(&segment.text);
        last_right = Some(segment.right);
        last_font_size = segment.font_size;
    }

    let line = line.trim_end().to_string();
    if !line.trim().is_empty() {
        paragraphs.push(DocxParagraph::Text(line));
    }
}

fn should_insert_space(existing: &str, next: &str) -> bool {
    let prev_has_space = existing
        .chars()
        .last()
        .map(|c| c.is_whitespace())
        .unwrap_or(true);
    let next_has_space = next
        .chars()
        .next()
        .map(|c| c.is_whitespace())
        .unwrap_or(true);
    !prev_has_space && !next_has_space && !starts_with_closing_punctuation(next)
}

fn starts_with_closing_punctuation(text: &str) -> bool {
    text.chars()
        .next()
        .map(|c| {
            matches!(
                c,
                '，' | '。'
                    | '、'
                    | '；'
                    | '：'
                    | '！'
                    | '？'
                    | ','
                    | '.'
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | ')'
                    | ']'
                    | '}'
                    | '）'
                    | '】'
                    | '」'
                    | '』'
            )
        })
        .unwrap_or(false)
}

fn write_text_docx(
    output_path: &std::path::Path,
    paragraphs: &[DocxParagraph],
) -> Result<(), HandlerError> {
    use oxml::OxmlPackage;

    let mut body = String::new();
    for paragraph in paragraphs {
        match paragraph {
            DocxParagraph::Text(text) => {
                body.push_str("    <w:p><w:r><w:t xml:space=\"preserve\">");
                body.push_str(&xml_escape_text(text));
                body.push_str("</w:t></w:r></w:p>\n");
            }
            DocxParagraph::PageBreak => {
                body.push_str("    <w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>\n");
            }
        }
    }

    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
{body}  </w:body>
</w:document>"#
    );

    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    let mut pkg = OxmlPackage::create(&output_path.to_string_lossy());
    pkg.add_part("[Content_Types].xml", content_types.as_bytes());
    pkg.add_part("_rels/.rels", rels.as_bytes());
    pkg.add_part("word/document.xml", document_xml.as_bytes());
    pkg.add_part(
        "word/_rels/document.xml.rels",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>",
    );

    pkg.save_as(&output_path.to_string_lossy())
        .map_err(|e| HandlerError::SaveError(e.to_string()))
}

fn xml_escape_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Convert via LibreOffice CLI (soffice --convert-to).
fn convert_via_libreoffice(
    input_file: &str,
    output_path: &std::path::Path,
    target_ext: &str,
) -> Result<(), HandlerError> {
    let soffice = find_soffice()?;

    // soffice --convert-to writes to --outdir, using the input filename stem + target ext
    // We need to handle the case where --output specifies a different filename
    let output_dir = match output_path.parent() {
        Some(p) if p.as_os_str().is_empty() => std::path::Path::new("."),
        Some(p) => p,
        None => std::path::Path::new("."),
    };

    // Ensure output directory exists
    if !output_dir.exists() {
        std::fs::create_dir_all(output_dir).map_err(|e| {
            HandlerError::OperationFailed(format!("cannot create output dir: {}", e))
        })?;
    }

    // Determine input extension for PDF-specific filter
    let input_ext = std::path::Path::new(input_file)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    // PDF requires writer_pdf_import filter, otherwise soffice silently fails.
    let mut args: Vec<&std::ffi::OsStr> = vec!["--headless".as_ref()];
    if input_ext == "pdf" {
        args.push("--infilter=writer_pdf_import".as_ref());
    }
    args.push("--convert-to".as_ref());
    args.push(target_ext.as_ref());
    args.push("--outdir".as_ref());
    args.push(output_dir.as_os_str());
    args.push(input_file.as_ref());

    // Private soffice profile so concurrent conversions don't block on the
    // shared default profile lock; cleaned up regardless of the outcome.
    let profile_dir = unique_temp_path("officecli-lo-profile");
    let run_res = run_soffice(&soffice, &profile_dir, &args);
    let _ = std::fs::remove_dir_all(&profile_dir);
    run_res?;

    // soffice generates output as <input_stem>.<target_ext> in output_dir
    let input_path_for_stem = PathBuf::from(input_file);
    let input_stem = input_path_for_stem
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output")
        .to_string();
    let soffice_output = output_dir.join(format!("{}.{}", input_stem, target_ext));

    // If soffice output differs from desired output path, rename it
    if soffice_output != *output_path && soffice_output.exists() {
        std::fs::rename(&soffice_output, output_path).map_err(|e| {
            HandlerError::OperationFailed(format!(
                "failed to rename '{}' to '{}': {}",
                soffice_output.display(),
                output_path.display(),
                e
            ))
        })?;
    }

    // Verify the output file was created
    if !output_path.exists() {
        return Err(HandlerError::OperationFailed(format!(
            "soffice did not produce output file '{}'",
            output_path.display()
        )));
    }

    Ok(())
}

/// Find soffice executable, with helpful install hints if missing.
fn find_soffice() -> Result<String, HandlerError> {
    let candidates = [
        "soffice",
        "/usr/bin/soffice",
        "/usr/local/bin/soffice",
        "/Applications/LibreOffice.app/Contents/MacOS/soffice",
    ];

    for candidate in &candidates {
        if std::path::Path::new(candidate).exists() {
            return Ok(candidate.to_string());
        }
    }

    // Try `which` on Unix platforms
    #[cfg(unix)]
    {
        let which_output = std::process::Command::new("which")
            .arg("soffice")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output();
        if let Ok(output) = which_output {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() && std::path::Path::new(&path).exists() {
                return Ok(path);
            }
        }
    }

    Err(HandlerError::OperationFailed(
        "LibreOffice (soffice) not found.\n\nInstall it:\n  macOS:  brew install --cask libreoffice\n  Ubuntu: sudo apt install libreoffice\n  Windows: https://www.libreoffice.org/download/\n\nOr use --engine oxide for pure Rust conversion (lower fidelity)".to_string(),
    ))
}

/// Convert PDF to DOCX via the Python pdf2docx CLI.
fn convert_via_pdf2docx(
    input_file: &str,
    output_path: &std::path::Path,
) -> Result<(), HandlerError> {
    let pdf2docx = find_pdf2docx()?;

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                HandlerError::OperationFailed(format!("cannot create output dir: {}", e))
            })?;
        }
    }

    let output = std::process::Command::new(&pdf2docx)
        .arg("convert")
        .arg(input_file)
        .arg(output_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| HandlerError::OperationFailed(format!("failed to run pdf2docx CLI: {}", e)))?;

    if !output.status.success() {
        return Err(HandlerError::OperationFailed(format!(
            "pdf2docx conversion failed (exit code {}). stdout: {} stderr: {}",
            output.status.code().unwrap_or(-1),
            truncate_lossy(&output.stdout, 2000),
            truncate_lossy(&output.stderr, 2000)
        )));
    }

    if !output_path.exists() {
        return Err(HandlerError::OperationFailed(format!(
            "pdf2docx did not produce output file '{}'",
            output_path.display()
        )));
    }

    Ok(())
}

/// Find the pdf2docx executable. The optional OFFICECLI_PDF2DOCX_BIN env var
/// may point at a full path or at a command available on PATH.
fn find_pdf2docx() -> Result<String, HandlerError> {
    if let Ok(bin) = std::env::var("OFFICECLI_PDF2DOCX_BIN") {
        let bin = bin.trim();
        if !bin.is_empty() {
            return resolve_command(bin).ok_or_else(|| {
                HandlerError::OperationFailed(format!(
                    "OFFICECLI_PDF2DOCX_BIN is set to '{}' but it was not found",
                    bin
                ))
            });
        }
    }

    let candidates = [
        "pdf2docx",
        "/usr/local/bin/pdf2docx",
        "/opt/pdf2docx-venv/bin/pdf2docx",
    ];

    for candidate in candidates {
        if let Some(path) = resolve_command(candidate) {
            return Ok(path);
        }
    }

    Err(HandlerError::OperationFailed(
        "pdf2docx CLI not found.\n\nInstall it:\n  python3 -m pip install pdf2docx\n\nOr set OFFICECLI_PDF2DOCX_BIN=/path/to/pdf2docx".to_string(),
    ))
}

fn resolve_command(candidate: &str) -> Option<String> {
    let is_path = candidate.contains('/') || candidate.contains('\\');
    if is_path {
        return std::path::Path::new(candidate)
            .exists()
            .then(|| candidate.to_string());
    }

    #[cfg(unix)]
    {
        let output = std::process::Command::new("which")
            .arg(candidate)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() && !path.is_empty() {
            return Some(path);
        }
        None
    }

    #[cfg(windows)]
    {
        let output = std::process::Command::new("where")
            .arg(candidate)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let path = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if output.status.success() && !path.is_empty() {
            return Some(path);
        }
        None
    }
}

fn truncate_lossy(bytes: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...<truncated>");
            return out;
        }
        out.push(ch);
    }
    out
}

/// Convert via office_oxide (pure Rust, lower fidelity).
fn convert_via_oxide(input_file: &str, output_path: &std::path::Path) -> Result<(), HandlerError> {
    let doc = office_oxide::Document::open(input_file)
        .map_err(|e| HandlerError::OpenError(format!("failed to open '{}': {}", input_file, e)))?;

    doc.save_as(output_path.to_str().unwrap_or_default())
        .map_err(|e| HandlerError::SaveError(format!("failed to convert: {}", e)))?;

    Ok(())
}

/// Whether `ext` is a native Office extension the in-tree handlers speak.
fn is_native_office_ext(ext: &str) -> bool {
    matches!(ext, "docx" | "xlsx" | "pptx")
}

/// Map a legacy/foreign extension to its native sibling.
fn family_from_foreign(ext: &str) -> &'static str {
    match ext {
        "doc" => "docx",
        "xls" => "xlsx",
        "ppt" => "pptx",
        _ => "docx",
    }
}

/// Try to delegate the conversion to an installed exporter plugin. Returns
/// `Ok(None)` when no plugin is registered for `(input_ext, output_ext)`;
/// the caller falls back to the built-in engines.
fn try_plugin_export(
    _input_ext: &str,
    output_ext: &str,
    input_file: &str,
    output_path: &std::path::Path,
) -> Result<Option<super::plugin_process::ExportResult>, HandlerError> {
    match super::plugin_process::run_exporter(
        input_file,
        output_ext,
        &output_path.to_string_lossy(),
    ) {
        Ok(res) => Ok(Some(res)),
        Err(HandlerError::UnsupportedMode(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Try to delegate foreign-source migration to a dump-reader plugin. Writes
/// the native sibling file into a temporary path next to `input_file`. Returns
/// `Ok(None)` when no plugin is registered for `input_ext`.
fn try_plugin_dump(
    input_ext: &str,
    input_file: &str,
) -> Result<Option<super::plugin_process::DumpResult>, HandlerError> {
    if super::plugin_process::resolve_dump_reader(input_ext).is_none() {
        return Ok(None);
    }
    // Derive a sibling output path. The dump-reader's manifest `target`
    // selects the family; we don't know it without consulting the manifest,
    // so we guess from `input_ext` (`.doc` → `.docx`, `.xls` → `.xlsx`,
    // `.ppt` → `.pptx`). If wrong, the plugin's manifest target will
    // already have steered the skeleton creation, so the on-disk file is
    // correct — only our guess at the sibling name is wrong.
    let input_path = PathBuf::from(input_file);
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted")
        .to_string();
    let dir = input_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let family_guess = match input_ext {
        "doc" => "docx",
        "xls" => "xlsx",
        "ppt" => "pptx",
        _ => "docx",
    };
    let sibling = dir.join(format!("{}.{}", stem, family_guess));
    match super::plugin_process::run_dump_reader(input_file, &sibling.to_string_lossy()) {
        Ok(r) => Ok(Some(r)),
        Err(HandlerError::UnsupportedMode(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Validate that the conversion is supported.
fn validate_conversion(
    input_ext: &str,
    output_ext: &str,
    engine: ConvertEngine,
) -> Result<(), HandlerError> {
    if matches!(input_ext, "html" | "htm") {
        return if matches!(output_ext, "docx" | "xlsx" | "pptx" | "pdf") {
            Ok(())
        } else {
            Err(HandlerError::UnsupportedMode(format!(
                "HTML can convert to .docx, .xlsx, .pptx, or .pdf, not .{output_ext}"
            )))
        };
    }
    // Cross-family: PDF -> DOCX (PDF-specific engines only)
    // Foreign target extensions are valid if a plugin exporter exists.
    // Skip the family-rule check below — those only cover native→native.
    if !is_native_office_ext(output_ext) {
        if super::plugin_process::resolve_exporter(input_ext, output_ext).is_some() {
            return Ok(());
        }
        // No direct exporter for (input_ext, output_ext) — but a
        // foreign→foreign pipeline through a dump-reader + exporter may
        // still succeed. Allow validate to pass; the dispatcher in
        // handle_convert handles the case where the pipeline is missing.
        if !is_native_office_ext(input_ext)
            && super::plugin_process::resolve_dump_reader(input_ext).is_some()
        {
            return Ok(());
        }
        return Err(HandlerError::UnsupportedMode(format!(
            "no plugin exporter handles .{} → .{} — install one or see plugins/plugin-protocol.md",
            input_ext, output_ext
        )));
    }

    if input_ext == "pdf" && output_ext == "docx" {
        if engine != ConvertEngine::LibreOffice
            && engine != ConvertEngine::PdfText
            && engine != ConvertEngine::Pdf2Docx
        {
            return Err(HandlerError::UnsupportedMode(
                "PDF to DOCX conversion requires --engine libreoffice, --engine pdf-text, or --engine pdf2docx"
                    .to_string(),
            ));
        }
        return Ok(());
    }
    if engine == ConvertEngine::PdfText || engine == ConvertEngine::Pdf2Docx {
        let engine_name = if engine == ConvertEngine::PdfText {
            "pdf-text"
        } else {
            "pdf2docx"
        };
        return Err(HandlerError::UnsupportedMode(format!(
            "--engine {} is only supported for .pdf -> .docx",
            engine_name
        )));
    }
    if input_ext == "pdf" && output_ext != "docx" {
        return Err(HandlerError::UnsupportedMode(format!(
            "cannot convert .pdf to .{}; .pdf files can only convert to .docx",
            output_ext
        )));
    }

    // Same-family conversions
    let families = [
        (&["doc", "docx"][..], "docx"),
        (&["xls", "xlsx"][..], "xlsx"),
        (&["ppt", "pptx"][..], "pptx"),
    ];

    for (members, target) in families {
        if members.contains(&input_ext) {
            if output_ext != target {
                return Err(HandlerError::UnsupportedMode(format!(
                    "cannot convert .{} to .{}; .{} files must convert to .{}",
                    input_ext, output_ext, input_ext, target
                )));
            }
            return Ok(());
        }
    }

    Err(HandlerError::UnsupportedMode(format!(
        "unsupported input format: .{}",
        input_ext
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_valid_doc_to_docx() {
        assert!(validate_conversion("doc", "docx", ConvertEngine::LibreOffice).is_ok());
    }

    #[test]
    fn test_valid_xls_to_xlsx() {
        assert!(validate_conversion("xls", "xlsx", ConvertEngine::LibreOffice).is_ok());
    }

    #[test]
    fn test_valid_ppt_to_pptx() {
        assert!(validate_conversion("ppt", "pptx", ConvertEngine::LibreOffice).is_ok());
    }

    #[test]
    fn test_valid_docx_resave() {
        assert!(validate_conversion("docx", "docx", ConvertEngine::Oxide).is_ok());
    }

    #[test]
    fn test_valid_pdf_to_docx() {
        assert!(validate_conversion("pdf", "docx", ConvertEngine::LibreOffice).is_ok());
    }

    #[test]
    fn test_valid_pdf_to_docx_with_pdf_text_engine() {
        assert!(validate_conversion("pdf", "docx", ConvertEngine::PdfText).is_ok());
    }

    #[test]
    fn test_valid_pdf_to_docx_with_pdf2docx_engine() {
        assert!(validate_conversion("pdf", "docx", ConvertEngine::Pdf2Docx).is_ok());
    }

    #[test]
    fn test_valid_html_targets() {
        for target in ["docx", "xlsx", "pptx", "pdf"] {
            assert!(validate_conversion("html", target, ConvertEngine::LibreOffice).is_ok());
        }
        assert!(validate_conversion("html", "txt", ConvertEngine::LibreOffice).is_err());
    }

    #[test]
    fn test_pdf_to_docx_rejects_oxide() {
        let result = validate_conversion("pdf", "docx", ConvertEngine::Oxide);
        assert!(result.is_err());
    }

    #[test]
    fn test_pdf_text_engine_is_pdf_only() {
        let result = validate_conversion("doc", "docx", ConvertEngine::PdfText);
        assert!(result.is_err());
    }

    #[test]
    fn test_pdf2docx_engine_is_pdf_only() {
        let result = validate_conversion("doc", "docx", ConvertEngine::Pdf2Docx);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_cross_family() {
        let result = validate_conversion("doc", "xlsx", ConvertEngine::LibreOffice);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, HandlerError::UnsupportedMode(_)));
    }

    #[test]
    fn test_unsupported_input_format() {
        let result = validate_conversion("odt", "docx", ConvertEngine::LibreOffice);
        assert!(result.is_err());
    }

    #[test]
    fn test_engine_from_str() {
        assert_eq!(
            ConvertEngine::from_str("libreoffice").unwrap(),
            ConvertEngine::LibreOffice
        );
        assert_eq!(
            ConvertEngine::from_str("lo").unwrap(),
            ConvertEngine::LibreOffice
        );
        assert_eq!(
            ConvertEngine::from_str("oxide").unwrap(),
            ConvertEngine::Oxide
        );
        assert_eq!(
            ConvertEngine::from_str("pdf-text").unwrap(),
            ConvertEngine::PdfText
        );
        assert_eq!(
            ConvertEngine::from_str("pdf2docx").unwrap(),
            ConvertEngine::Pdf2Docx
        );
        assert!(ConvertEngine::from_str("foo").is_err());
    }
}
