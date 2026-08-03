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

    /// Emit the refreshed text+offset map after the edit (JSON output only).
    /// Use this after range edits to re-address elements whose node structure changed.
    #[arg(long)]
    pub emit_map: bool,

    /// Watch server port used only with the `selected` pseudo-path.
    #[arg(long)]
    pub port: Option<u16>,

    /// Watch document id used only with the `selected` pseudo-path.
    #[arg(long)]
    pub id: Option<String>,
}

pub fn handle_set(cmd: SetCommand, format: OutputFormat) -> Result<String, HandlerError> {
    let handler = crate::open_handler(&cmd.file, true)?;

    let mut properties: HashMap<String, String> = cmd
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
        return Err(HandlerError::InvalidArgument(
            "either element path or --range-paths is required".to_string(),
        ));
    }

    let path_str = if cmd.range_paths.is_some() {
        // When using --range-paths, path is not meaningful
        String::new()
    } else {
        cmd.path.unwrap_or_default()
    };
    let selected_paths = if path_str.eq_ignore_ascii_case("selected") {
        let id = cmd
            .id
            .clone()
            .unwrap_or_else(|| crate::commands::default_id(&cmd.file));
        let response = crate::commands::get_json(
            crate::commands::resolve_port(cmd.port),
            &format!("/{id}/selection"),
        )?;
        response
            .get("paths")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                HandlerError::OperationFailed("invalid watch selection response".to_string())
            })?
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        vec![path_str]
    };
    if selected_paths.is_empty() {
        return Err(HandlerError::OperationFailed(
            "no elements are currently selected".to_string(),
        ));
    }
    let mut unsupported = Vec::new();
    for path in selected_paths {
        unsupported.extend(handler.set(&path, &properties)?);
    }
    handler.save()?;

    let offset_map = if cmd.emit_map {
        super::offset_map_value(handler.as_ref())
    } else {
        None
    };

    match format {
        OutputFormat::Text => {
            // Find/replace-style handlers return synthetic entries like "replaced=N".
            // Surface those as counts rather than as "unsupported" props.
            let (counts, real_unsupported): (Vec<&String>, Vec<&String>) =
                unsupported.iter().partition(|s| s.starts_with("replaced="));
            if counts.is_empty() && real_unsupported.is_empty() {
                Ok("OK".to_string())
            } else if !counts.is_empty() && real_unsupported.is_empty() {
                let joined: Vec<&str> = counts.iter().map(|s| s.as_str()).collect();
                Ok(format!("OK ({})", joined.join(", ")))
            } else {
                // Wrap real unsupported props through style_unsupported_hints so
                // near-misses get a suggestion (e.g. "blod" → "did you mean: bold?").
                let hint = handler_common::format_style_hint(
                    &real_unsupported
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>(),
                );
                if let Some(msg) = hint {
                    Ok(format!("OK ({})", msg))
                } else {
                    let joined: Vec<&str> = real_unsupported.iter().map(|s| s.as_str()).collect();
                    Ok(format!("OK (unsupported: {})", joined.join(", ")))
                }
            }
        }
        OutputFormat::Json => {
            let mut extensions = serde_json::Map::new();
            extensions.insert(
                "result".to_string(),
                serde_json::Value::String("OK".to_string()),
            );
            extensions.insert("unsupported".to_string(), serde_json::json!(unsupported));
            let warnings = unsupported
                .iter()
                .filter(|property| !property.starts_with("replaced="))
                .map(|property| {
                    serde_json::json!({
                        "message": format!("Unsupported property: {}", property),
                        "code": "unsupported_property",
                    })
                })
                .collect::<Vec<_>>();
            if !warnings.is_empty() {
                extensions.insert("warnings".to_string(), serde_json::Value::Array(warnings));
            }
            if let Some(map) = offset_map {
                extensions.insert("offset_map".to_string(), map);
            }
            Ok(crate::commands::json_text_envelope("OK", extensions))
        }
    }
}
