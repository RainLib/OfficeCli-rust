use clap::Args;
use handler_common::{DocumentHandler, DocumentNode, HandlerError, OutputFormat};
use std::collections::HashSet;

/// Find all elements of a given type (paragraph, table, image, page, text-block)
#[derive(Args)]
pub struct QueryCommand {
    /// Document file path
    pub file: String,

    /// CSS-like selector (e.g. "p[@style=Normal]", "shape[@id=5]")
    pub selector: String,

    /// Filter results to elements containing this text (case-insensitive substring).
    /// Use r"..." or r'...' for a case-insensitive regular expression.
    #[arg(long)]
    pub find: Option<String>,

    /// Render a stable one-line-per-node text format (DOCX/PPTX only).
    #[arg(long)]
    pub compact: bool,

    /// Comma-separated format keys appended as k=value columns in --compact output.
    #[arg(long)]
    pub fields: Option<String>,
}

pub fn handle_query(cmd: QueryCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    if cmd.compact && matches!(format, OutputFormat::Json) {
        return Err(HandlerError::InvalidArgument(
            "--compact is a plain-text line format; drop --json (or drop --compact for the JSON tree)"
                .to_string(),
        ));
    }
    let nodes = handler.query(&cmd.selector)?;
    let mut nodes = if let Some(find) = cmd.find.as_deref() {
        let mut filtered = Vec::with_capacity(nodes.len());
        for node in nodes {
            let matches = match node.text.as_deref() {
                Some(text) => handler_common::matches_text_filter(text, find).map_err(|error| {
                    HandlerError::InvalidArgument(format!(
                        "invalid regex pattern in '{}': {}",
                        find, error
                    ))
                })?,
                None => false,
            };
            if matches {
                filtered.push(node);
            }
        }
        filtered
    } else {
        nodes
    };

    if cmd.compact {
        return format_nodes_compact(handler.as_ref(), nodes, cmd.fields.as_deref());
    }

    if matches!(format, OutputFormat::Json) {
        // C# query JSON exposes the same first child layer as get --depth 1.
        // Query implementations may intentionally build shallow nodes, so
        // hydrate only those that advertise children but omitted them.
        for node in &mut nodes {
            if node.child_count > 0 && node.children.is_empty() && !node.path.is_empty() {
                if let Ok(hydrated) = handler.get(&node.path, 1) {
                    if !hydrated.children.is_empty() {
                        node.children = hydrated.children;
                    }
                }
            }
        }
    }

    match format {
        OutputFormat::Text => {
            let lines: Vec<String> = nodes
                .iter()
                .map(|n| format!("{} ({})", n.path, n.element_type))
                .collect();
            if lines.is_empty() {
                let extension = std::path::Path::new(&cmd.file)
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or("document");
                eprintln!(
                    "No matches. Run 'officecli {} query' for selector syntax.",
                    extension
                );
            }
            Ok(lines.join("\n"))
        }
        OutputFormat::Json => crate::commands::nodes_json_envelope(&nodes),
    }
}

/// Render C#'s compact query protocol. Its column order and total line are a
/// scripting contract, so changes here must be additive only.
fn format_nodes_compact(
    handler: &dyn DocumentHandler,
    mut nodes: Vec<DocumentNode>,
    fields: Option<&str>,
) -> Result<String, HandlerError> {
    if handler.format_name() == "xlsx" {
        return Err(HandlerError::InvalidArgument(
            "--compact is not supported for xlsx: 'view text' is already the compact per-row form ([/Sheet1/row[N]] A1=v ...). Use 'view text' or 'view text --range Sheet1!A1:C10'."
                .to_string(),
        ));
    }

    let fields = fields
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if handler.format_name() == "pptx" {
        nodes.sort_by_key(|node| (slide_index(&node.path), z_order(node), node.path.clone()));
    }

    let mut lines = Vec::with_capacity(nodes.len() + 1);
    for node in &nodes {
        let mut line = node.path.clone();
        let rows = node_format(node, "rows");
        let cols = node_format(node, "cols");
        if node.element_type == "table" && rows.is_some() && cols.is_some() {
            line.push_str(&format!("\t[table {}x{}]", rows.unwrap(), cols.unwrap()));
        } else {
            let label = compact_label(handler, node);
            line.push_str(&format!(
                "\t[{}]\t{}",
                label,
                compact_text(node.text.as_deref())
            ));
        }
        for field in &fields {
            line.push('\t');
            line.push_str(field);
            line.push('=');
            if let Some(value) = node_format(node, field) {
                line.push_str(&value);
            }
        }
        lines.push(line);
    }

    let (total, suffix) = compact_denominator(handler, nodes.len())?;
    lines.push(format!(
        "total: {} of {} elements{}",
        nodes.len(),
        total,
        suffix
    ));
    Ok(lines.join("\n"))
}

fn compact_label(handler: &dyn DocumentHandler, node: &DocumentNode) -> String {
    if let Some(style) = node.style.as_deref() {
        return style.to_string();
    }
    // Word query nodes intentionally omit rich formatting for speed. C#'s
    // compact format has a fixed style-name label, so hydrate only here.
    if handler.format_name() == "docx" {
        if let Ok(node) = handler.get(&node.path, 0) {
            if let Some(style) = node.style {
                return style;
            }
        }
    }
    node.element_type.clone()
}

fn node_format(node: &DocumentNode, key: &str) -> Option<String> {
    node.format
        .get(key)
        .and_then(|value| value.as_ref())
        .map(|value| match value.as_str() {
            Some(value) => value.to_string(),
            None => value.to_string(),
        })
}

fn compact_text(text: Option<&str>) -> String {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return "(empty)".to_string();
    };
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "")
        .replace('\n', "\\n")
        .replace('"', "\\\"");
    let mut shortened = escaped.chars().take(60).collect::<String>();
    if escaped.chars().count() > 60 {
        shortened.push('…');
    }
    format!("\"{}\"", shortened)
}

fn slide_index(path: &str) -> usize {
    path.strip_prefix("/slide[")
        .and_then(|path| path.split(']').next())
        .and_then(|index| index.parse().ok())
        .unwrap_or(usize::MAX)
}

fn z_order(node: &DocumentNode) -> usize {
    node_format(node, "zorder")
        .and_then(|value| value.parse().ok())
        .unwrap_or(usize::MAX)
}

fn compact_denominator(
    handler: &dyn DocumentHandler,
    result_count: usize,
) -> Result<(usize, String), HandlerError> {
    match handler.format_name() {
        "pptx" => {
            let slides = handler.query("slide")?.len();
            let mut paths = HashSet::new();
            for selector in ["shape", "picture", "table", "chart", "connector", "group"] {
                if let Ok(nodes) = handler.query(selector) {
                    paths.extend(nodes.into_iter().map(|node| node.path));
                }
            }
            Ok((paths.len(), format!(" / {} slides", slides)))
        }
        "docx" => {
            let body = handler.get("/body", 1)?;
            Ok((
                body.children
                    .iter()
                    .filter(|node| node.element_type != "section")
                    .count(),
                String::new(),
            ))
        }
        // PDF is a Rust-only extension. It keeps query support and gets a
        // coherent compact total without claiming C# compatibility.
        _ => Ok((result_count, String::new())),
    }
}
