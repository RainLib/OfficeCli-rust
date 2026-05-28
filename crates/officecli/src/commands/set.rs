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

    /// Path to the element
    pub path: String,

    /// Properties to set (key=value pairs, e.g. "text=hello" "style=Heading1")
    #[arg(num_args = 1..)]
    pub properties: Vec<String>,
}

pub fn handle_set(cmd: SetCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;

    let properties: HashMap<String, String> = cmd
        .properties
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

    let unsupported = handler.set(&cmd.path, &properties)?;
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
