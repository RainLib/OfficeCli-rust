use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Reorder an element within the document
#[derive(Args)]
pub struct MoveCommand {
    pub file: String,
    pub source: String,
    #[arg(long, alias = "to")]
    pub target: Option<String>,
    /// Legacy compact position syntax: index, after:/path, or before:/path.
    #[arg(long)]
    pub position: Option<String>,
    /// Insert at a 0-based sibling index (C#-compatible spelling).
    #[arg(long)]
    pub index: Option<usize>,
    /// Move after this sibling path (C#-compatible spelling).
    #[arg(long)]
    pub after: Option<String>,
    /// Move before this sibling path (C#-compatible spelling).
    #[arg(long)]
    pub before: Option<String>,
}

pub fn handle_move(cmd: MoveCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;
    let pos = crate::commands::add::resolve_position_fields(
        cmd.position.as_deref(),
        cmd.index,
        cmd.after.as_deref(),
        cmd.before.as_deref(),
    )?;
    // C# reorders in the source's parent when --to is omitted. An explicit
    // anchor is more specific, so use its parent for anchored moves.
    let inferred_target = cmd
        .after
        .as_deref()
        .or(cmd.before.as_deref())
        .and_then(parent_path)
        .or_else(|| parent_path(&cmd.source));
    let target = cmd.target.as_deref().or(inferred_target.as_deref());
    let result = handler.move_element(&cmd.source, target, pos)?;
    handler.save()?;
    Ok(format!("Moved to {}", result))
}

fn parent_path(path: &str) -> Option<String> {
    let slash = path.rfind('/')?;
    if slash == 0 {
        Some("/".to_string())
    } else {
        Some(path[..slash].to_string())
    }
}
