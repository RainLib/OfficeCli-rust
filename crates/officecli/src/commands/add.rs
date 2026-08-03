use clap::Args;
use handler_common::{HandlerError, InsertPosition, OutputFormat};
use std::collections::HashMap;

/// Insert a new element (paragraph, table, slide, image, bookmark) into the document
#[derive(Args)]
pub struct AddCommand {
    /// Document file path
    pub file: String,

    /// Parent path where to add
    #[arg(long)]
    pub parent: Option<String>,

    /// Parent path where to add (C#-compatible positional form).
    #[arg(index = 2)]
    pub parent_path: Option<String>,

    /// Element type to add
    #[arg(long, alias = "type")]
    pub type_name: Option<String>,

    /// Copy from an existing element path instead of creating a new element.
    #[arg(long)]
    pub from: Option<String>,

    /// Position: index number, "after:/path", or "before:/path"
    #[arg(long)]
    pub position: Option<String>,

    /// Insert at a 0-based child index (C#-compatible spelling).
    #[arg(long)]
    pub index: Option<usize>,

    /// Insert after this sibling path (C#-compatible spelling).
    #[arg(long)]
    pub after: Option<String>,

    /// Insert before this sibling path (C#-compatible spelling).
    #[arg(long)]
    pub before: Option<String>,

    /// Properties (key=value pairs)
    #[arg(long, alias = "prop", num_args = 1..)]
    pub properties: Vec<String>,

    /// Wrap an existing element: bookmarkStart goes before, bookmarkEnd goes after the target
    #[arg(long)]
    pub wrap: Option<String>,

    /// Range-paths for bookmark: insert bookmarkStart/End around text at char offsets
    /// Syntax: /path[start..end],/path[start..end] (same as set --range-paths)
    #[arg(long)]
    pub range_paths: Option<String>,

    /// Emit the refreshed text+offset map after the edit (JSON output only).
    #[arg(long)]
    pub emit_map: bool,
}

pub fn handle_add(cmd: AddCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let parent = resolve_parent(&cmd)?;
    let handler = crate::open_handler(&cmd.file, true)?;

    let position = resolve_position(&cmd)?;
    let mut properties = parse_properties(&cmd.properties);

    if cmd.type_name.is_none() && cmd.from.is_none() {
        return Err(HandlerError::InvalidArgument(
            "Either --type-name or --from must be specified.".to_string(),
        ));
    }
    if cmd.from.is_some() && (!cmd.properties.is_empty() || cmd.range_paths.is_some()) {
        return Err(HandlerError::InvalidArgument(
            "--properties cannot be combined with --from; use `set` on the copied path to modify properties."
                .to_string(),
        ));
    }

    // Merge range_paths into properties (same pattern as set command)
    if let Some(rp) = &cmd.range_paths {
        properties.insert("range_paths".to_string(), rp.clone());
    }

    let (new_path, message) = if let Some(source) = cmd.from.as_deref() {
        let path = handler.copy_from(source, &parent, position)?;
        (path.clone(), format!("Copied to {}", path))
    } else {
        let path = handler.add(
            &parent,
            cmd.type_name.as_deref().expect("validated above"),
            position,
            &properties,
            cmd.wrap.as_deref(),
        )?;
        (path.clone(), format!("Created: {}", path))
    };
    handler.save()?;

    let offset_map = if cmd.emit_map {
        super::offset_map_value(handler.as_ref())
    } else {
        None
    };

    match format {
        OutputFormat::Text => Ok(message.clone()),
        OutputFormat::Json => {
            let mut extensions = serde_json::Map::new();
            extensions.insert(
                "path".to_string(),
                serde_json::Value::String(new_path.clone()),
            );
            if let Some(map) = offset_map {
                extensions.insert("offset_map".to_string(), map);
            }
            Ok(crate::commands::json_text_envelope(&message, extensions))
        }
    }
}

fn resolve_parent(cmd: &AddCommand) -> Result<String, HandlerError> {
    match (&cmd.parent, &cmd.parent_path) {
        (Some(_), Some(_)) => Err(HandlerError::InvalidArgument(
            "Use either positional parent or --parent, not both.".to_string(),
        )),
        (Some(parent), None) | (None, Some(parent)) => Ok(parent.clone()),
        (None, None) => Err(HandlerError::InvalidArgument(
            "Parent path is required. Use `officecli add <file> <parent> ...` or --parent <path>."
                .to_string(),
        )),
    }
}

pub fn parse_position(input: Option<&str>) -> InsertPosition {
    match input {
        None => InsertPosition::Append,
        Some(s) => {
            if let Some(idx) = s.parse::<usize>().ok() {
                InsertPosition::AtIndex(idx)
            } else if let Some(rest) = s.strip_prefix("after:") {
                InsertPosition::AfterElement(rest.to_string())
            } else if let Some(rest) = s.strip_prefix("before:") {
                InsertPosition::BeforeElement(rest.to_string())
            } else {
                InsertPosition::Append
            }
        }
    }
}

/// Resolve the current compact `--position` syntax and the C# CLI's explicit
/// position flags. Keeping `--position` avoids breaking existing Rust CLI
/// users while making the public C# syntax available verbatim.
fn resolve_position(cmd: &AddCommand) -> Result<InsertPosition, HandlerError> {
    resolve_position_fields(
        cmd.position.as_deref(),
        cmd.index,
        cmd.after.as_deref(),
        cmd.before.as_deref(),
    )
}

/// Shared resolver for the C# explicit flags and the Rust legacy compact
/// position field used by add and move.
pub(crate) fn resolve_position_fields(
    position: Option<&str>,
    index: Option<usize>,
    after: Option<&str>,
    before: Option<&str>,
) -> Result<InsertPosition, HandlerError> {
    let explicit_count = usize::from(position.is_some())
        + usize::from(index.is_some())
        + usize::from(after.is_some())
        + usize::from(before.is_some());
    if explicit_count > 1 {
        return Err(HandlerError::InvalidArgument(
            "--index, --after, --before, and --position are mutually exclusive. Use only one."
                .to_string(),
        ));
    }

    Ok(match (index, after, before) {
        (Some(index), None, None) => InsertPosition::AtIndex(index),
        (None, Some(after), None) => InsertPosition::AfterElement(after.to_string()),
        (None, None, Some(before)) => InsertPosition::BeforeElement(before.to_string()),
        _ => parse_position(position),
    })
}

fn parse_properties(props: &[String]) -> HashMap<String, String> {
    props
        .iter()
        .filter_map(|p| {
            let parts: Vec<&str> = p.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect()
}
