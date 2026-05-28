use clap::Args;
use handler_common::{HandlerError, OutputFormat};
use std::collections::HashMap;

/// Modify properties of an element at a path (text, style, content)
#[derive(Args)]
#[command(after_help = "\
SUPPORTED PROPERTIES BY FORMAT:

PDF:
  text=VALUE         Set text content
  font=FONT_NAME     Set font name (e.g. HeitiSC, Helvetica)
  fontFile=PATH.ttf  Subset and embed a custom TrueType font file
  size=NUMBER        Set font size in pt
  color=COLOR        Set text color (hex '#FF0000', 'FF0000', or 'rgb(255,0,0)')
  bgColor=COLOR      Set block background color (hex '#FFFF00', 'FFFF00')
  charSpacing=NUM    Set character spacing (f32)
  wordSpacing=NUM    Set word spacing (f32)

Word (.docx):
  text=VALUE         Set text content of a paragraph or run
  style=STYLE_NAME   Set style name (e.g. Heading1, Normal)

Excel (.xlsx):
  text=VALUE         Set cell text content

PowerPoint (.pptx):
  text=VALUE         Set textbox text content

EXAMPLES:
  officecli set demo.pdf '/page[1]/text[5]' text='New Title' color='#FF0000' bgColor='#FFFF00'
  officecli set demo.pdf '/page[1]/text[5]' fontFile='assets/MyFont.ttf' size=14.5
  officecli set demo.docx '/body/p[1]' text='Hello World' style='Heading1'
")]
pub struct SetCommand {
    /// Document file path
    pub file: String,

    /// Path to the element (optional if using --range-paths)
    pub path: Option<String>,

    /// Path range list with optional partial offsets (e.g. "/page[1]/text[2][2..],/page[1]/text[3]")
    #[arg(long)]
    pub range_paths: Option<String>,

    /// Properties to set (key=value pairs, e.g. "text=hello" "style=Heading1")
    #[arg(num_args = 0..)]
    pub properties: Vec<String>,
}

pub fn handle_set(cmd: SetCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;

    let mut properties: HashMap<String, String> = cmd.properties
        .iter()
        .filter_map(|p| {
            let parts: Vec<&str> = p.splitn(2, '=').collect();
            if parts.len() == 2 {
                Some((parts[0].to_string(), parts[1].to_string()))
            } else {
                None
            }
        })
        .collect();

    if let Some(ref rp) = cmd.range_paths {
        // When --range-paths is used, clap may misparse the first key=value property
        // as the positional `path` argument. Detect and fix this.
        if let Some(ref path_val) = cmd.path {
            if path_val.contains('=') {
                let parts: Vec<&str> = path_val.splitn(2, '=').collect();
                if parts.len() == 2 {
                    properties.insert(parts[0].to_string(), parts[1].to_string());
                }
            }
        }

        // Validate DSL syntax
        handler_common::parse_range_paths(rp)
            .map_err(|e| HandlerError::InvalidArgument(format!("invalid --range-paths: {}", e)))?;
        properties.insert("range_paths".to_string(), rp.clone());
    } else if cmd.path.is_none() {
        return Err(HandlerError::InvalidArgument("either element path or --range-paths is required".to_string()));
    }

    let path_str = if cmd.range_paths.is_some() {
        // When using --range-paths, path is not meaningful
        String::new()
    } else {
        cmd.path.unwrap_or_default()
    };
    let unsupported = handler.set(&path_str, &properties)?;
    handler.save()?;

    match format {
        OutputFormat::Text => {
            if unsupported.is_empty() {
                Ok("OK".to_string())
            } else {
                Ok(format!("OK (unsupported: {})", unsupported.join(", ")))
            }
        }
        OutputFormat::Json => Ok(serde_json::json!({
            "result": "OK",
            "unsupported": unsupported
        })
        .to_string()),
    }
}
