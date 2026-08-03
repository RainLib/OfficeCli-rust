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
            if let Some(xpath) = docx_body_subtree_xpath(logical_path) {
                let handler = crate::open_handler(&cmd.file, false)?;
                let document =
                    handler.raw("word/document.xml", handler_common::RawOptions::default())?;
                let subtree = oxml::xml_util::find_elements_by_xpath(&document, &xpath)
                    .map_err(|error| HandlerError::OperationFailed(error.to_string()))?
                    .into_iter()
                    .next()
                    .ok_or_else(|| HandlerError::PathNotFound(logical_path.to_string()))?;
                let output = serde_json::to_string(&vec![
                    serde_json::json!({"command":"meta","dumpVersion":2}),
                    serde_json::json!({"command":"raw-set","part":"/document","xpath":xpath,"action":"replace","xml":subtree}),
                ]).map_err(HandlerError::JsonError)?;
                if let Some(path) = cmd.out.filter(|path| path != "-") {
                    std::fs::write(&path, format!("{}\n", output))
                        .map_err(HandlerError::IoError)?;
                    return Ok(path);
                }
                return Ok(output);
            }
            let normalized_path = logical_path.to_ascii_lowercase();
            let (part, xpath, replay_part) = match normalized_path.as_str() {
                "/" | "/document" => ("word/document.xml", "/w:document", "/document"),
                "/styles" => ("word/styles.xml", "/w:styles", "/styles"),
                "/settings" => ("word/settings.xml", "/w:settings", "/settings"),
                "/numbering" => ("word/numbering.xml", "/w:numbering", "/numbering"),
                "/comments" => ("/comments", "/w:comments", "/comments"),
                "/theme" => ("/theme", "/a:theme", "/theme"),
                "/fonttable" => ("/fontTable", "/w:fonts", "/fontTable"),
                _ => return Err(HandlerError::UnsupportedMode("replayable DOCX dump supports /, /document, /body, /body/p[N], /body/tbl[N], /theme, /fontTable, /styles, /settings, /numbering, and /comments; use --dom for other subtrees".to_string())),
            };
            let handler = crate::open_handler(&cmd.file, false)?;
            let xml = match handler.raw(part, handler_common::RawOptions::default()) {
                Ok(xml) => Some(xml),
                Err(_) if normalized_path == "/fonttable" => None,
                Err(error) => return Err(error),
            };
            if xml.is_none() {
                let output = serde_json::to_string(&vec![
                    serde_json::json!({"command":"meta","dumpVersion":2}),
                ])
                .map_err(HandlerError::JsonError)?;
                if let Some(path) = cmd.out.filter(|path| path != "-") {
                    std::fs::write(&path, format!("{}\n", output))
                        .map_err(HandlerError::IoError)?;
                    return Ok(path);
                }
                return Ok(output);
            }
            let xml =
                oxml::xml_util::strip_prolog(xml.as_deref().expect("checked above")).to_string();
            let mut items = vec![
                serde_json::json!({"command":"meta","dumpVersion":2}),
                serde_json::json!({"command":"raw-set","part":replay_part,"xpath":xpath,"action":"replace","xml":xml}),
            ];
            if normalized_path == "/fonttable" {
                items.extend(docx_font_table_binary_items(&cmd.file)?);
            }
            let output = serde_json::to_string(&items).map_err(HandlerError::JsonError)?;
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
            for sheet in root.children {
                let xml = handler.raw(&sheet.path, handler_common::RawOptions::default())?;
                items.push(serde_json::json!({"command":"raw-set","part":sheet.path,"xpath":"/worksheet","action":"replace","xml":oxml::xml_util::strip_prolog(&xml)}));
            }
            // Worksheets must be replayed before workbook.xml: a source
            // workbook can reference sheets that do not yet exist in a fresh
            // target, and raw-set creates those semantic sheet parts first.
            let workbook = handler.raw("/workbook", handler_common::RawOptions::default())?;
            items.push(serde_json::json!({"command":"raw-set","part":"/workbook","xpath":"/workbook","action":"replace","xml":oxml::xml_util::strip_prolog(&workbook)}));
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
            for slide in root
                .children
                .into_iter()
                .filter(|node| node.path.starts_with("/slide["))
            {
                let xml = handler.raw(&slide.path, handler_common::RawOptions::default())?;
                items.push(serde_json::json!({"command":"raw-set","part":slide.path,"xpath":"/sld","action":"replace","xml":oxml::xml_util::strip_prolog(&xml)}));
            }
            // Slides must be created/replayed before presentation.xml adopts
            // the source slide relationship list.
            let presentation = handler.raw(
                "ppt/presentation.xml",
                handler_common::RawOptions::default(),
            )?;
            items.push(serde_json::json!({"command":"raw-set","part":"/presentation","xpath":"/presentation","action":"replace","xml":oxml::xml_util::strip_prolog(&presentation)}));
            let output = serde_json::to_string(&items).map_err(HandlerError::JsonError)?;
            if let Some(path) = cmd.out.filter(|path| path != "-") {
                std::fs::write(&path, format!("{}\n", output)).map_err(HandlerError::IoError)?;
                return Ok(path);
            }
            return Ok(output);
        }
        if extension.eq_ignore_ascii_case("pptx") {
            let slide_path = path.as_deref().unwrap_or("/");
            let handler = crate::open_handler(&cmd.file, false)?;
            let xml = handler.raw(slide_path, handler_common::RawOptions::default())?;
            let xpath = match slide_path {
                "/presentation" => "/presentation",
                "/theme" => "/theme",
                path if path.starts_with("/slideMaster[") => "/sldMaster",
                path if path.starts_with("/slideLayout[") => "/sldLayout",
                path if path.starts_with("/noteSlide[") => "/notes",
                _ => "/sld",
            };
            let output = serde_json::to_string(&vec![
                serde_json::json!({"command":"meta","dumpVersion":2}),
                serde_json::json!({"command":"raw-set","part":slide_path,"xpath":xpath,"action":"replace","xml":oxml::xml_util::strip_prolog(&xml)}),
            ]).map_err(HandlerError::JsonError)?;
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

const DOCX_FONT_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/font";
const DOCX_OBFUSCATED_FONT_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.obfuscatedFont";

fn docx_font_table_binary_items(file: &str) -> Result<Vec<serde_json::Value>, HandlerError> {
    let package = oxml::OxmlPackage::open(file, false)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let relationships = package
        .part_rels("word/fontTable.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut fonts = relationships.by_type(DOCX_FONT_REL_TYPE);
    fonts.sort_by(|left, right| left.id.cmp(&right.id));
    let mut items = Vec::new();
    for relationship in fonts {
        if relationship.target_mode.eq_ignore_ascii_case("external") {
            continue;
        }
        let part_path = package.resolve_rel_target("word/fontTable.xml", &relationship.target);
        let bytes = package
            .read_part_bytes(&part_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let content_type = package
            .content_types()
            .content_type_for(&part_path)
            .map(String::as_str)
            .unwrap_or(DOCX_OBFUSCATED_FONT_CONTENT_TYPE);
        items.push(serde_json::json!({
            "command":"raw-set",
            "part":"/fontTable",
            "xpath":relationship.id,
            "action":"embed-binary",
            "xml":format!("data:{};base64,{}", content_type, base64_encode(bytes)),
        }));
    }
    Ok(items)
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn docx_body_subtree_xpath(path: &str) -> Option<String> {
    if path == "/body" {
        return Some("/w:document/w:body".to_string());
    }
    let suffix = path.strip_prefix("/body/")?;
    let mut xpath = "/w:document/w:body".to_string();
    for segment in suffix.split('/') {
        let (name, predicate) = segment.split_once('[').unwrap_or((segment, ""));
        if !matches!(name, "p" | "tbl" | "tr" | "tc" | "r") {
            return None;
        }
        xpath.push_str("/w:");
        xpath.push_str(name);
        if !predicate.is_empty() {
            xpath.push('[');
            xpath.push_str(predicate);
        }
    }
    Some(xpath)
}
