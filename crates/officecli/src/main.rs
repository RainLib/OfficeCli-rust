#![allow(
    clippy::match_result_ok,
    clippy::redundant_closure,
    clippy::let_and_return
)]

mod commands;
mod mcp;
#[cfg(unix)]
mod resident;
#[cfg(not(unix))]
mod resident {
    pub struct IpcResponse {
        pub error: Option<String>,
    }
    pub async fn run_server(_file_path: &str) -> Result<(), anyhow::Error> {
        Err(anyhow::anyhow!(
            "Resident mode is not supported on this platform"
        ))
    }
    pub fn spawn_server(_file_path: &str) -> Result<(), anyhow::Error> {
        Err(anyhow::anyhow!(
            "Resident mode is not supported on this platform"
        ))
    }
    pub async fn close_server(_file_path: &str) -> Result<IpcResponse, anyhow::Error> {
        Err(anyhow::anyhow!(
            "Resident mode is not supported on this platform"
        ))
    }
}
mod watch;

use clap::Parser;
use handler_common::{DocumentHandler, HandlerError, OutputFormat};
use std::path::PathBuf;

/// OfficeCLI — CLI tool for Office documents (docx/xlsx/pptx) and PDF
#[derive(Parser)]
#[command(name = "officecli")]
#[command(version = "0.1.0")]
#[command(about = "Create, view, query, and modify Office documents and PDFs")]
#[command(after_help = "\
EXAMPLES:
  officecli create demo.docx                  Create a blank Word document
  officecli view demo.docx                    View document as plain text
  officecli view demo.docx -m outline         View outline with metadata
  officecli view demo.pdf -m annotated        View PDF with bbox coordinates
  officecli view demo.pdf -m html             Generate HTML layout preview for browser
  officecli get demo.docx '/body/p[1]'        Get a specific paragraph
  officecli set demo.docx '/body/p[1]' text='Hello'  Replace text
  officecli set demo.pdf '/page[1]/text[1]' text='Title' color='#FF0000' bgColor='#FFFF00'
  officecli set demo.pdf '/page[1]/text[1]' fontFile='assets/MyFont.ttf' size=14.5
  officecli query demo.docx paragraph         Find all paragraphs
  officecli extract-text demo.docx            Extract text with offset→path mapping
  officecli extract-text demo.pdf --with-offsets --json  Extract PDF text and offset mapping as JSON")]
struct Cli {
    /// Internal flag: run as resident IPC server (do not use directly)
    #[arg(long, hide = true)]
    resident_serve: Option<String>,

    /// Output as JSON instead of text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<commands::Command>,
}

fn main() {
    // Parse CLI args — if invalid, print full help + error instead of terse usage
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            use clap::CommandFactory;
            if e.kind() == clap::error::ErrorKind::UnknownArgument
                || e.kind() == clap::error::ErrorKind::InvalidSubcommand
                || e.kind() == clap::error::ErrorKind::MissingSubcommand
            {
                // Print full help then the error message
                let _ = Cli::command().print_help();
                eprintln!("\n\n{}", e);
                std::process::exit(1);
            }
            // For other errors (wrong types, etc.), use default clap output
            e.exit();
        }
    };

    // Handle internal resident server mode
    if let Some(file_path) = cli.resident_serve {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        rt.block_on(async {
            if let Err(e) = resident::run_server(&file_path).await {
                eprintln!("Resident server error: {}", e);
                std::process::exit(1);
            }
        });
        return;
    }

    let format = if cli.json {
        OutputFormat::Json
    } else {
        OutputFormat::Text
    };

    let command = cli.command.unwrap_or_else(|| {
        // No subcommand → print full help and exit with error code
        use clap::CommandFactory;
        let mut cmd = Cli::command();
        cmd.print_help().unwrap();
        eprintln!("\nError: a subcommand is required. See above for available commands.");
        std::process::exit(1);
    });

    let result = match command {
        commands::Command::View(cmd) => commands::handle_view(cmd, format),
        commands::Command::Get(cmd) => commands::handle_get(cmd, format),
        commands::Command::Query(cmd) => commands::handle_query(cmd, format),
        commands::Command::Set(cmd) => commands::handle_set(cmd, format),
        commands::Command::Add(cmd) => commands::handle_add(cmd, format),
        commands::Command::Remove(cmd) => commands::handle_remove(cmd, format),
        commands::Command::Move(cmd) => commands::handle_move(cmd, format),
        commands::Command::Raw(cmd) => commands::handle_raw(cmd, format),
        commands::Command::RawSet(cmd) => commands::handle_raw_set(cmd, format),
        commands::Command::Validate(cmd) => commands::handle_validate(cmd, format),
        commands::Command::Save(cmd) => commands::handle_save(cmd, format),
        commands::Command::ExtractText(cmd) => commands::handle_extract_text(cmd, format),
        commands::Command::Create(cmd) => commands::handle_create(cmd, format),
        commands::Command::Dump(cmd) => commands::handle_dump(cmd, format),
        commands::Command::Batch(cmd) => commands::handle_batch(cmd, format),
        commands::Command::Info(cmd) => commands::handle_info(cmd, format),
        commands::Command::Open(cmd) => handle_open(cmd),
        commands::Command::Close(cmd) => handle_close(cmd),
        commands::Command::Watch(cmd) => handle_watch(cmd),
        commands::Command::Unwatch(cmd) => handle_unwatch(cmd),
        commands::Command::Mcp(_) => handle_mcp(),
    };

    match result {
        Ok(text) => println!("{}", text),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

// ─── Handler functions for resident, watch, and MCP commands ───────────

fn handle_open(cmd: commands::OpenCommand) -> Result<String, HandlerError> {
    resident::spawn_server(&cmd.file).map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    Ok(format!("Resident server started for: {}", cmd.file))
}

fn handle_close(cmd: commands::CloseCommand) -> Result<String, HandlerError> {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        resident::close_server(&cmd.file)
            .await
            .map(|resp| {
                if let Some(error) = resp.error {
                    format!("Error: {}", error)
                } else {
                    format!("Resident server closed for: {}", cmd.file)
                }
            })
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))
    })
}

fn handle_watch(cmd: commands::WatchCommand) -> Result<String, HandlerError> {
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let abs_path = std::fs::canonicalize(&cmd.file)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| {
                if let Ok(curr) = std::env::current_dir() {
                    curr.join(&cmd.file).to_string_lossy().to_string()
                } else {
                    cmd.file.clone()
                }
            });

        watch::run_server(&cmd.file, &abs_path, cmd.port, cmd.id)
            .await
            .map(|_| "Watch server stopped".to_string())
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))
    })
}

fn handle_unwatch(cmd: commands::UnwatchCommand) -> Result<String, HandlerError> {
    // Currently the watch server blocks until Ctrl+C. Unwatch is a placeholder
    // that could send a shutdown signal in a future implementation.
    Ok(format!(
        "Unwatch not yet supported for: {} — use Ctrl+C to stop the watch server",
        cmd.file
    ))
}

fn handle_mcp() -> Result<String, HandlerError> {
    mcp::run_server()
        .map(|_| "MCP server stopped".to_string())
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))
}

/// Open a document handler based on file extension.
fn open_handler(file: &str, editable: bool) -> Result<Box<dyn DocumentHandler>, HandlerError> {
    let path = PathBuf::from(file);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "docx" => {
            let handler = docx_handler::WordHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        "xlsx" => {
            let handler = xlsx_handler::ExcelHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        "pptx" => {
            let handler = pptx_handler::PptxHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        "pdf" => {
            let handler = pdf_handler::PdfHandler::open(file, editable)?;
            Ok(Box::new(handler))
        }
        other => Err(HandlerError::OpenError(format!(
            "unsupported format: {}",
            other
        ))),
    }
}
