use clap::Args;
use handler_common::HandlerError;

/// Export full document structure and content as JSON
#[derive(Args)]
pub struct DumpCommand {
    pub file: String,
    /// C#-compatible replay format. `batch` is the default.
    #[arg(long, default_value = "batch")]
    pub format: String,
    #[arg(long, short)]
    pub out: Option<String>,
    /// Keep Rust's original DOM JSON export mode.
    #[arg(long)]
    pub dom: bool,
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    /// Legacy Rust spelling for the dump subtree path.
    #[arg(long = "path")]
    pub path_flag: Option<String>,
}

pub fn handle_dump(
    cmd: DumpCommand,
    _format: handler_common::OutputFormat,
) -> Result<String, HandlerError> {
    let path = match (cmd.path, cmd.path_flag) {
        (Some(_), Some(_)) => {
            return Err(HandlerError::InvalidArgument(
                "dump path cannot be supplied both positionally and with --path".to_string(),
            ))
        }
        (Some(path), None) | (None, Some(path)) => Some(path),
        (None, None) => None,
    };
    if !cmd.dom && cmd.format.eq_ignore_ascii_case("batch") {
        let extension = std::path::Path::new(&cmd.file)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if extension.eq_ignore_ascii_case("docx") {
            let logical_path = path.as_deref().unwrap_or("/");
            let (part, xpath) = match logical_path {
                "/" | "/document" => ("word/document.xml", "/w:document"),
                "/styles" => ("word/styles.xml", "/w:styles"),
                "/settings" => ("word/settings.xml", "/w:settings"),
                "/numbering" => ("word/numbering.xml", "/w:numbering"),
                _ => return Err(HandlerError::UnsupportedMode("replayable DOCX dump supports /, /document, /styles, /settings, and /numbering; use --dom for other subtrees".to_string())),
            };
            let replay_part = if logical_path == "/" {
                "/document"
            } else {
                logical_path
            };
            let handler = crate::open_handler(&cmd.file, false)?;
            let xml = handler.raw(part, handler_common::RawOptions::default())?;
            let xml = oxml::xml_util::strip_prolog(&xml).to_string();
            let output = serde_json::to_string(&vec![
                serde_json::json!({"command":"meta","dumpVersion":2}),
                serde_json::json!({"command":"raw-set","part":replay_part,"xpath":xpath,"action":"replace","xml":xml}),
            ]).map_err(HandlerError::JsonError)?;
            if let Some(path) = cmd.out.filter(|path| path != "-") {
                std::fs::write(&path, format!("{}\n", output)).map_err(HandlerError::IoError)?;
                return Ok(path);
            }
            return Ok(output);
        }
        if extension.eq_ignore_ascii_case("xlsx") && path.as_deref().unwrap_or("/") == "/" {
            let handler = crate::open_handler(&cmd.file, false)?;
            let root = handler.get("/", 0)?;
            let mut items = vec![serde_json::json!({"command":"meta","dumpVersion":2})];
            let workbook = handler.raw("/workbook", handler_common::RawOptions::default())?;
            items.push(serde_json::json!({"command":"raw-set","part":"/workbook","xpath":"/workbook","action":"replace","xml":oxml::xml_util::strip_prolog(&workbook)}));
            for sheet in root.children {
                let xml = handler.raw(&sheet.path, handler_common::RawOptions::default())?;
                items.push(serde_json::json!({"command":"raw-set","part":sheet.path,"xpath":"/worksheet","action":"replace","xml":oxml::xml_util::strip_prolog(&xml)}));
            }
            let output = serde_json::to_string(&items).map_err(HandlerError::JsonError)?;
            if let Some(path) = cmd.out.filter(|path| path != "-") {
                std::fs::write(&path, format!("{}\n", output)).map_err(HandlerError::IoError)?;
                return Ok(path);
            }
            return Ok(output);
        }
        if extension.eq_ignore_ascii_case("xlsx") {
            let sheet_path = path.as_deref().unwrap_or("/");
            let handler = crate::open_handler(&cmd.file, false)?;
            let xml = handler.raw(sheet_path, handler_common::RawOptions::default())?;
            let output = serde_json::to_string(&vec![
                serde_json::json!({"command":"meta","dumpVersion":2}),
                serde_json::json!({"command":"raw-set","part":sheet_path,"xpath":"/worksheet","action":"replace","xml":oxml::xml_util::strip_prolog(&xml)}),
            ]).map_err(HandlerError::JsonError)?;
            if let Some(path) = cmd.out.filter(|path| path != "-") {
                std::fs::write(&path, format!("{}\n", output)).map_err(HandlerError::IoError)?;
                return Ok(path);
            }
            return Ok(output);
        }
        if extension.eq_ignore_ascii_case("pptx") && path.as_deref().unwrap_or("/") == "/" {
            let handler = crate::open_handler(&cmd.file, false)?;
            let root = handler.get("/", 1)?;
            let mut items = vec![serde_json::json!({"command":"meta","dumpVersion":2})];
            let presentation = handler.raw(
                "ppt/presentation.xml",
                handler_common::RawOptions::default(),
            )?;
            items.push(serde_json::json!({"command":"raw-set","part":"/presentation","xpath":"/presentation","action":"replace","xml":oxml::xml_util::strip_prolog(&presentation)}));
            for slide in root
                .children
                .into_iter()
                .filter(|node| node.path.starts_with("/slide["))
            {
                let xml = handler.raw(&slide.path, handler_common::RawOptions::default())?;
                items.push(serde_json::json!({"command":"raw-set","part":slide.path,"xpath":"/sld","action":"replace","xml":oxml::xml_util::strip_prolog(&xml)}));
            }
            let output = serde_json::to_string(&items).map_err(HandlerError::JsonError)?;
            if let Some(path) = cmd.out.filter(|path| path != "-") {
                std::fs::write(&path, format!("{}\n", output)).map_err(HandlerError::IoError)?;
                return Ok(path);
            }
            return Ok(output);
        }
        return Err(HandlerError::UnsupportedMode("replayable dump currently supports full .docx documents; use --dom for the Rust DOM export".to_string()));
    }
    let handler = crate::open_handler(&cmd.file, false)?;

    if let Some(path) = path {
        // Dump a specific node as JSON
        let node = handler.get(&path, 10)?;
        let json = serde_json::to_string_pretty(&node).map_err(|e| HandlerError::JsonError(e))?;
        Ok(json)
    } else {
        // Dump the entire document structure
        let root = handler.get("/", 3)?;
        let json = serde_json::to_string_pretty(&root).map_err(|e| HandlerError::JsonError(e))?;
        Ok(json)
    }
}
