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
    pub fn socket_path_for_file(_file: &str) -> std::path::PathBuf {
        // Resident mode is not available on this platform; return a placeholder.
        std::path::PathBuf::from(format!(
            "{}\\officecli\\resident\\unsupported.sock",
            std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string())
        ))
    }
}
mod screenshot;
mod watch;

use clap::Parser;
use handler_common::{DocumentHandler, HandlerError, OutputFormat};
use std::path::PathBuf;

/// OfficeCLI — CLI tool for Office documents (docx/xlsx/pptx) and PDF
#[derive(Parser)]
#[command(name = "officecli")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(disable_help_subcommand = true)]
#[command(about = "Create, view, query, and modify Office documents and PDFs")]
#[command(after_help = "\
EXAMPLES:
  officecli create demo.docx                  Create a blank Word document
  officecli convert old.doc                   Convert legacy .doc to .docx
  officecli convert old.xls -o new.xlsx       Convert with explicit output path
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
        commands::Command::AddPart(cmd) => commands::handle_add_part(cmd, format),
        commands::Command::Remove(cmd) => commands::handle_remove(cmd, format),
        commands::Command::Move(cmd) => commands::handle_move(cmd, format),
        commands::Command::Swap(cmd) => commands::handle_swap(cmd, format),
        commands::Command::Refresh(cmd) => commands::handle_refresh(cmd, format),
        commands::Command::Raw(cmd) => commands::handle_raw(cmd, format),
        commands::Command::RawSet(cmd) => commands::handle_raw_set(cmd, format),
        commands::Command::Validate(cmd) => commands::handle_validate(cmd, format),
        commands::Command::Save(cmd) => commands::handle_save(cmd, format),
        commands::Command::ExtractText(cmd) => commands::handle_extract_text(cmd, format),
        commands::Command::Create(cmd) => commands::handle_create(cmd, format),
        commands::Command::Dump(cmd) => commands::handle_dump(cmd, format),
        commands::Command::Convert(cmd) => commands::handle_convert(cmd, format),
        commands::Command::Batch(cmd) => commands::handle_batch(cmd, format),
        commands::Command::Info(cmd) => commands::handle_info(cmd, format),
        commands::Command::Merge(cmd) => commands::handle_merge(cmd, format),
        commands::Command::Help(cmd) => commands::handle_help(cmd, cli.json),
        commands::Command::Import(cmd) => commands::handle_import(cmd, format),
        commands::Command::Plugins(cmd) => commands::handle_plugins(cmd, format),
        commands::Command::Install(cmd) => commands::handle_install(cmd, format),
        commands::Command::Skills(cmd) => commands::handle_skills(cmd, format),
        commands::Command::Open(cmd) => handle_open(cmd),
        commands::Command::Close(cmd) => handle_close(cmd),
        commands::Command::Watch(cmd) => handle_watch(cmd),
        commands::Command::Unwatch(cmd) => handle_unwatch(cmd),
        commands::Command::Mark(cmd) => commands::handle_mark(cmd, cli.json),
        commands::Command::Unmark(cmd) => commands::handle_unmark(cmd, cli.json),
        commands::Command::Marks(cmd) => commands::handle_marks(cmd, cli.json),
        commands::Command::Goto(cmd) => commands::handle_goto(cmd, cli.json),
        commands::Command::Mcp(_) => handle_mcp(),
        commands::Command::_SocketPath(cmd) => handle_socket_path(cmd),
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
    let port = cmd.port.unwrap_or(crate::watch::DEFAULT_PORT);
    let addr = format!("127.0.0.1:{}", port);
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr
            .parse()
            .map_err(|e| HandlerError::OperationFailed(format!("invalid address: {}", e)))?,
        std::time::Duration::from_secs(3),
    )
    .map_err(|e| {
        HandlerError::OperationFailed(format!(
            "no watch server listening on port {}: {}. Start it with `officecli watch {}`.",
            port, e, cmd.file
        ))
    })?;

    use std::io::{Read, Write};
    let request = "POST /shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream
        .write_all(request.as_bytes())
        .map_err(|e| HandlerError::OperationFailed(format!("write: {}", e)))?;
    let mut buf = [0u8; 256];
    let n = stream
        .read(&mut buf)
        .map_err(|e| HandlerError::OperationFailed(format!("read: {}", e)))?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let status_line = head.lines().next().unwrap_or("(no response)");
    if !status_line.contains("200") && !status_line.contains("204") {
        return Err(HandlerError::OperationFailed(format!(
            "watch server returned: {}",
            status_line
        )));
    }
    Ok(format!("Watch server on port {} shutting down", port))
}

fn handle_mcp() -> Result<String, HandlerError> {
    mcp::run_server()
        .map(|_| "MCP server stopped".to_string())
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))
}

fn handle_socket_path(cmd: commands::SocketPathCommand) -> Result<String, HandlerError> {
    let sock = resident::socket_path_for_file(&cmd.file);
    Ok(sock.to_string_lossy().to_string())
}

/// Open a document handler based on file extension.
pub(crate) fn open_handler(
    file: &str,
    editable: bool,
) -> Result<Box<dyn DocumentHandler>, HandlerError> {
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
        other => {
            // Last-resort: any installed format-handler plugin that owns
            // this extension (e.g. .hwpx). See plugins/plugin-protocol.md §2.3.
            if commands::resolve_format_handler(other).is_some() {
                let proxy = commands::FormatHandlerProxy::open(file)?;
                return Ok(Box::new(proxy));
            }
            Err(HandlerError::OpenError(format!(
                "unsupported format: {}",
                other
            )))
        }
    }
}
