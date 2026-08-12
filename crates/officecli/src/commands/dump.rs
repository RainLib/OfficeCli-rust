use clap::Args;
use handler_common::HandlerError;
use std::collections::HashSet;

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
    // A few rich dump emitters inspect package parts directly. Flush the
    // resident snapshot first so these reads cannot observe an older disk
    // version than the raw handler calls in the same dump.
    if crate::resident_available(&cmd.file) {
        crate::open_handler(&cmd.file, true)?.save()?;
    }
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
            if logical_path == "/" {
                let handler = crate::open_handler(&cmd.file, false)?;
                let mut items = vec![serde_json::json!({"command":"meta","dumpVersion":2})];
                // Header/footer XML is linked by r:id from document.xml.  Replay
                // the document first, then create each referenced part while
                // retaining that source relationship id.
                let document =
                    handler.raw("word/document.xml", handler_common::RawOptions::default())?;
                items.push(serde_json::json!({"command":"raw-set","part":"/document","xpath":"/w:document","action":"replace","xml":oxml::xml_util::strip_prolog(&document)}));
                items.extend(docx_recursive_relationship_items(&cmd.file)?);
                items.extend(docx_document_image_items(&cmd.file)?);
                items.extend(docx_custom_xml_items(&cmd.file)?);
                items.extend(docx_header_footer_items(&cmd.file, "header")?);
                items.extend(docx_header_footer_items(&cmd.file, "footer")?);
                for (part, xpath) in [
                    ("/numbering", "/w:numbering"),
                    ("/styles", "/w:styles"),
                    ("/theme", "/a:theme"),
                    ("/settings", "/w:settings"),
                    ("/footnotes", "/w:footnotes"),
                    ("/endnotes", "/w:endnotes"),
                    ("/webSettings", "/w:webSettings"),
                    ("/docProps/core.xml", "/cp:coreProperties"),
                    ("/docProps/app.xml", "/Properties"),
                    ("/docProps/custom.xml", "/Properties"),
                    ("/fontTable", "/w:fonts"),
                    ("/comments", "/w:comments"),
                    ("/commentsExtended", "/w15:commentsEx"),
                ] {
                    if let Ok(xml) = handler.raw(part, handler_common::RawOptions::default()) {
                        items.push(serde_json::json!({"command":"raw-set","part":part,"xpath":xpath,"action":"replace","xml":oxml::xml_util::strip_prolog(&xml)}));
                        if part == "/fontTable" {
                            items.extend(docx_font_table_binary_items(&cmd.file)?);
                        }
                        if part == "/numbering" {
                            items.extend(docx_numbering_image_items(&cmd.file)?);
                        }
                        if let Some(source_part) = match part {
                            "/comments" => Some("word/comments.xml"),
                            "/footnotes" => Some("word/footnotes.xml"),
                            "/endnotes" => Some("word/endnotes.xml"),
                            _ => None,
                        } {
                            let package =
                                oxml::OxmlPackage::open(&cmd.file, false).map_err(|error| {
                                    HandlerError::OperationFailed(error.to_string())
                                })?;
                            items.extend(docx_part_image_items(&package, source_part, part)?);
                        }
                    }
                }
                let output = serde_json::to_string(&items).map_err(HandlerError::JsonError)?;
                if let Some(path) = cmd.out.filter(|path| path != "-") {
                    std::fs::write(&path, format!("{}\n", output))
                        .map_err(HandlerError::IoError)?;
                    return Ok(path);
                }
                return Ok(output);
            }
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
                "/footnotes" => ("/footnotes", "/w:footnotes", "/footnotes"),
                "/endnotes" => ("/endnotes", "/w:endnotes", "/endnotes"),
                "/websettings" => ("/webSettings", "/w:webSettings", "/webSettings"),
                "/comments" => ("/comments", "/w:comments", "/comments"),
                "/commentsextended" => (
                    "/commentsExtended",
                    "/w15:commentsEx",
                    "/commentsExtended",
                ),
                "/theme" => ("/theme", "/a:theme", "/theme"),
                "/fonttable" => ("/fontTable", "/w:fonts", "/fontTable"),
                path if semantic_header_footer_path(path).is_some() => (logical_path, "/", logical_path),
                _ => return Err(HandlerError::UnsupportedMode("replayable DOCX dump supports /, /document, /body, /body/p[N], /body/tbl[N], /header[N], /footer[N], /theme, /fontTable, /styles, /settings, /numbering, /footnotes, /endnotes, /webSettings, /comments, and /commentsExtended; use --dom for other subtrees".to_string())),
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
            // Preserve drawings, chart sidecars, richData/metadata, embedded
            // workbooks and external links instead of limiting replay to sheet
            // XML. Parent edges are emitted before their descendants.
            items.extend(recursive_relationship_items(&cmd.file, "xl/workbook.xml")?);
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
            // A presentation's resource graph includes slide/master/layout
            // edges plus opaque chart, media, OLE, SmartArt, model3d and p15
            // extension sidecars. Emit it verbatim for lossless replay.
            items.extend(recursive_relationship_items(
                &cmd.file,
                "ppt/presentation.xml",
            )?);
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

const DOCX_HEADER_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const DOCX_FOOTER_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
const DOCX_IMAGE_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
const DOCX_CUSTOM_XML_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
const DOCX_CUSTOM_XML_PROPS_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";

fn docx_custom_xml_items(file: &str) -> Result<Vec<serde_json::Value>, HandlerError> {
    let package = oxml::OxmlPackage::open(file, false)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let relationships = package
        .part_rels("word/document.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut custom_items = relationships.by_type(DOCX_CUSTOM_XML_REL_TYPE);
    custom_items.sort_by(|left, right| left.id.cmp(&right.id));
    let mut items = Vec::new();
    for relationship in custom_items {
        if relationship.target_mode.eq_ignore_ascii_case("external") {
            continue;
        }
        let item_part = package.resolve_rel_target("word/document.xml", &relationship.target);
        let bytes = package
            .read_part_bytes(&item_part)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let content_type = package
            .content_types()
            .content_type_for(&item_part)
            .map(String::as_str)
            .unwrap_or("application/xml");
        items.push(serde_json::json!({
            "command":"raw-set",
            "part":"/customXml",
            "xpath":relationship.id,
            "action":"embed-binary",
            "xml":format!("data:{};base64,{}", content_type, base64_encode(bytes)),
        }));
        let item_relationships = package
            .part_rels(&item_part)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let mut properties = item_relationships.by_type(DOCX_CUSTOM_XML_PROPS_REL_TYPE);
        properties.sort_by(|left, right| left.id.cmp(&right.id));
        for properties_relationship in properties {
            if properties_relationship
                .target_mode
                .eq_ignore_ascii_case("external")
            {
                continue;
            }
            let properties_part =
                package.resolve_rel_target(&item_part, &properties_relationship.target);
            let bytes = package
                .read_part_bytes(&properties_part)
                .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
            let content_type = package
                .content_types()
                .content_type_for(&properties_part)
                .map(String::as_str)
                .unwrap_or("application/vnd.openxmlformats-officedocument.customXmlProperties+xml");
            items.push(serde_json::json!({
                "command":"raw-set",
                "part":format!("/customXml/{}", relationship.id),
                "xpath":properties_relationship.id,
                "action":"embed-binary",
                "xml":format!("data:{};base64,{}", content_type, base64_encode(bytes)),
            }));
        }
    }
    Ok(items)
}

fn docx_document_image_items(file: &str) -> Result<Vec<serde_json::Value>, HandlerError> {
    let package = oxml::OxmlPackage::open(file, false)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    docx_part_image_items(&package, "word/document.xml", "/document")
}

/// Emit every relationship reachable from the main document as a physical-part
/// replay edge.  This complements the C# structured emitter for opaque OOXML
/// resources (Chart sidecars, SmartArt, ActiveX, OLE and producer extensions)
/// whose XML is intentionally preserved verbatim.  Parent edges precede child
/// edges so replay creates the owning part before its `.rels` is attached.
fn docx_recursive_relationship_items(file: &str) -> Result<Vec<serde_json::Value>, HandlerError> {
    let package = oxml::OxmlPackage::open(file, false)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    emit_docx_relationship_edges(&package, "word/document.xml", &mut seen, &mut items)?;
    Ok(items)
}

fn emit_docx_relationship_edges(
    package: &oxml::OxmlPackage,
    source_part: &str,
    seen: &mut HashSet<(String, String)>,
    items: &mut Vec<serde_json::Value>,
) -> Result<(), HandlerError> {
    let relationships = package
        .part_rels(source_part)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut edges: Vec<_> = relationships.all().values().collect();
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    for edge in edges {
        if !seen.insert((source_part.to_string(), edge.id.clone())) {
            continue;
        }
        let mut payload = serde_json::json!({
            "type": edge.type_uri,
            "target": edge.target,
            "targetMode": edge.target_mode,
        });
        if !edge.target_mode.eq_ignore_ascii_case("external") {
            let part_path = package.resolve_rel_target(source_part, &edge.target);
            let bytes = package
                .read_part_bytes(&part_path)
                .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
            let content_type = package
                .content_types()
                .content_type_for(&part_path)
                .map(String::as_str)
                .unwrap_or("application/octet-stream");
            payload["partPath"] = serde_json::Value::String(part_path.clone());
            payload["contentType"] = serde_json::Value::String(content_type.to_string());
            payload["data"] = serde_json::Value::String(format!(
                "data:{};base64,{}",
                content_type,
                base64_encode(bytes)
            ));
            items.push(serde_json::json!({"command":"raw-set","part":source_part,"xpath":edge.id,"action":"embed-part","xml":payload.to_string()}));
            emit_docx_relationship_edges(package, &part_path, seen, items)?;
        } else {
            items.push(serde_json::json!({"command":"raw-set","part":source_part,"xpath":edge.id,"action":"embed-part","xml":payload.to_string()}));
        }
    }
    Ok(())
}

/// Format-neutral version of the opaque OOXML relationship emitter. XLSX and
/// PPTX use the same package graph semantics as DOCX; their handlers consume
/// the physical source part names with the `embed-part` raw action.
fn recursive_relationship_items(
    file: &str,
    root_part: &str,
) -> Result<Vec<serde_json::Value>, HandlerError> {
    let package = oxml::OxmlPackage::open(file, false)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    emit_relationship_edges(&package, root_part, &mut seen, &mut items)?;
    Ok(items)
}

fn emit_relationship_edges(
    package: &oxml::OxmlPackage,
    source_part: &str,
    seen: &mut HashSet<(String, String)>,
    items: &mut Vec<serde_json::Value>,
) -> Result<(), HandlerError> {
    let relationships = package
        .part_rels(source_part)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut edges: Vec<_> = relationships.all().values().collect();
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    for edge in edges {
        if !seen.insert((source_part.to_string(), edge.id.clone())) {
            continue;
        }
        let mut payload = serde_json::json!({
            "type": edge.type_uri,
            "target": edge.target,
            "targetMode": edge.target_mode,
        });
        if !edge.target_mode.eq_ignore_ascii_case("external") {
            let part_path = package.resolve_rel_target(source_part, &edge.target);
            let bytes = package
                .read_part_bytes(&part_path)
                .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
            let content_type = package
                .content_types()
                .content_type_for(&part_path)
                .map(String::as_str)
                .unwrap_or("application/octet-stream");
            payload["partPath"] = serde_json::Value::String(part_path.clone());
            payload["contentType"] = serde_json::Value::String(content_type.to_string());
            payload["data"] = serde_json::Value::String(format!(
                "data:{};base64,{}",
                content_type,
                base64_encode(bytes)
            ));
            items.push(serde_json::json!({"command":"raw-set","part":source_part,"xpath":edge.id,"action":"embed-part","xml":payload.to_string()}));
            emit_relationship_edges(package, &part_path, seen, items)?;
        } else {
            items.push(serde_json::json!({"command":"raw-set","part":source_part,"xpath":edge.id,"action":"embed-part","xml":payload.to_string()}));
        }
    }
    Ok(())
}

fn docx_part_image_items(
    package: &oxml::OxmlPackage,
    source_part: &str,
    replay_part: &str,
) -> Result<Vec<serde_json::Value>, HandlerError> {
    let relationships = package
        .part_rels(source_part)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut images = relationships.by_type(DOCX_IMAGE_REL_TYPE);
    images.sort_by(|left, right| left.id.cmp(&right.id));
    let mut items = Vec::new();
    for relationship in images {
        if relationship.target_mode.eq_ignore_ascii_case("external") {
            continue;
        }
        let part_path = package.resolve_rel_target(source_part, &relationship.target);
        let bytes = package
            .read_part_bytes(&part_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let content_type = package
            .content_types()
            .content_type_for(&part_path)
            .map(String::as_str)
            .or_else(|| image_content_type_from_path(&part_path))
            .ok_or_else(|| {
                HandlerError::OperationFailed(format!(
                    "cannot determine content type for document image '{}'",
                    part_path
                ))
            })?;
        items.push(serde_json::json!({
            "command":"raw-set",
            "part":replay_part,
            "xpath":relationship.id,
            "action":"embed-binary",
            "xml":format!("data:{};base64,{}", content_type, base64_encode(bytes)),
        }));
    }
    Ok(items)
}

fn docx_numbering_image_items(file: &str) -> Result<Vec<serde_json::Value>, HandlerError> {
    let package = oxml::OxmlPackage::open(file, false)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    docx_part_image_items(&package, "word/numbering.xml", "/numbering")
}

fn image_content_type_from_path(path: &str) -> Option<&'static str> {
    match path.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "ico" => Some("image/x-icon"),
        "emf" => Some("image/x-emf"),
        "wmf" => Some("image/x-wmf"),
        _ => None,
    }
}

fn docx_header_footer_items(
    file: &str,
    kind: &str,
) -> Result<Vec<serde_json::Value>, HandlerError> {
    let relationship_type = match kind {
        "header" => DOCX_HEADER_REL_TYPE,
        "footer" => DOCX_FOOTER_REL_TYPE,
        _ => {
            return Err(HandlerError::InvalidArgument(format!(
                "invalid header/footer kind: {kind}"
            )))
        }
    };
    let package = oxml::OxmlPackage::open(file, false)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let relationships = package
        .part_rels("word/document.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut parts = relationships.by_type(relationship_type);
    parts.sort_by(|left, right| left.id.cmp(&right.id));
    let mut items = Vec::new();
    for (index, relationship) in parts.into_iter().enumerate() {
        if relationship.target_mode.eq_ignore_ascii_case("external") {
            continue;
        }
        let part_path = package.resolve_rel_target("word/document.xml", &relationship.target);
        let xml = package
            .read_part_xml(&part_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        items.push(serde_json::json!({
            "command":"raw-set",
            "part":format!("/{kind}[{}]", index + 1),
            "xpath":relationship.id,
            "action":"replace",
            "xml":oxml::xml_util::strip_prolog(&xml),
        }));
        items.extend(docx_part_image_items(
            &package,
            &part_path,
            &format!("/{kind}[{}]", index + 1),
        )?);
    }
    Ok(items)
}

fn semantic_header_footer_path(path: &str) -> Option<()> {
    let lower = path.trim_matches('/').to_ascii_lowercase();
    for kind in ["header", "footer"] {
        if let Some(index) = lower
            .strip_prefix(kind)
            .and_then(|suffix| suffix.strip_prefix('['))
            .and_then(|suffix| suffix.strip_suffix(']'))
            .and_then(|suffix| suffix.parse::<usize>().ok())
        {
            if index > 0 {
                return Some(());
            }
        }
    }
    None
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
