use clap::{Args, Subcommand, ValueEnum};
use handler_common::{HandlerError, OutputFormat};
use hcd_core::{
    hash_file, manifest_at_revision, render_standalone_html_with_transform, Bundle, FidelityLevel,
    FidelityReport, FidelityWarning, HcdError, HcdManifest, HtmlPresentationOptions, PatchBatch,
    HCD_SCHEMA_VERSION,
};
use hcd_formats::{ExportOptions, ImportOptions};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_SEMANTIC_ASSET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SEMANTIC_ASSETS_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Args)]
pub struct HdocCommand {
    #[command(subcommand)]
    pub command: HdocSubcommand,
}

#[derive(Subcommand)]
pub enum HdocSubcommand {
    /// Stream a DOCX/XLSX/PPTX/PDF/HTML/Markdown/TXT into an immutable, chunked HCD directory
    Import(HdocImportCommand),
    /// Validate an HCD bundle and all content hashes
    Validate(HdocValidateCommand),
    /// Extract one cursor page of node-local text and source anchors
    ExtractText(HdocExtractTextCommand),
    /// Resolve one current text node by stable HCD nodeId
    GetNode(HdocGetNodeCommand),
    /// Materialize the current chunk sequence as a standalone inspection HTML file
    RenderHtml(HdocRenderHtmlCommand),
    /// Apply a text/annotation patch and append a revision
    Apply(HdocApplyCommand),
    /// Export one HCD revision by source-backed rewrite or pure-Rust semantic conversion
    Export(HdocExportCommand),
}

#[derive(Args)]
pub struct HdocImportCommand {
    /// Immutable source DOCX, XLSX, PPTX, PDF, HTML, UTF-8 Markdown, or UTF-8 TXT
    pub source: String,
    /// New HCD directory; must not already exist
    #[arg(short, long)]
    pub output: String,
    /// Stable business document id; derived from the immutable source SHA-256 when omitted
    #[arg(long)]
    pub document_id: Option<String>,
    /// Emit import_started/chunk_ready/asset_ready/completed as NDJSON
    #[arg(long, value_name = "ndjson")]
    pub events: Option<String>,
    /// Soft HTML chunk byte limit
    #[arg(long, default_value_t = hcd_core::DEFAULT_CHUNK_SOFT_BYTES)]
    pub chunk_bytes: usize,
    /// Maximum top-level blocks per ordinary chunk
    #[arg(long, default_value_t = hcd_core::DEFAULT_CHUNK_BLOCKS)]
    pub chunk_blocks: usize,
}

#[derive(Args)]
pub struct HdocValidateCommand {
    pub bundle: String,
}

#[derive(Args)]
pub struct HdocExtractTextCommand {
    pub bundle: String,
    #[arg(long)]
    pub cursor: Option<String>,
    #[arg(long, default_value_t = 1000)]
    pub limit: usize,
}

#[derive(Args)]
pub struct HdocGetNodeCommand {
    pub bundle: String,
    /// Stable canonical text node ID (`n_` followed by 32 lowercase hex digits)
    pub node_id: String,
}

#[derive(Args)]
pub struct HdocRenderHtmlCommand {
    pub bundle: String,
    /// Standalone HTML output path; written incrementally without a full-document buffer
    #[arg(short, long)]
    pub output: String,
    /// Revision to render; defaults to the current head
    #[arg(long)]
    pub revision: Option<u64>,
    /// Text-node hover-outline state in the generated preview
    #[arg(long, value_enum, default_value_t = HdocHitboxState::On)]
    pub text_hitboxes: HdocHitboxState,
    /// Image/form-node hover-outline state in the generated preview
    #[arg(long, value_enum, default_value_t = HdocHitboxState::On)]
    pub image_hitboxes: HdocHitboxState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HdocHitboxState {
    On,
    Off,
}

#[derive(Args)]
pub struct HdocApplyCommand {
    pub bundle: String,
    /// Patch JSON path, or '-' for stdin
    #[arg(long)]
    pub patch: String,
    #[arg(long)]
    pub expected_revision: u64,
}

#[derive(Args)]
pub struct HdocExportCommand {
    pub bundle: String,
    /// Immutable source for same-format source-backed EXACT/HIGH export. Omit for semantic export.
    #[arg(long)]
    pub source: Option<String>,
    #[arg(short, long)]
    pub output: String,
    /// Semantic target format; inferred from --output when omitted
    #[arg(long, value_name = "docx|xlsx|pptx|pdf|html|md|txt")]
    pub to: Option<String>,
    #[arg(long)]
    pub revision: Option<u64>,
    #[arg(long)]
    pub fidelity_report: Option<PathBuf>,
}

pub fn handle_hdoc(
    command: HdocCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    match command.command {
        HdocSubcommand::Import(command) => import(command, format),
        HdocSubcommand::Validate(command) => validate(command, format),
        HdocSubcommand::ExtractText(command) => extract_text(command, format),
        HdocSubcommand::GetNode(command) => get_node(command, format),
        HdocSubcommand::RenderHtml(command) => render_html(command, format),
        HdocSubcommand::Apply(command) => apply(command, format),
        HdocSubcommand::Export(command) => export(command, format),
    }
}

fn import(
    command: HdocImportCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    let events = command.events.as_deref();
    if events.is_some_and(|value| value != "ndjson") {
        return Err(HandlerError::InvalidArgument(
            "--events currently accepts only 'ndjson'".to_string(),
        ));
    }
    if events.is_some() && format == OutputFormat::Json {
        return Err(HandlerError::InvalidArgument(
            "--events ndjson cannot be combined with global --json".to_string(),
        ));
    }
    let document_id = match command.document_id {
        Some(document_id) => document_id,
        None => deterministic_document_id(&command.source)?,
    };
    let mut options = ImportOptions::new(document_id);
    options.chunk_soft_bytes = command.chunk_bytes;
    options.chunk_blocks = command.chunk_blocks;
    let manifest =
        hcd_formats::import_document(&command.source, &command.output, &options, |event| {
            if events.is_some() {
                println!("{}", serde_json::to_string(event).map_err(HcdError::from)?);
            }
            Ok(())
        })
        .map_err(handler_error)?;
    if events.is_some() {
        Ok((String::new(), true))
    } else {
        render(
            &manifest,
            format,
            format!(
                "Imported {} as {} chunks at revision 0",
                command.source, manifest.chunk_count
            ),
        )
    }
}

fn validate(
    command: HdocValidateCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    let bundle = Bundle::open(&command.bundle).map_err(handler_error)?;
    let report = hcd_core::validate_bundle(&bundle).map_err(handler_error)?;
    let valid = report.valid;
    if format == OutputFormat::Json {
        return Ok((
            crate::commands::json_data_envelope(serde_json::to_value(&report)?, valid),
            valid,
        ));
    }
    render(
        &report,
        format,
        if valid {
            "HCD validation passed".to_string()
        } else {
            format!(
                "HCD validation failed with {} issue(s)",
                report.issues.len()
            )
        },
    )
    .map(|(output, _)| (output, valid))
}

fn extract_text(
    command: HdocExtractTextCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    let bundle = Bundle::open(&command.bundle).map_err(handler_error)?;
    let page = hcd_core::extract_text_page(&bundle, command.cursor.as_deref(), command.limit)
        .map_err(handler_error)?;
    let text = page
        .entries
        .iter()
        .map(|entry| entry.text.as_str())
        .collect::<Vec<_>>()
        .join("");
    render(&page, format, text)
}

fn get_node(
    command: HdocGetNodeCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    let bundle = Bundle::open(&command.bundle).map_err(handler_error)?;
    let node = hcd_core::get_text_node(&bundle, &command.node_id).map_err(handler_error)?;
    let text = node.node.text.clone();
    render(&node, format, text)
}

fn render_html(
    command: HdocRenderHtmlCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    let bundle = Bundle::open(&command.bundle).map_err(handler_error)?;
    let validation = hcd_core::validate_bundle(&bundle).map_err(handler_error)?;
    if !validation.valid {
        return Err(HandlerError::ValidationError(format!(
            "cannot render an invalid HCD bundle with {} issue(s)",
            validation.issues.len()
        )));
    }
    let output = Path::new(&command.output);
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let asset_base_href = relative_directory_href(parent, bundle.root())?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".officecli-hcd-preview-")
        .suffix(".html")
        .tempfile_in(parent)?;
    let report = render_standalone_html_with_transform(
        &bundle,
        &HtmlPresentationOptions {
            revision: command.revision,
            asset_base_href: Some(asset_base_href),
            text_hitboxes_enabled: command.text_hitboxes == HdocHitboxState::On,
            image_hitboxes_enabled: command.image_hitboxes == HdocHitboxState::On,
            ..HtmlPresentationOptions::default()
        },
        temporary.as_file_mut(),
        hcd_formats::enhance_presentation_fragment,
    )
    .map_err(handler_error)?;
    temporary.as_file_mut().flush()?;
    temporary.persist(output).map_err(|error| error.error)?;
    let result = serde_json::json!({
        "documentId": report.document_id,
        "revision": report.revision,
        "profile": report.profile,
        "chunkCount": report.chunk_count,
        "bytes": report.bytes_written,
        "output": output,
    });
    render(
        &result,
        format,
        format!(
            "Rendered HCD revision {} as {} chunks to {}",
            report.revision,
            report.chunk_count,
            output.display()
        ),
    )
}

fn relative_directory_href(from: &Path, to: &Path) -> Result<String, HandlerError> {
    let from = std::fs::canonicalize(from)?;
    let to = std::fs::canonicalize(to)?;
    let from_components = from.components().collect::<Vec<_>>();
    let to_components = to.components().collect::<Vec<_>>();
    let common = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(HandlerError::InvalidArgument(format!(
            "preview output and HCD bundle do not share a filesystem root: {} and {}",
            from.display(),
            to.display()
        )));
    }
    let mut parts = vec!["..".to_string(); from_components.len() - common];
    parts.extend(
        to_components[common..].iter().map(|component| {
            percent_encode_href_component(&component.as_os_str().to_string_lossy())
        }),
    );
    let mut href = if parts.is_empty() {
        "./".to_string()
    } else {
        format!("{}/", parts.join("/"))
    };
    if href == "/" {
        href = "./".to_string();
    }
    Ok(href)
}

fn percent_encode_href_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn deterministic_document_id(source: &str) -> Result<String, HandlerError> {
    let source_hash = hash_file(source).map_err(handler_error)?;
    Ok(format!("doc-{}", &source_hash[..32]))
}

fn apply(command: HdocApplyCommand, format: OutputFormat) -> Result<(String, bool), HandlerError> {
    let patch_json = read_patch_json(&command.patch)?;
    let patch: PatchBatch = serde_json::from_str(&patch_json)?;
    let bundle = Bundle::open(&command.bundle).map_err(handler_error)?;
    let result =
        hcd_core::apply_patch(&bundle, &patch, command.expected_revision).map_err(handler_error)?;
    render(
        &result,
        format,
        format!(
            "Applied patch {}: revision {} ({} dirty chunk(s))",
            result.patch_id,
            result.revision,
            result.dirty_chunk_ids.len()
        ),
    )
}

fn read_patch_json(source: &str) -> Result<String, HandlerError> {
    if source == "-" {
        let stdin = std::io::stdin();
        return read_patch_json_from(stdin.lock(), "stdin");
    }
    let file = std::fs::File::open(source)?;
    read_patch_json_from(file, source)
}

fn read_patch_json_from(mut reader: impl Read, source: &str) -> Result<String, HandlerError> {
    let maximum = hcd_core::MAX_PATCH_JSON_BYTES;
    let mut bytes = Vec::with_capacity((maximum.min(64 * 1024)) as usize);
    reader
        .by_ref()
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(HandlerError::InvalidArgument(format!(
            "HCD patch JSON from {source} exceeds the {maximum} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(|error| {
        HandlerError::InvalidArgument(format!(
            "HCD patch JSON from {source} is not UTF-8: {error}"
        ))
    })
}

fn export(
    command: HdocExportCommand,
    format: OutputFormat,
) -> Result<(String, bool), HandlerError> {
    let bundle = Bundle::open(&command.bundle).map_err(handler_error)?;
    let head = bundle.manifest().map_err(handler_error)?;
    let target = semantic_target(&command.output, command.to.as_deref())?;
    let source_format = normalize_format(&head.source.format);
    let options = ExportOptions {
        revision: command.revision,
        fidelity_report: command.fidelity_report.clone(),
    };
    let report = match command.source.as_deref() {
        Some(source) if target == source_format => {
            hcd_formats::export_document(&command.bundle, source, &command.output, &options)
                .map_err(handler_error)?
        }
        _ => semantic_export(
            &bundle,
            &head,
            &command.output,
            &target,
            command.revision,
            command.fidelity_report.as_deref(),
        )?,
    };
    render(
        &report,
        format,
        format!(
            "Exported revision to {} ({:?} fidelity)",
            command.output, report.level
        ),
    )
}

fn semantic_target(output: &str, requested: Option<&str>) -> Result<String, HandlerError> {
    let extension = Path::new(output)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(normalize_format)
        .unwrap_or_default();
    let requested = requested.map(normalize_format);
    if let Some(requested) = requested.as_deref() {
        if requested != extension {
            return Err(HandlerError::InvalidArgument(format!(
                "--to {requested} does not match --output extension .{extension}"
            )));
        }
    }
    let target = requested.unwrap_or(extension);
    if !matches!(
        target.as_str(),
        "docx" | "xlsx" | "pptx" | "pdf" | "html" | "md" | "txt"
    ) {
        return Err(HandlerError::UnsupportedMode(format!(
            "HCD export supports .docx, .xlsx, .pptx, .pdf, .html, .md/.markdown, or .txt; found .{target}"
        )));
    }
    Ok(target)
}

fn normalize_format(value: &str) -> String {
    match value
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "htm" => "html".to_string(),
        "markdown" => "md".to_string(),
        format => format.to_string(),
    }
}

fn semantic_export(
    bundle: &Bundle,
    head: &HcdManifest,
    output: &str,
    target: &str,
    requested_revision: Option<u64>,
    fidelity_report: Option<&Path>,
) -> Result<FidelityReport, HandlerError> {
    if !matches!(target, "docx" | "xlsx" | "pptx" | "pdf" | "md" | "txt") {
        return Err(HandlerError::UnsupportedMode(format!(
            "source-free semantic export supports .docx, .xlsx, .pptx, .pdf, .md, or .txt; .{target} requires its immutable source"
        )));
    }
    let validation = hcd_core::validate_bundle(bundle).map_err(handler_error)?;
    if !validation.valid {
        return Err(HandlerError::ValidationError(format!(
            "HCD validation failed with {} issue(s) before semantic export",
            validation.issues.len()
        )));
    }
    let (manifest, revision) =
        manifest_at_revision(bundle, head, requested_revision).map_err(handler_error)?;
    let output_path = Path::new(output);
    let parent = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut materialized = tempfile::Builder::new()
        .prefix(".officecli-hcd-semantic-")
        .suffix(".html")
        .tempfile_in(parent)?;
    let presentation = render_standalone_html_with_transform(
        bundle,
        &HtmlPresentationOptions {
            revision: Some(revision),
            ..HtmlPresentationOptions::default()
        },
        materialized.as_file_mut(),
        |html| {
            if target == "pdf" {
                hcd_formats::enhance_presentation_fragment(html)
            } else {
                Ok(html.to_string())
            }
        },
    )
    .map_err(handler_error)?;
    let chunk_count = presentation.chunk_count;
    materialized.as_file_mut().flush()?;
    let assets = semantic_assets(bundle).map_err(handler_error)?;
    let summary = super::html_convert::convert_html_with_assets(
        materialized.path(),
        output_path,
        target,
        assets,
    )?;
    let image_count = summary.image_count;
    let embedded_image_count = summary.embedded_image_count;
    let conversion_engine = summary.engine;

    let mut warnings = vec![FidelityWarning {
        code: "HCD_CROSS_FORMAT_SEMANTIC_EXPORT".to_string(),
        message: format!(
            "revision {revision} was rebuilt as .{target} by OfficeCLI's in-process Rust {conversion_engine} handler; source-backed opaque parts and exact {profile} source layout were not applied",
            profile = manifest.profile
        ),
        node_id: None,
        source_part: None,
    }];
    warnings.extend(summary.warnings.into_iter().map(|message| FidelityWarning {
        code: "RUST_HTML_SEMANTIC_WARNING".to_string(),
        message,
        node_id: None,
        source_part: None,
    }));
    let mut preserved = vec![
        format!("canonical HCD revision {revision} text in chunk sequence order"),
        format!(
            "semantic headings, paragraphs, lists and tables recognized across {chunk_count} HCD chunks"
        ),
        "in-process Rust output generation and target structure validation".to_string(),
    ];
    if conversion_engine == "rust-html-css-pdf" {
        preserved.push(
            "canonical HCD HTML structure, stylesheet cascade, inline formatting, links, tables and paginated print layout rendered directly to PDF"
                .to_string(),
        );
    }
    if embedded_image_count > 0 {
        preserved.push(format!(
            "{embedded_image_count} of {image_count} content-addressed HCD image assets embedded in the target artifact; bounded source dimensions and direct slide coordinates are applied when present and target-compatible"
        ));
    }
    let report = FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: if conversion_engine == "rust-html-css-pdf" {
            FidelityLevel::High
        } else {
            FidelityLevel::Semantic
        },
        preserved,
        flattened: if conversion_engine == "rust-html-css-pdf" {
            vec![
                format!(
                    "{} source-format physical geometry outside canonical HCD HTML remains distinct from CSS print pagination",
                    manifest.profile
                ),
                "source-format opaque parts, annotations, JavaScript and unsupported browser-only CSS are not embedded in the PDF".to_string(),
            ]
        } else {
            vec![
                format!(
                    "{} profile-specific geometry, styles and pagination are reduced to semantic document defaults",
                    manifest.profile
                ),
                "source-format opaque parts, annotations and unsupported active content are not embedded in the cross-format artifact".to_string(),
            ]
        },
        dropped: Vec::new(),
        warnings,
    };
    if let Some(path) = fidelity_report {
        write_semantic_fidelity_report(path, &report)?;
    }
    Ok(report)
}

fn semantic_assets(
    bundle: &Bundle,
) -> Result<HashMap<String, super::html_convert::SemanticAsset>, HcdError> {
    let records = bundle.read_asset_index()?;
    let mut total = 0u64;
    let mut assets = HashMap::with_capacity(records.len());
    for record in records {
        if record.byte_length > MAX_SEMANTIC_ASSET_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "asset {} is {} bytes; source-free semantic export allows at most {MAX_SEMANTIC_ASSET_BYTES} bytes per asset",
                record.hash, record.byte_length
            )));
        }
        total = total.checked_add(record.byte_length).ok_or_else(|| {
            HcdError::ResourceLimit("semantic asset bytes overflowed".to_string())
        })?;
        if total > MAX_SEMANTIC_ASSETS_TOTAL_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "source-free semantic export assets exceed the {MAX_SEMANTIC_ASSETS_TOTAL_BYTES} byte total limit"
            )));
        }
        let path = bundle.resolve_href(&record.href)?;
        let metadata = std::fs::metadata(&path)?;
        if metadata.len() != record.byte_length {
            return Err(HcdError::InvalidBundle(format!(
                "asset {} changed size after validation",
                record.hash
            )));
        }
        let actual_hash = hash_file(&path)?;
        if actual_hash != record.hash {
            return Err(HcdError::InvalidBundle(format!(
                "asset {} changed content after validation: {actual_hash}",
                record.hash
            )));
        }
        let format = semantic_asset_format(&path)?;
        assets.insert(
            format!("asset://sha256/{}", record.hash),
            super::html_convert::SemanticAsset { path, format },
        );
    }
    Ok(assets)
}

fn semantic_asset_format(path: &Path) -> Result<String, HcdError> {
    let mut file = std::fs::File::open(path)?;
    let mut signature = [0u8; 16];
    let length = file.read(&mut signature)?;
    let bytes = &signature[..length];
    let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "gif"
    } else if bytes.starts_with(b"BM") {
        "bmp"
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        "tiff"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        "ico"
    } else {
        ""
    };
    Ok(format.to_string())
}

fn write_semantic_fidelity_report(
    path: &Path,
    report: &FidelityReport,
) -> Result<(), HandlerError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".officecli-hcd-fidelity-")
        .suffix(".json")
        .tempfile_in(parent)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), report)?;
    temporary.as_file_mut().flush()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn render<T: serde::Serialize>(
    value: &T,
    format: OutputFormat,
    text: String,
) -> Result<(String, bool), HandlerError> {
    Ok((
        match format {
            OutputFormat::Text => text,
            OutputFormat::Json => serde_json::to_string_pretty(value)?,
        },
        true,
    ))
}

fn handler_error(error: HcdError) -> HandlerError {
    match error {
        HcdError::ResourceLimit(message) | HcdError::InvalidPatch(message) => {
            HandlerError::InvalidArgument(message)
        }
        HcdError::InvalidBundle(message) => HandlerError::ValidationError(message),
        HcdError::NodeNotFound(node) => HandlerError::PathNotFound(node),
        HcdError::Unsupported(message) => HandlerError::UnsupportedMode(message),
        HcdError::Io(error) => HandlerError::IoError(error),
        other => HandlerError::OperationFailed(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_reader_stops_at_the_json_byte_limit() {
        let input = std::io::repeat(b'x').take(hcd_core::MAX_PATCH_JSON_BYTES + 1);
        let error = read_patch_json_from(input, "test input").unwrap_err();
        assert!(error.to_string().contains("exceeds the"));
        assert!(error.to_string().contains("byte limit"));
    }
}
