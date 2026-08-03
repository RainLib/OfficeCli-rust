use clap::Args;
use handler_common::{HandlerError, OutputFormat};

/// Modify raw XML or PDF content stream
#[derive(Args)]
pub struct RawSetCommand {
    pub file: String,
    pub part_path: String,
    /// Legacy positional XPath.
    pub xpath_legacy: Option<String>,
    /// Legacy positional action.
    pub action_legacy: Option<String>,
    #[arg(long)]
    pub xpath: Option<String>,
    #[arg(long)]
    pub action: Option<String>,
    #[arg(long)]
    pub xml: Option<String>,
}

pub fn handle_raw_set(cmd: RawSetCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let xpath = resolve_required_option("--xpath", cmd.xpath, cmd.xpath_legacy)?;
    let action = resolve_required_option("--action", cmd.action, cmd.action_legacy)?;
    let handler = crate::open_handler(&cmd.file, true)?;
    let part_path = super::raw::normalize_logical_part_path(&cmd.file, &cmd.part_path);
    handler.raw_set(&part_path, &xpath, &action, cmd.xml.as_deref())?;
    handler.save()?;
    Ok("OK".to_string())
}

fn resolve_required_option(
    option: &str,
    explicit: Option<String>,
    legacy: Option<String>,
) -> Result<String, HandlerError> {
    match (explicit, legacy) {
        (Some(_), Some(_)) => Err(HandlerError::InvalidArgument(format!(
            "{} cannot be combined with its positional form.",
            option
        ))),
        (Some(value), None) | (None, Some(value)) => Ok(value),
        (None, None) => Err(HandlerError::InvalidArgument(format!(
            "{} is required.",
            option
        ))),
    }
}
