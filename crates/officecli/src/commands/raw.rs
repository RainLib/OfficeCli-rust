use clap::Args;
use handler_common::{HandlerError, OutputFormat, RawOptions};

/// View raw XML or PDF content stream of a document part
#[derive(Args)]
pub struct RawCommand {
    pub file: String,
    pub part_path: String,
    #[arg(long)]
    pub start_row: Option<usize>,
    #[arg(long)]
    pub end_row: Option<usize>,
    #[arg(long)]
    pub cols: Option<String>,
}

pub fn handle_raw(cmd: RawCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, false)?;
    let opts = RawOptions {
        start_row: cmd.start_row,
        end_row: cmd.end_row,
        cols: cmd
            .cols
            .map(|c| c.split(',').map(|s| s.to_string()).collect()),
    };
    handler.raw(&cmd.part_path, opts)
}
