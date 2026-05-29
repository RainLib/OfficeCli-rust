use clap::Args;
use handler_common::HandlerError;

/// Display tool information and help topics (formats, commands, paths)
#[derive(Args)]
pub struct InfoCommand {
    /// Topic to get info on
    pub topic: Option<String>,
}

pub fn handle_info(
    cmd: InfoCommand,
    _format: handler_common::OutputFormat,
) -> Result<String, HandlerError> {
    match cmd.topic.as_deref() {
        Some("docx") => Ok("Word document (.docx):\n  Elements: p (paragraph), r (run), tbl (table), tr (row), tc (cell)\n  Paths: /body/p[N], /body/tbl[N]/tr[N]/tc[N]".to_string()),
        Some("xlsx") => Ok("Excel spreadsheet (.xlsx):\n  Elements: sheet, cell, chart, table, pivot\n  Paths: /SheetName/A1, /Sheet1/B5".to_string()),
        Some("pptx") => Ok("PowerPoint (.pptx):\n  Elements: slide, shape, picture, textbox, table\n  Paths: /slide[N]/shape[N]".to_string()),
        Some("pdf") => Ok("PDF:\n  Elements: page, text, image, annotation, link\n  Paths: /page[N], /page[N]/text[N]\n  Properties for 'set':\n    text=VALUE         Set text content\n    font=FONT_NAME     Set font name (e.g. HeitiSC, Helvetica)\n    fontFile=PATH.ttf  Subset and embed a custom TrueType font file\n    size=NUMBER        Set font size in pt\n    color=COLOR_STR    Set text fill color (hex '#FF0000' or 'rgb(255,0,0)')\n    bgColor=COLOR_STR  Set block background color (hex '#FFFF00')\n    charSpacing=NUM    Set character spacing (f32)\n    wordSpacing=NUM    Set word spacing (f32)\n\n  extract-text --with-offsets: get text+offset->path mapping".to_string()),
        Some("offset") => Ok("Text Offset Mapping:\n  officecli extract-text <file> --with-offsets --json\n  Returns: { full_text, spans: [{start, end, path, text, element_type}], meta }\n  Each character offset maps to a document path ID".to_string()),
        None => Ok("OfficeCLI (Rust) Commands:\n  view, get, query, set, add, remove, move, raw, raw-set\n  validate, save, extract-text, create, dump, batch, info\n\n  Supported formats: docx, xlsx, pptx, pdf\n  Use 'info <topic>' for details".to_string()),
        Some(other) => Err(HandlerError::InvalidArgument(format!("unknown info topic: {}", other))),
    }
}
