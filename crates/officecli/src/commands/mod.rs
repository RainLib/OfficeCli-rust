mod view;
mod get;
mod query;
mod set;
mod add;
mod remove;
mod move_element;
mod raw;
mod raw_set;
mod validate;
mod save;
mod extract_text;
mod create;
mod dump;
mod batch;
mod info;

use handler_common::HandlerError;
use clap::Args;

pub use view::ViewCommand;
pub use get::GetCommand;
pub use query::QueryCommand;
pub use set::SetCommand;
pub use add::AddCommand;
pub use remove::RemoveCommand;
pub use move_element::MoveCommand;
pub use raw::RawCommand;
pub use raw_set::RawSetCommand;
pub use validate::ValidateCommand;
pub use save::SaveCommand;
pub use extract_text::ExtractTextCommand;
pub use create::CreateCommand;
pub use dump::DumpCommand;
pub use batch::BatchCommand;
pub use info::InfoCommand;

// ─── Resident / Watch / MCP commands ───────────────────────────────────

/// Open a document in resident mode (keeps handler in memory for fast subsequent commands)
#[derive(Args)]
pub struct OpenCommand {
    /// Document file path
    pub file: String,
}

/// Close a document in resident mode (stops the background server)
#[derive(Args)]
pub struct CloseCommand {
    /// Document file path
    pub file: String,
}

/// Start a live preview HTTP server for the document
#[derive(Args)]
pub struct WatchCommand {
    /// Document file path
    pub file: String,

    /// Port to serve on (default: 26315)
    #[arg(short, long)]
    pub port: Option<u16>,

    /// Unique ID for this document in shared port mode
    #[arg(short, long)]
    pub id: Option<String>,
}

/// Stop a running watch server for the document
#[derive(Args)]
pub struct UnwatchCommand {
    /// Document file path
    pub file: String,
}

/// Start an MCP stdio server for AI agent integration
#[derive(Args)]
pub struct McpCommand;

#[derive(clap::Subcommand)]
pub enum Command {
    /// View document content (text, outline, annotated, html, svg)
    View(ViewCommand),
    /// Get a specific element by path (e.g. '/page[1]/text[1]', '/body/p[2]')
    Get(GetCommand),
    /// Query elements by type (paragraph, table, image, text-block, page)
    Query(QueryCommand),
    /// Set properties on a specific element (text, font, size, color, style)
    Set(SetCommand),
    /// Add a new element (paragraph, table, slide, image)
    Add(AddCommand),
    /// Remove an element at a path
    Remove(RemoveCommand),
    /// Move an element to a new position
    Move(MoveCommand),
    /// View raw XML/PDF content of a part
    Raw(RawCommand),
    /// Modify raw XML/PDF content
    RawSet(RawSetCommand),
    /// Validate document structure
    Validate(ValidateCommand),
    /// Save changes back to the file
    Save(SaveCommand),
    /// Extract text with offset→path mapping for AI agent positioning
    ExtractText(ExtractTextCommand),
    /// Create a blank document (docx, xlsx, pptx, pdf)
    Create(CreateCommand),
    /// Dump document structure to JSON
    Dump(DumpCommand),
    /// Run commands from a batch file
    Batch(BatchCommand),
    /// Show info about the tool or document topics
    Info(InfoCommand),
    /// Open a document in resident mode (keeps handler in memory for fast subsequent commands)
    Open(OpenCommand),
    /// Close a document in resident mode (stops the background server)
    Close(CloseCommand),
    /// Start a live preview HTTP server for the document
    Watch(WatchCommand),
    /// Stop a running watch server for the document
    Unwatch(UnwatchCommand),
    /// Start an MCP stdio server for AI agent integration
    Mcp(McpCommand),
}

// Re-export handler functions
pub use view::handle_view;
pub use get::handle_get;
pub use query::handle_query;
pub use set::handle_set;
pub use add::handle_add;
pub use remove::handle_remove;
pub use move_element::handle_move;
pub use raw::handle_raw;
pub use raw_set::handle_raw_set;
pub use validate::handle_validate;
pub use save::handle_save;
pub use extract_text::handle_extract_text;
pub use create::handle_create;
pub use dump::handle_dump;
pub use batch::handle_batch;
pub use info::handle_info;

/// Helper: open a handler from file path
fn open_handler(file: &str, editable: bool) -> Result<Box<dyn handler_common::DocumentHandler>, HandlerError> {
    crate::open_handler(file, editable)
}