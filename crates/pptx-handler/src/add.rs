use handler_common::{HandlerError, InsertPosition};
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Add an element to the PPTX presentation.
/// Expanded vocabulary matching C# PowerPointHandler.Add:
/// slide, shape, textbox, text, rectangle/rect, ellipse/oval, connector,
/// line, group, picture/image, video, audio, table, chart, hyperlink,
/// media, model3d, comment, note
pub fn add_element(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    match element_type {
        "slide" => add_slide(package, parent),
        "shape" | "textbox" | "sp" => add_shape(package, parent, element_type, properties),
        "text" => add_text_to_shape(package, parent, properties),
        "rectangle" | "rect" => add_rectangle(package, parent, properties),
        "ellipse" | "oval" | "circle" => add_ellipse(package, parent, properties),
        "line" | "lineShape" => add_line_shape(package, parent, properties),
        "connector" => add_connector(package, parent, properties),
        "group" | "grpSp" => add_group(package, parent, properties),
        "picture" | "image" | "img" => add_picture(package, parent, properties),
        "video" | "media" => add_video(package, parent, properties),
        "audio" => add_audio(package, parent, properties),
        "table" | "graphicFrame" => add_table(package, parent, properties),
        "chart" => add_chart_real(package, parent, properties),
        "model3d" | "3dmodel" => add_model3d_real(package, parent, properties),
        "comment" => add_comment(package, parent, properties),
        "note" | "notes" => add_note(package, parent, properties),
        "hyperlink" => add_hyperlink(package, parent, properties),
        "transition" => add_transition(package, parent, properties),
        "animation" | "anim" => add_animation(package, parent, properties),
        other => Err(HandlerError::UnsupportedType(format!(
            "PPTX add '{}' not supported. Supported types: slide, shape, textbox, text, \
             rectangle, ellipse, line, connector, group, picture/image, video, audio, \
             table, chart, model3d, comment, note, hyperlink, transition, animation",
            other
        ))),
    }
}

fn add_slide(package: &mut OxmlPackage, _parent: &str) -> Result<String, HandlerError> {
    // Logical slide index and physical part number are separate: part names can
    // have gaps after a deletion.
    let pres = crate::navigation::build_presentation(package)?;
    let slide_index = pres.slides.len() + 1;
    let slide_part_number = next_slide_part_number(package);
    let slide_path = format!("ppt/slides/slide{}.xml", slide_part_number);

    let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr/>
    </p:spTree>
  </p:cSld>
</p:sld>"#
        .to_string();

    package
        .write_part_xml(&slide_path, &slide_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Update presentation.xml to add the new slide reference
    update_presentation_slides(package, slide_part_number)?;
    register_slide_content_type(package, &slide_path)?;

    Ok(format!("/slide[{}]", slide_index))
}

fn add_shape(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    // Parse parent path to find slide
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;

    let text = properties.get("text").cloned().unwrap_or_default();
    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| element_type.to_string());

    // Get the existing slide XML
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Find next shape ID
    let next_id = find_max_id(&slide_xml) + 1;

    // Create new shape XML
    let shape_xml = create_text_shape_xml(next_id, &name, &text);

    // Insert the shape into the spTree in the slide XML
    let modified = insert_shape_in_sp_tree(&slide_xml, &shape_xml);

    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Determine shape index (count existing shapes + 1)
    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    let shape_idx = slide.shapes.len() + 1;

    Ok(format!("/slide[{}]/shape[{}]", slide_num, shape_idx))
}

fn add_text_to_shape(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    // This is essentially set_text on the shape
    crate::view::set_shape_text(package, parent, properties)?;
    Ok(parent.to_string())
}

pub fn update_presentation_slides(
    package: &mut OxmlPackage,
    slide_part_number: usize,
) -> Result<(), HandlerError> {
    let pres_xml = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let sld_id = next_slide_id(&pres_xml)?;

    // Relationship IDs share a namespace with masters, themes and other
    // presentation parts, so derive the next free ID from the rels part.
    let rels_path = "ppt/_rels/presentation.xml.rels";
    let rels_xml = package
        .read_part_xml(rels_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let r_id = format!("rId{}", find_max_rel_id(&rels_xml) + 1);
    let new_entry = format!("<p:sldId id=\"{}\" r:id=\"{}\"/>", sld_id, r_id);

    // Insert into <p:sldIdLst>
    let modified = if let Some(pos) = pres_xml.find("</p:sldIdLst>") {
        let mut result = pres_xml.clone();
        result.insert_str(pos, &new_entry);
        result
    } else {
        pres_xml
    };

    package
        .write_part_xml("ppt/presentation.xml", &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
        r_id, slide_part_number
    );

    let modified_rels = if let Some(pos) = rels_xml.find("</Relationships>") {
        let mut result = rels_xml.clone();
        result.insert_str(pos, &new_rel);
        result
    } else {
        rels_xml
    };

    package
        .write_part_xml(rels_path, &modified_rels)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(())
}

pub(crate) fn next_slide_part_number(package: &OxmlPackage) -> usize {
    package
        .list_parts()
        .into_iter()
        .filter_map(|path| {
            path.strip_prefix("ppt/slides/slide")
                .and_then(|value| value.strip_suffix(".xml"))
                .and_then(|value| value.parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0)
        + 1
}

fn next_slide_id(presentation_xml: &str) -> Result<u64, HandlerError> {
    let doc = roxmltree::Document::parse(presentation_xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid presentation.xml: {}", e)))?;
    Ok(doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "sldId")
        .filter_map(|node| node.attribute("id"))
        .filter_map(|value| value.parse::<u64>().ok())
        .max()
        .unwrap_or(255)
        + 1)
}

pub(crate) fn register_slide_content_type(
    package: &mut OxmlPackage,
    slide_path: &str,
) -> Result<(), HandlerError> {
    let content_types_path = "[Content_Types].xml";
    let content_types_xml = package
        .read_part_xml(content_types_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let part_name = format!("/{}", slide_path.trim_start_matches('/'));
    if content_types_xml.contains(&format!("PartName=\"{}\"", part_name)) {
        return Ok(());
    }
    let override_xml = format!(
        "<Override PartName=\"{}\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>",
        part_name
    );
    let close = content_types_xml.find("</Types>").ok_or_else(|| {
        HandlerError::OperationFailed("invalid [Content_Types].xml: missing </Types>".to_string())
    })?;
    let mut updated = content_types_xml;
    updated.insert_str(close, &override_xml);
    package
        .write_part_xml(content_types_path, &updated)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))
}

fn create_text_shape_xml(id: usize, name: &str, text: &str) -> String {
    create_shape_xml_with_geometry(
        id, name, text, "rect", "457200", "274638", "8382000", "304800", true, None,
    )
}

/// Parameters for building a <p:sp> shape XML.
#[allow(clippy::too_many_arguments)]
#[derive(Debug, Clone)]
struct ShapeParams<'a> {
    id: usize,
    name: &'a str,
    text: &'a str,
    prst: &'a str,
    x: &'a str,
    y: &'a str,
    cx: &'a str,
    cy: &'a str,
    is_textbox: bool,
    fill_color: Option<&'a str>,
}

/// Build a <p:sp> shape XML with arbitrary geometry, preset geometry, and optional fill.
#[allow(clippy::too_many_arguments)]
fn create_shape_xml_with_geometry(
    id: usize,
    name: &str,
    text: &str,
    prst: &str,
    x: &str,
    y: &str,
    cx: &str,
    cy: &str,
    is_textbox: bool,
    fill_color: Option<&str>,
) -> String {
    build_shape_xml(ShapeParams {
        id,
        name,
        text,
        prst,
        x,
        y,
        cx,
        cy,
        is_textbox,
        fill_color,
    })
}

fn build_shape_xml(p: ShapeParams) -> String {
    let ShapeParams {
        id,
        name,
        text,
        prst,
        x,
        y,
        cx,
        cy,
        is_textbox,
        fill_color,
    } = p;
    let escaped_text = xml_escape_text(text);
    let cnvpr_sp_pr = if is_textbox {
        "<p:cNvSpPr txBox=\"1\"/>"
    } else {
        "<p:cNvSpPr/>"
    };
    let fill_xml = if let Some(color) = fill_color {
        let hex = color.strip_prefix('#').unwrap_or(color);
        format!("<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>", hex)
    } else {
        String::new()
    };

    let body_xml = if is_textbox || !text.is_empty() {
        format!(
            r#"<p:txBody>
    <a:bodyPr/>
    <a:lstStyle/>
    <a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{escaped_text}</a:t></a:r></a:p>
  </p:txBody>"#
        )
    } else {
        String::new()
    };

    format!(
        r#"<p:sp>
  <p:nvSpPr>
    <p:cNvPr id="{id}" name="{name}"/>
    {cnvpr_sp_pr}
    <p:nvPr/>
  </p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>
    <a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom>
    {fill_xml}
  </p:spPr>
  {body_xml}
</p:sp>"#
    )
}

fn insert_shape_in_sp_tree(slide_xml: &str, shape_xml: &str) -> String {
    // Find the end of the spTree's last child before </p:spTree>
    if let Some(pos) = slide_xml.find("</p:spTree>") {
        let mut result = slide_xml.to_string();
        result.insert_str(pos, shape_xml);
        result
    } else {
        slide_xml.to_string()
    }
}

fn find_max_id(xml: &str) -> usize {
    let mut max_id = 1;
    // Find all id="N" patterns
    for part in xml.split("id=\"") {
        if let Some(end) = part.find('"') {
            if let Ok(id) = part[..end].parse::<usize>() {
                if id > max_id {
                    max_id = id;
                }
            }
        }
    }
    max_id
}

fn parse_slide_num(path: &str) -> Result<usize, HandlerError> {
    path.strip_prefix("/slide[")
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|s| s.split('/').next())
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| HandlerError::InvalidPath(format!("expected /slide[N], got: {}", path)))
}

fn xml_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ─── New Element Types ─────────────────────────────────────────────────

/// Extract position/size properties from a props map with sensible defaults.
fn extract_geometry(props: &HashMap<String, String>) -> (String, String, String, String) {
    let x = props
        .get("x")
        .or_else(|| props.get("left"))
        .map(|v| unit_to_emu(v))
        .unwrap_or_else(|| "457200".to_string()); // 0.5 inch
    let y = props
        .get("y")
        .or_else(|| props.get("top"))
        .map(|v| unit_to_emu(v))
        .unwrap_or_else(|| "274638".to_string());
    let cx = props
        .get("width")
        .or_else(|| props.get("w"))
        .or_else(|| props.get("cx"))
        .map(|v| unit_to_emu(v))
        .unwrap_or_else(|| "8382000".to_string()); // ~9 inches
    let cy = props
        .get("height")
        .or_else(|| props.get("h"))
        .or_else(|| props.get("cy"))
        .map(|v| unit_to_emu(v))
        .unwrap_or_else(|| "1143000".to_string()); // ~1.25 inches
    (x, y, cx, cy)
}

/// Convert units (px, in, cm, mm, pt) to EMU.
fn unit_to_emu(v: &str) -> String {
    let v = v.trim();
    if let Some(num) = v.strip_suffix("px") {
        if let Ok(n) = num.parse::<f64>() {
            let emu = (n * 9525.0) as i64;
            return emu.to_string();
        }
    }
    if let Some(num) = v.strip_suffix("in") {
        if let Ok(n) = num.parse::<f64>() {
            let emu = (n * 914400.0) as i64;
            return emu.to_string();
        }
    }
    if let Some(num) = v.strip_suffix("cm") {
        if let Ok(n) = num.parse::<f64>() {
            let emu = (n * 360000.0) as i64;
            return emu.to_string();
        }
    }
    if let Some(num) = v.strip_suffix("mm") {
        if let Ok(n) = num.parse::<f64>() {
            let emu = (n * 36000.0) as i64;
            return emu.to_string();
        }
    }
    if let Some(num) = v.strip_suffix("pt") {
        if let Ok(n) = num.parse::<f64>() {
            let emu = (n * 12700.0) as i64;
            return emu.to_string();
        }
    }
    v.to_string()
}

/// Add a rectangle shape.
fn add_rectangle(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Rectangle".to_string());
    let text = properties.get("text").cloned().unwrap_or_default();
    let (x, y, cx, cy) = extract_geometry(properties);
    let fill = properties
        .get("fill")
        .or_else(|| properties.get("fillColor"));

    let shape_xml = create_shape_xml_with_geometry(
        next_id,
        &name,
        &text,
        "rect",
        &x,
        &y,
        &cx,
        &cy,
        false,
        fill.map(|s| s.as_str()),
    );

    let modified = insert_shape_in_sp_tree(&slide_xml, &shape_xml);
    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    Ok(format!(
        "/slide[{}]/shape[{}]",
        slide_num,
        slide.shapes.len() + 1
    ))
}

/// Add an ellipse/oval shape.
fn add_ellipse(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Ellipse".to_string());
    let text = properties.get("text").cloned().unwrap_or_default();
    let (x, y, cx, cy) = extract_geometry(properties);
    let fill = properties
        .get("fill")
        .or_else(|| properties.get("fillColor"));

    let shape_xml = create_shape_xml_with_geometry(
        next_id,
        &name,
        &text,
        "ellipse",
        &x,
        &y,
        &cx,
        &cy,
        false,
        fill.map(|s| s.as_str()),
    );

    let modified = insert_shape_in_sp_tree(&slide_xml, &shape_xml);
    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    Ok(format!(
        "/slide[{}]/shape[{}]",
        slide_num,
        slide.shapes.len() + 1
    ))
}

/// Add a line shape.
fn add_line_shape(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Line".to_string());
    let (x, y, cx, cy) = extract_geometry(properties);

    let line_color = properties
        .get("color")
        .or_else(|| properties.get("lineColor"))
        .map(|c| c.strip_prefix('#').unwrap_or(c))
        .unwrap_or("000000");
    let line_w = properties
        .get("lineWidth")
        .map(|v| unit_to_emu(v))
        .unwrap_or_else(|| "12700".to_string());

    let shape_xml = format!(
        r#"<p:cxnSp>
  <p:nvCxnSpPr>
    <p:cNvPr id="{next_id}" name="{name}"/>
    <p:cNvCxnSpPr/>
    <p:nvPr/>
  </p:nvCxnSpPr>
  <p:spPr>
    <a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>
    <a:prstGeom prst="line"><a:avLst/></a:prstGeom>
    <a:ln w="{line_w}"><a:solidFill><a:srgbClr val="{line_color}"/></a:solidFill></a:ln>
  </p:spPr>
</p:cxnSp>"#
    );

    let modified = insert_shape_in_sp_tree(&slide_xml, &shape_xml);
    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    Ok(format!(
        "/slide[{}]/shape[{}]",
        slide_num,
        slide.shapes.len() + 1
    ))
}

/// Add a connector shape (a line between two shapes).
fn add_connector(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    // A connector is essentially a line with optional start/end connection targets.
    add_line_shape(package, parent, properties)
}

/// Add a group shape (empty container for other shapes).
fn add_group(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Group".to_string());
    let (x, y, cx, cy) = extract_geometry(properties);

    let grp_xml = format!(
        r#"<p:grpSp>
  <p:nvGrpSpPr>
    <p:cNvPr id="{next_id}" name="{name}"/>
    <p:cNvGrpSpPr/>
    <p:nvPr/>
  </p:nvGrpSpPr>
  <p:grpSpPr>
    <a:xfrm>
      <a:off x="{x}" y="{y}"/>
      <a:ext cx="{cx}" cy="{cy}"/>
      <a:chOff x="{x}" y="{y}"/>
      <a:chExt cx="{cx}" cy="{cy}"/>
    </a:xfrm>
  </p:grpSpPr>
</p:grpSp>"#
    );

    let modified = insert_shape_in_sp_tree(&slide_xml, &grp_xml);
    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    Ok(format!(
        "/slide[{}]/shape[{}]",
        slide_num,
        slide.shapes.len() + 1
    ))
}

/// Add a picture (image) shape. Requires embedding binary data via a relationship.
/// NOTE: This creates the picture shape XML and updates the relationship; it does
/// not yet write the binary media file. The caller should copy the image file into
/// ppt/media/ separately.
fn add_picture(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    use std::path::Path;

    let src = properties.get("src").or_else(|| properties.get("path"));

    // Resolve image extension — explicit property takes priority, then derive
    // from `src` filename extension. Default to png.
    let ext = properties
        .get("format")
        .or_else(|| properties.get("ext"))
        .map(|s| s.as_str())
        .or_else(|| {
            src.and_then(|p| Path::new(p).extension())
                .and_then(|e| e.to_str())
        })
        .unwrap_or("png");
    let (ext_norm, content_type) = match ext.to_lowercase().as_str() {
        "png" => ("png", "image/png"),
        "jpg" | "jpeg" => ("jpeg", "image/jpeg"),
        "gif" => ("gif", "image/gif"),
        "bmp" => ("bmp", "image/bmp"),
        "tiff" | "tif" => ("tiff", "image/tiff"),
        "webp" => ("webp", "image/webp"),
        "svg" => ("svg", "image/svg+xml"),
        "ico" => ("ico", "image/x-icon"),
        "emf" => ("emf", "image/x-emf"),
        "wmf" => ("wmf", "image/x-wmf"),
        _ => ("png", "image/png"),
    };

    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Picture".to_string());
    let (x, y, cx, cy) = extract_geometry(properties);
    let alt = properties
        .get("alt")
        .or_else(|| properties.get("description"))
        .map(|s| s.as_str())
        .unwrap_or("");

    // Probe for the next free image index and decide on the part path.
    let image_idx = next_ppt_image_index(package, ext_norm);
    let image_filename = format!("image{}.{}", image_idx, ext_norm);
    let media_part_path = format!("ppt/media/{}", image_filename);

    // Write image binary — priority: src file > payloadBase64 > payloadHex > empty stub.
    let bytes_to_write = if let Some(src_path) = src {
        std::fs::read(src_path).ok()
    } else if let Some(b64) = properties.get("payloadBase64") {
        base64_decode(b64).ok()
    } else if let Some(hex) = properties.get("payloadHex") {
        hex_decode(hex).ok()
    } else {
        Some(Vec::new())
    };
    if let Some(bytes) = bytes_to_write {
        let _ = package.write_part(&media_part_path, bytes);
    }

    // Generate a relationship ID for the image
    let rels_path = crate::navigation::relationships_part_path(&slide_path);
    let rels_xml = package
        .read_part_xml(&rels_path)
        .unwrap_or_else(|_| "<Relationships/>".to_string());
    let next_rel_id = format!("rId{}", find_max_rel_id(&rels_xml) + 1);
    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{}\"/>",
        next_rel_id, image_filename
    );

    let modified_rels = if let Some(pos) = rels_xml.find("</Relationships>") {
        let mut result = rels_xml.clone();
        result.insert_str(pos, &new_rel);
        result
    } else if rels_xml.trim() == "<Relationships/>" || rels_xml.trim() == "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>" {
        let mut result = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">".to_string();
        result.push_str(&new_rel);
        result.push_str("</Relationships>");
        result
    } else {
        let mut result = "<Relationships>".to_string();
        result.push_str(&new_rel);
        result.push_str("</Relationships>");
        result
    };
    package
        .write_part_xml(&rels_path, &modified_rels)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Register the image extension in [Content_Types].xml if missing.
    update_ppt_content_types_for_model(package, ext_norm, content_type)?;

    let shape_xml = format!(
        r#"<p:pic>
  <p:nvPicPr>
    <p:cNvPr id="{next_id}" name="{name}" descr="{alt}"/>
    <p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr>
    <p:nvPr/>
  </p:nvPicPr>
  <p:blipFill>
    <a:blip r:embed="{next_rel_id}"/>
    <a:stretch><a:fillRect/></a:stretch>
  </p:blipFill>
  <p:spPr>
    <a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
  </p:spPr>
</p:pic>"#
    );

    let modified = insert_shape_in_sp_tree(&slide_xml, &shape_xml);
    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    Ok(format!(
        "/slide[{}]/shape[{}]",
        slide_num,
        slide.shapes.len() + 1
    ))
}

/// Find next free image index in ppt/media/imageN.<ext>.
fn next_ppt_image_index(package: &OxmlPackage, ext: &str) -> usize {
    let mut i = 1;
    loop {
        let path = format!("ppt/media/image{}.{}", i, ext);
        if package.read_part_xml(&path).is_err() {
            return i;
        }
        i += 1;
    }
}

/// Find the max rId in a relationships XML.
fn find_max_rel_id(xml: &str) -> usize {
    let mut max_id = 0;
    for part in xml.split("Id=\"rId") {
        if let Some(end) = part.find('"') {
            if let Ok(id) = part[..end].parse::<usize>() {
                if id > max_id {
                    max_id = id;
                }
            }
        }
    }
    max_id
}

/// Add a video. Writes the media part (from `payloadBase64` or stub), wires
/// a video relationship, and embeds a `<p:pic>` with an `<a:videoFile>`
/// extension in `<p:nvPr>`. A poster image (`<a:blip r:link>`) is optional;
/// when omitted the slide uses the first video frame as poster on most viewers.
fn add_video(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;

    let video_ext = properties
        .get("format")
        .or_else(|| properties.get("ext"))
        .map(|s| s.as_str())
        .unwrap_or("mp4");
    let (idx, content_type, ext_norm) = next_ppt_video_index(package, video_ext);
    let part_path = format!("ppt/media/video{}.{}", idx, ext_norm);

    // Write the binary payload — from base64 / hex, or an empty stub so the
    // part exists. Real users must overwrite the part with actual bytes.
    if let Some(b64) = properties.get("payloadBase64") {
        if let Ok(bytes) = base64_decode(b64) {
            let _ = package.write_part(&part_path, bytes);
        }
    } else if let Some(hex) = properties.get("payloadHex") {
        if let Ok(bytes) = hex_decode(hex) {
            let _ = package.write_part(&part_path, bytes);
        }
    } else {
        let _ = package.write_part(&part_path, Vec::new());
    }

    // Wire slide→video relationship (Type: video, not image).
    let slide_rels_path = crate::navigation::relationships_part_path(&slide_path);
    let rels_xml = package
        .read_part_xml(&slide_rels_path)
        .unwrap_or_else(|_| "<Relationships/>".to_string());
    let next_rel_id = format!("rId{}", find_max_rel_id(&rels_xml) + 1);
    let video_target = format!("../media/video{}.{}", idx, ext_norm);
    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/video\" Target=\"{}\"/>",
        next_rel_id, video_target
    );
    let modified_rels = if let Some(pos) = rels_xml.find("</Relationships>") {
        let mut r = rels_xml.clone();
        r.insert_str(pos, &new_rel);
        r
    } else if rels_xml.trim() == "<Relationships/>"
        || rels_xml.trim() == "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>"
    {
        let mut r = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">".to_string();
        r.push_str(&new_rel);
        r.push_str("</Relationships>");
        r
    } else {
        let mut r = "<Relationships>".to_string();
        r.push_str(&new_rel);
        r.push_str("</Relationships>");
        r
    };
    package
        .write_part_xml(&slide_rels_path, &modified_rels)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    update_ppt_content_types_for_model(package, ext_norm, content_type)?;

    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;
    let (x, y, cx, cy) = extract_geometry(properties);
    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| format!("Video {}", idx));

    let shape_xml = format!(
        r#"<p:pic>
  <p:nvPicPr>
    <p:cNvPr id="{next_id}" name="{name}"/>
    <p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr>
    <p:nvPr>
      <a:videoFile r:link="{rel_id}"/>
    </p:nvPr>
  </p:nvPicPr>
  <p:blipFill/>
  <p:spPr>
    <a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
  </p:spPr>
</p:pic>"#,
        next_id = next_id,
        name = xml_escape_text(&name),
        rel_id = next_rel_id,
        x = x,
        y = y,
        cx = cx,
        cy = cy
    );

    let modified = insert_shape_in_sp_tree(&slide_xml, &shape_xml);
    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    Ok(format!(
        "/slide[{}]/shape[{}]",
        slide_num,
        slide.shapes.len() + 1
    ))
}

/// Find next free video index in ppt/media/videoN.<ext>.
/// Returns (index, content_type_for_override, normalized_extension).
fn next_ppt_video_index(
    package: &OxmlPackage,
    requested_ext: &str,
) -> (usize, &'static str, &'static str) {
    let lower = requested_ext.to_lowercase();
    let (ext_norm, content_type) = match lower.as_str() {
        "mp4" => ("mp4", "video/mp4"),
        "webm" => ("webm", "video/webm"),
        "mov" => ("mov", "video/quicktime"),
        "avi" => ("avi", "video/x-msvideo"),
        "mkv" => ("mkv", "video/x-matroska"),
        "ogg" | "ogv" => ("ogg", "video/ogg"),
        "wmv" => ("wmv", "video/x-ms-wmv"),
        _ => ("mp4", "video/mp4"),
    };

    let mut i = 1;
    loop {
        let path = format!("ppt/media/video{}.{}", i, ext_norm);
        if package.read_part_xml(&path).is_err() {
            return (i, content_type, ext_norm);
        }
        i += 1;
    }
}

/// Add an audio shape.
fn add_audio(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    add_picture(package, parent, properties)
}

/// Add a table (graphic frame with a:tbl).
fn add_table(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;

    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| "Table".to_string());
    let (x, y, cx, cy) = extract_geometry(properties);
    let cols: usize = properties
        .get("cols")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let rows: usize = properties
        .get("rows")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);

    let col_width = 914400 / cols as i64; // Divide the width by columns
    let row_height = if rows > 0 {
        457200 / rows as i64
    } else {
        457200
    }; // 0.5 inch per row default

    // Build the table grid
    let mut grid_cols = String::new();
    for _ in 0..cols {
        grid_cols.push_str(&format!("<a:gridCol w=\"{}\"/>", col_width));
    }

    // Build rows with cells
    let mut rows_xml = String::new();
    for r in 0..rows {
        let mut cells_xml = String::new();
        for c in 0..cols {
            let cell_text = properties
                .get(&format!("r{}c{}", r + 1, c + 1))
                .cloned()
                .unwrap_or_default();
            let escaped = xml_escape_text(&cell_text);
            cells_xml.push_str(&format!(
                r#"<a:tc>
  <a:txBody>
    <a:bodyPr/>
    <a:lstStyle/>
    <a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{escaped}</a:t></a:r></a:p>
  </a:txBody>
</a:tc>"#
            ));
        }
        rows_xml.push_str(&format!("<a:tr h=\"{}\">{}</a:tr>", row_height, cells_xml));
    }

    let table_xml = format!(
        r#"<p:graphicFrame>
  <p:nvGraphicFramePr>
    <p:cNvPr id="{next_id}" name="{name}"/>
    <p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr>
    <p:nvPr/>
  </p:nvGraphicFramePr>
  <p:xfrm>
    <a:off x="{x}" y="{y}"/>
    <a:ext cx="{cx}" cy="{cy}"/>
  </p:xfrm>
  <a:graphic>
    <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">
      <a:tbl>
        <a:tblPr firstRow="1" bandRow="1"><a:tableStyleId>{{5940675A-B579-4CD6-9FD5-AB1180B14A42}}</a:tableStyleId></a:tblPr>
        <a:tblGrid>{grid_cols}</a:tblGrid>
        {rows_xml}
      </a:tbl>
    </a:graphicData>
  </a:graphic>
</p:graphicFrame>"#
    );

    let modified = insert_shape_in_sp_tree(&slide_xml, &table_xml);
    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    Ok(format!(
        "/slide[{}]/shape[{}]",
        slide_num,
        slide.shapes.len() + 1
    ))
}

/// Build and embed a chart in a PPT slide.
///
/// Charts embed via a `<p:graphicFrame>` directly inside the slide's `<p:spTree>`,
/// referencing the chart part via `r:id`. The chart XML lives in
/// `ppt/charts/chartN.xml` and is linked via the slide's rels.
///
/// Supported properties:
///   type=bar|column|line|pie    (default: column)
///   title=<chart title>          (default: "Chart")
///   categories=A1:A5             (cell range for x-axis labels; literal "a,b,c" also OK)
///   values=1,2,3                 (CSV literal values; or "Sheet1!A1:A5")
///   x, y, width, height          (EMU or "1in"/"2cm" — defaults to 4x3 inches)
fn add_chart_real(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;

    // Chart index — probe parts for the next free number.
    let chart_idx = next_ppt_chart_index(package);
    let chart_path = format!("ppt/charts/chart{}.xml", chart_idx);

    let chart_type = properties
        .get("type")
        .map(|s| s.as_str())
        .unwrap_or("column")
        .to_lowercase();
    let title = properties
        .get("title")
        .cloned()
        .unwrap_or_else(|| "Chart".to_string());
    let categories = properties
        .get("categories")
        .or_else(|| properties.get("cat"))
        .cloned()
        .unwrap_or_else(|| "Cat A,Cat B,Cat C".to_string());
    let values = properties
        .get("values")
        .or_else(|| properties.get("val"))
        .cloned()
        .unwrap_or_else(|| "1,2,3".to_string());

    // Parse categories and values into literal lists so the chart is self-contained.
    let cats: Vec<&str> = categories
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let vals: Vec<f64> = values
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();
    if vals.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "chart requires 'values' as CSV of numbers (e.g. values=1,2,3)".to_string(),
        ));
    }

    // Build chart XML.
    let chart_xml = build_ppt_chart_xml(&chart_type, &title, &cats, &vals)?;
    package
        .write_part_xml(&chart_path, &chart_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Wire slide→chart rels.
    let slide_rels_path = crate::navigation::relationships_part_path(&slide_path);
    let chart_rel_id = next_ppt_rel_id(package, &slide_rels_path);
    let chart_target = format!("../charts/chart{}.xml", chart_idx);
    let rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\" Target=\"{}\"/>",
        chart_rel_id, chart_target
    );
    inject_ppt_relationship(package, &slide_rels_path, &rel_xml)?;

    // Inject <p:graphicFrame> into the slide.
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;
    let (x, y, w, h) = extract_geometry(properties);
    let graphic_xml = format!(
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{}\" name=\"Chart {}\"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></p:xfrm><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/chart\"><c:chart xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" r:id=\"{}\"/></a:graphicData></a:graphic></p:graphicFrame>",
        next_id, chart_idx, x, y, w, h, chart_rel_id
    );
    let new_slide_xml = insert_shape_in_sp_tree(&slide_xml, &graphic_xml);
    package
        .write_part_xml(&slide_path, &new_slide_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    // Update content types.
    update_ppt_content_types_for_chart(package, &chart_path)?;

    // Determine shape index for the result path.
    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    let shape_idx = slide.shapes.len() + 1;

    Ok(format!("/slide[{}]/shape[{}]", slide_num, shape_idx))
}

/// Find next free chart index in ppt/charts/chartN.xml.
fn next_ppt_chart_index(package: &OxmlPackage) -> usize {
    let mut i = 1;
    loop {
        if package
            .read_part_xml(&format!("ppt/charts/chart{}.xml", i))
            .is_err()
        {
            return i;
        }
        i += 1;
    }
}

/// Find next free rId in a rels part.
fn next_ppt_rel_id(package: &OxmlPackage, rels_path: &str) -> String {
    let Ok(xml) = package.read_part_xml(rels_path) else {
        return "rId2".to_string();
    };
    let mut max = 0;
    for hit in xml.match_indices("Id=\"rId") {
        let after = &xml[hit.0 + "Id=\"rId".len()..];
        if let Some(end) = after.find('"') {
            if let Ok(n) = after[..end].parse::<usize>() {
                if n > max {
                    max = n;
                }
            }
        }
    }
    format!("rId{}", max + 1)
}

/// Insert a <Relationship/> into a .rels part, creating the part if missing.
fn inject_ppt_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    rel_xml: &str,
) -> Result<(), HandlerError> {
    let existing = package.read_part_xml(rels_path).ok();
    let new = match existing {
        Some(xml) if xml.contains("</Relationships>") => {
            xml.replace("</Relationships>", &format!("{}</Relationships>", rel_xml))
        }
        _ => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">{}</Relationships>",
            rel_xml
        ),
    };
    package
        .write_part_xml(rels_path, &new)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

/// Append chart override to [Content_Types].xml if missing.
fn update_ppt_content_types_for_chart(
    package: &mut OxmlPackage,
    chart_path: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let override_xml = format!(
        "<Override PartName=\"/{}\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>",
        chart_path
    );
    if xml.contains(&override_xml) {
        return Ok(());
    }
    let new_xml = xml.replace("</Types>", &format!("{}</Types>", override_xml));
    package
        .write_part_xml("[Content_Types].xml", &new_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

/// Build ppt/charts/chartN.xml content with inline literal data.
fn build_ppt_chart_xml(
    chart_type: &str,
    title: &str,
    cats: &[&str],
    vals: &[f64],
) -> Result<String, HandlerError> {
    let (bar_dir, bar_dir_xml, grouping_xml): (&str, &str, &str) = match chart_type {
        "bar" => (
            "bar",
            "<c:barDir val=\"bar\"/>",
            "<c:grouping val=\"clustered\"/>",
        ),
        "column" | "col" => (
            "col",
            "<c:barDir val=\"col\"/>",
            "<c:grouping val=\"clustered\"/>",
        ),
        "line" => ("line", "", "<c:grouping val=\"standard\"/>"),
        "pie" => ("pie", "", ""),
        other => {
            return Err(HandlerError::InvalidArgument(format!(
                "unsupported chart type '{}'; supported: bar, column, line, pie",
                other
            )))
        }
    };

    let cats_xml =
        format!(
        "<c:cat><c:strLit><c:strCache><c:ptCount val=\"{}\"/>{}</c:strCache></c:strLit></c:cat>",
        cats.len(),
        cats.iter()
            .enumerate()
            .map(|(i, c)| format!("<c:pt idx=\"{}\"><c:v>{}</c:v></c:pt>", i, xml_escape_text(c)))
            .collect::<String>()
    );
    let vals_xml = format!(
        "<c:val><c:numLit><c:numCache><c:formatCode>General</c:formatCode><c:ptCount val=\"{}\"/>{}</c:numCache></c:numLit></c:val>",
        vals.len(),
        vals.iter()
            .enumerate()
            .map(|(i, v)| format!("<c:pt idx=\"{}\"><c:v>{}</c:v></c:pt>", i, v))
            .collect::<String>()
    );
    let series_xml = format!(
        "<c:ser><c:idx val=\"0\"/><c:order val=\"0\"/><c:tx><c:v>Series 1</c:v></c:tx>{}{}</c:ser>",
        cats_xml, vals_xml
    );

    let plot_xml = if bar_dir == "pie" {
        format!(
            "<c:pieChart>{}<c:varyColors val=\"0\"/><c:firstSliceAng val=\"0\"/></c:pieChart>",
            series_xml
        )
    } else {
        format!(
            "<c:{}Chart>{}{}<c:varyColors val=\"0\"/>{}</c:{}Chart>",
            bar_dir, bar_dir_xml, grouping_xml, series_xml, bar_dir
        )
    };

    let title_xml = format!(
        "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title>",
        xml_escape_text(title)
    );

    let axes_xml = if bar_dir == "pie" {
        String::new()
    } else {
        format!(
            "<c:plotArea>{}<c:catAx><c:axId val=\"1\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:crossAx val=\"2\"/></c:catAx><c:valAx><c:axId val=\"2\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:crossAx val=\"1\"/></c:valAx></c:plotArea>",
            plot_xml
        )
    };

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    xml.push_str(
        "<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" ",
    );
    xml.push_str("xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" ");
    xml.push_str(
        "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">",
    );
    xml.push_str("<c:chart>");
    xml.push_str(&title_xml);
    xml.push_str("<c:autoTitleDeleted val=\"0\"/>");
    xml.push_str(&axes_xml);
    xml.push_str("<c:plotVisOnly val=\"1\"/>");
    xml.push_str("<c:dispBlanksAs val=\"gap\"/>");
    xml.push_str("</c:chart></c:chartSpace>");

    Ok(xml)
}

/// Add a 3D model reference to a slide.
///
/// Writes the 3D model part (modelN.glb or modelN.xml), wires the slide→model
/// relationship, and injects an `<mc:AlternateContent>` block into the slide's
/// spTree that hosts the `<p:graphicFrame>` with a `<thm15:model3d>` graphicData.
/// The block degrades gracefully to a `<p:sp>` fallback (per the ECMA-376
/// model3d specification).
fn add_model3d_real(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;

    let model_ext = properties
        .get("format")
        .or_else(|| properties.get("ext"))
        .map(|s| s.as_str())
        .unwrap_or("glb");

    // Probe for next free model index and pick a MIME type per extension.
    let (model_idx, content_type, model_ext_lower) = next_ppt_model_index(package, model_ext);

    let model_part_path = format!("ppt/media/model{}.{}", model_idx, model_ext_lower);

    // Write a placeholder payload if the caller gave us bytes; otherwise emit a
    // minimal valid .glb header so the part exists. Real users must overwrite
    // this part with actual model bytes (via raw-set / file copy).
    if let Some(payload_b64) = properties.get("payloadBase64") {
        if let Ok(bytes) = base64_decode(payload_b64) {
            let _ = package.write_part(&model_part_path, bytes);
        }
    } else if let Some(payload_hex) = properties.get("payloadHex") {
        if let Ok(bytes) = hex_decode(payload_hex) {
            let _ = package.write_part(&model_part_path, bytes);
        }
    } else {
        // Minimal valid GLB v2 (Khronos spec): 12-byte header + JSON chunk
        // carrying `{"asset":{"version":"2.0"}}`. PowerPoint accepts the part
        // even without geometry; downstream viewers (Three.js etc.) require
        // a valid JSON chunk to load without error.
        let minimal = minimal_glb_v2();
        let _ = package.write_part(&model_part_path, minimal);
    }

    // Wire slide→model rel.
    let slide_rels_path = crate::navigation::relationships_part_path(&slide_path);
    let model_rel_id = next_ppt_rel_id(package, &slide_rels_path);
    let model_target = format!("../media/model{}.{}", model_idx, model_ext_lower);
    let model_rel_xml = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.microsoft.com/office/2017/10/relationships/model3d\" Target=\"{}\"/>",
        model_rel_id, model_target
    );
    inject_ppt_relationship(package, &slide_rels_path, &model_rel_xml)?;

    // Update [Content_Types].xml if extension is new.
    update_ppt_content_types_for_model(package, model_ext_lower, content_type)?;

    // Read slide and inject AlternateContent graphicFrame.
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let next_id = find_max_id(&slide_xml) + 1;
    let (x, y, w, h) = extract_geometry(properties);
    let name = properties
        .get("name")
        .cloned()
        .unwrap_or_else(|| format!("3D Model {}", model_idx));

    let frame_xml = format!(
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
  <mc:Choice xmlns:p159="http://schemas.microsoft.com/office/2017/10/relationships" Requires="p159">
    <p:graphicFrame>
      <p:nvGraphicFramePr>
        <p:cNvPr id="{next_id}" name="{name}"/>
        <p:cNvGraphicFramePr/>
        <p:nvPr/>
      </p:nvGraphicFramePr>
      <p:xfrm>
        <a:off x="{x}" y="{y}"/>
        <a:ext cx="{w}" cy="{h}"/>
      </p:xfrm>
      <a:graphic>
        <a:graphicData uri="http://schemas.microsoft.com/office/2017/10/model3d">
          <thm15:model3d xmlns:thm15="http://schemas.microsoft.com/office/threed/2015/model3d" r:id="{model_rel_id}"/>
        </a:graphicData>
      </a:graphic>
    </p:graphicFrame>
  </mc:Choice>
  <mc:Fallback>
    <p:sp>
      <p:nvSpPr>
        <p:cNvPr id="{next_id}" name="{name} (3D Model — fallback)"/>
        <p:cNvSpPr/>
        <p:nvPr/>
      </p:nvSpPr>
      <p:spPr>
        <a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>
        <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
      </p:spPr>
      <p:txBody>
        <a:bodyPr/><a:lstStyle/>
        <a:p><a:endParaRPr lang="en-US"/></a:p>
      </p:txBody>
    </p:sp>
  </mc:Fallback>
</mc:AlternateContent>"#,
        next_id = next_id,
        name = xml_escape_text(&name),
        x = x,
        y = y,
        w = w,
        h = h,
        model_rel_id = model_rel_id
    );

    let new_slide_xml = insert_shape_in_sp_tree(&slide_xml, &frame_xml);
    package
        .write_part_xml(&slide_path, &new_slide_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    let pres = crate::navigation::build_presentation(package)?;
    let slide = pres
        .slides
        .iter()
        .find(|s| s.index == slide_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    let shape_idx = slide.shapes.len() + 1;

    Ok(format!("/slide[{}]/shape[{}]", slide_num, shape_idx))
}

/// Find next free model index in ppt/media/modelN.<ext>. Returns
/// (index, content_type_for_override, normalized_extension).
fn next_ppt_model_index(
    package: &OxmlPackage,
    requested_ext: &str,
) -> (usize, &'static str, &'static str) {
    let lower = requested_ext.to_lowercase();
    let (ext_norm, content_type) = match lower.as_str() {
        "glb" => ("glb", "model/gltf-binary"),
        "gltf" => ("gltf", "model/gltf+json"),
        "obj" => ("obj", "model/obj"),
        "fbx" => ("fbx", "application/octet-stream"),
        "stl" => ("stl", "model/stl"),
        "3mf" => ("3mf", "application/vnd.ms-package.3dmanufacturing-3d"),
        other => (
            match other {
                "dae" => "dae",
                "ply" => "ply",
                _ => "glb",
            },
            "application/octet-stream",
        ),
    };

    let mut i = 1;
    loop {
        let path = format!("ppt/media/model{}.{}", i, ext_norm);
        if package.read_part_xml(&path).is_err() {
            return (i, content_type, ext_norm);
        }
        i += 1;
    }
}

fn update_ppt_content_types_for_model(
    package: &mut OxmlPackage,
    ext: &str,
    content_type: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let ext_attr = format!("Extension=\"{}\"", ext);
    if xml.contains(&ext_attr) {
        return Ok(());
    }
    let default_xml = format!(
        "<Default Extension=\"{}\" ContentType=\"{}\"/>",
        ext, content_type
    );
    // Insert Default at the top so Override entries stay grouped after.
    let new_xml = if let Some(pos) = xml.find('>') {
        // Right after opening <Types ...>
        let close = pos + 1;
        let mut out = String::with_capacity(xml.len() + default_xml.len());
        out.push_str(&xml[..close]);
        out.push_str(&default_xml);
        out.push_str(&xml[close..]);
        out
    } else {
        xml.replace("</Types>", &format!("{}{}</Types>", default_xml, ""))
    };
    package
        .write_part_xml("[Content_Types].xml", &new_xml)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(())
}

/// Decode standard base64 (RFC 4648). Whitespace tolerant.
fn base64_decode(s: &str) -> Result<Vec<u8>, ()> {
    let mut bits: u32 = 0;
    let mut nbits: u32 = 0;
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for c in s.chars().filter(|c| !c.is_whitespace()) {
        let v: u32 = match c {
            'A'..='Z' => (c as u32) - ('A' as u32),
            'a'..='z' => (c as u32) - ('a' as u32) + 26,
            '0'..='9' => (c as u32) - ('0' as u32) + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            '=' => break,
            _ => return Err(()),
        };
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Ok(out)
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !cleaned.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let chars: Vec<char> = cleaned.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let byte =
            u8::from_str_radix(&format!("{}{}", chars[i], chars[i + 1]), 16).map_err(|_| ())?;
        out.push(byte);
        i += 2;
    }
    Ok(out)
}

/// Minimum conformant GLB v2 (binary glTF). 12-byte header + JSON chunk
/// carrying `{"asset":{"version":"2.0"}}` (the only field the spec
/// requires). PowerPoint stores model parts as .glb; without this fallback
/// the part would be missing and the slide→model relationship would dangle.
/// Callers with real model bytes overwrite the part via `payloadBase64` /
/// `payloadHex` / raw-set.
fn minimal_glb_v2() -> Vec<u8> {
    // JSON asset spec: only `asset.version` is required.
    let json = br#"{"asset":{"version":"2.0"}}"#;
    // GLB chunk data must be padded to 4-byte alignment with 0x20 (space).
    let pad_len = (4 - (json.len() % 4)) % 4;
    let json_chunk_len = json.len() + pad_len;
    let total_len = 12 + 8 + json_chunk_len as u32;
    let mut v = Vec::with_capacity(total_len as usize);
    // ── header (12 bytes) ──
    v.extend_from_slice(b"glTF"); // magic 0x46546C67
    v.extend_from_slice(&2u32.to_le_bytes()); // version
    v.extend_from_slice(&total_len.to_le_bytes()); // total length
                                                   // ── JSON chunk ──
    v.extend_from_slice(&(json_chunk_len as u32).to_le_bytes());
    v.extend_from_slice(&0x4E4F534Au32.to_le_bytes()); // "JSON"
    v.extend_from_slice(json);
    v.extend(std::iter::repeat_n(0x20u8, pad_len));
    v
}

/// Add a slide comment (creates comments.xml if needed).
fn add_comment(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let comments_path = format!("ppt/comments/comment{}.xml", slide_num);

    let author = properties
        .get("author")
        .or_else(|| properties.get("initials"))
        .map(|s| s.as_str())
        .unwrap_or("officecli");
    let text = properties.get("text").map(|s| s.as_str()).unwrap_or("");
    let escaped = xml_escape_text(text);

    // Try to read existing comments; if not present, create new
    let existing = package.read_part_xml(&comments_path).unwrap_or_default();
    let next_id = find_max_id(&existing) + 1;

    let comment_xml = if existing.is_empty() {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:cmLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cm author="{author}" dt="2024-01-01T00:00:00Z" idx="{next_id}">
    <p:text><a:t>{escaped}</a:t></p:text>
  </p:cm>
</p:cmLst>"#
        )
    } else {
        // Insert into existing
        let new_cm = format!(
            r#"<p:cm author="{author}" dt="2024-01-01T00:00:00Z" idx="{next_id}">
    <p:text><a:t>{escaped}</a:t></p:text>
  </p:cm>"#
        );
        if let Some(pos) = existing.find("</p:cmLst>") {
            let mut result = existing.clone();
            result.insert_str(pos, &new_cm);
            result
        } else {
            existing.clone()
        }
    };

    package
        .write_part_xml(&comments_path, &comment_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    Ok(format!("/slide[{}]/comment[{}]", slide_num, next_id))
}

/// Add a speaker note.
fn add_note(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let notes_path = format!("ppt/notesSlides/notesSlide{}.xml", slide_num);
    let text = properties.get("text").map(|s| s.as_str()).unwrap_or("");
    let escaped = xml_escape_text(text);

    let notes_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr/>
      <p:sp>
        <p:nvSpPr>
          <p:cNvPr id="2" name="Notes Placeholder"/>
          <p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>
          <p:nvPr><p:ph type="body" idx="1"/></p:nvPr>
        </p:nvSpPr>
        <p:spPr/>
        <p:txBody>
          <a:bodyPr/>
          <a:lstStyle/>
          <a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{escaped}</a:t></a:r></a:p>
        </p:txBody>
      </p:sp>
    </p:spTree>
  </p:cSld>
</p:notes>"#
    );

    package
        .write_part_xml(&notes_path, &notes_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    Ok(format!("/slide[{}]/notes", slide_num))
}

/// Add a hyperlink to a shape's text.
fn add_hyperlink(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let url = properties
        .get("url")
        .or_else(|| properties.get("target"))
        .ok_or_else(|| {
            HandlerError::InvalidArgument("hyperlink requires 'url' or 'target'".to_string())
        })?;
    // Reject javascript:, data:, vbscript: targets before they round-trip
    // into a slide rels file. See handler_common::hyperlink_validator.
    if let Err(msg) = handler_common::hyperlink_validator::require_safe_scheme(url, "hyperlink") {
        return Err(HandlerError::InvalidArgument(msg));
    }

    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Add relationship for the URL
    let rels_path = crate::navigation::relationships_part_path(&slide_path);
    let rels_xml = package
        .read_part_xml(&rels_path)
        .unwrap_or_else(|_| "<Relationships/>".to_string());
    let next_rel_id = format!("rId{}", find_max_rel_id(&rels_xml) + 1);

    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"{}\" TargetMode=\"External\"/>",
        next_rel_id, url
    );

    let modified_rels = if let Some(pos) = rels_xml.find("</Relationships>") {
        let mut result = rels_xml.clone();
        result.insert_str(pos, &new_rel);
        result
    } else {
        let mut result = "<Relationships>".to_string();
        result.push_str(&new_rel);
        result.push_str("</Relationships>");
        result
    };
    package
        .write_part_xml(&rels_path, &modified_rels)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Inject <a:hlinkClick r:id="rIdN"/> into the targeted run's <a:rPr>.
    // If `text=foo` is provided, find the first run whose <a:t> contains
    // `foo`. Otherwise, link every run in the slide.
    let needle = properties.get("text").map(|s| s.as_str());
    let mut modified_slide = slide_xml.clone();
    let mut count = 0;
    loop {
        let next_target = find_run_for_hyperlink(&modified_slide, needle, count);
        let (run_open_start, run_open_end, rpr_open_start, rpr_open_end, has_rpr) =
            match next_target {
                Some(t) => t,
                None => break,
            };
        modified_slide = inject_hlink_click_into_run(
            &modified_slide,
            run_open_start,
            run_open_end,
            rpr_open_start,
            rpr_open_end,
            has_rpr,
            &next_rel_id,
        );
        count += 1;
        if needle.is_none() {
            // If no text filter, we only tag the first run by default — caller
            // can pass --properties target=all to tag every run.
            let tag_all = properties
                .get("target")
                .map(|s| s == "all")
                .unwrap_or(false)
                || properties.get("scope").map(|s| s == "all").unwrap_or(false);
            if !tag_all {
                break;
            }
        }
    }

    if count == 0 {
        // No matching run found — still keep the relationship we wrote above
        // so callers can attach the hlinkClick manually via raw-set if needed.
        return Err(HandlerError::PathNotFound(format!(
            "no run with text '{}' found on slide {}",
            needle.unwrap_or("(any)"),
            slide_num
        )));
    }

    package
        .write_part_xml(&slide_path, &modified_slide)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(format!("/slide[{}]/hyperlink/{}", slide_num, next_rel_id))
}

/// Locate the next `<a:r>` whose `<a:t>` contains `needle` (or the next run
/// when `needle` is None). Returns absolute byte offsets within `xml`:
/// `(run_open_start, run_open_end, rpr_open_start, rpr_open_end, has_rpr)`.
/// `has_rpr` is false when the run has no `<a:rPr .../>` element yet.
fn find_run_for_hyperlink(
    xml: &str,
    needle: Option<&str>,
    skip: usize,
) -> Option<(usize, usize, usize, usize, bool)> {
    let mut skipped = 0;
    let bytes = xml.as_bytes();
    let mut search_from = 0;
    while search_from < xml.len() {
        let run_start = find_byte_substring(bytes, b"<a:r>", search_from)
            .or_else(|| find_byte_substring(bytes, b"<a:r ", search_from))?;
        let run_open_end = run_start + 4;
        // Find the matching </a:r>
        let run_end_close = find_byte_substring(bytes, b"</a:r>", run_open_end)?;
        let run_body = &xml[run_open_end..run_end_close];
        if needle.is_none_or(|n| {
            // Extract <a:t>...</a:t> text and check containment.
            let text = extract_a_t_text(run_body);
            text.contains(n)
        }) {
            if skipped < skip {
                skipped += 1;
                search_from = run_end_close + 6;
                continue;
            }
            // Check for an existing <a:rPr ...> element.
            let rpr_open_start =
                find_byte_substring(bytes, b"<a:rPr", run_open_end).filter(|&p| p < run_end_close);
            if let Some(rs) = rpr_open_start {
                // Either self-closing or opening tag — find end of the tag.
                let after = &xml[rs..run_end_close];
                let tag_end_rel = after
                    .find("/>")
                    .map(|p| p + 2)
                    .or_else(|| after.find('>').map(|p| p + 1))
                    .unwrap_or(after.len());
                let rpr_open_end = rs + tag_end_rel;
                return Some((run_start, run_open_end, rs, rpr_open_end, true));
            }
            return Some((run_start, run_open_end, 0, 0, false));
        } else {
            search_from = run_end_close + 6;
        }
    }
    None
}

/// Insert the hlinkClick element into a run's rPr (or synthesize an rPr if
/// none exists yet). Returns the new XML string.
///
/// Two cases for an existing rPr tag:
/// 1. Self-closing: `<a:rPr .../>` → `<a:rPr ...><a:hlinkClick/></a:rPr>`
/// 2. With closing tag: `<a:rPr ...>...</a:rPr>` → insert hlinkClick just
///    before `</a:rPr>`. This is the common case when rPr has children
///    (solidFill, latin, etc.).
fn inject_hlink_click_into_run(
    xml: &str,
    _run_open_start: usize,
    run_open_end: usize,
    rpr_open_start: usize,
    rpr_open_end: usize,
    has_rpr: bool,
    rel_id: &str,
) -> String {
    let hlink = format!("<a:hlinkClick r:id=\"{}\"/>", rel_id);
    if !has_rpr {
        // Synthesize `<a:rPr><a:hlinkClick/></a:rPr>` right after `<a:r` token.
        let synthetic = format!("<a:rPr><a:hlinkClick r:id=\"{}\"/></a:rPr>", rel_id);
        let mut out = String::with_capacity(xml.len() + synthetic.len());
        out.push_str(&xml[..run_open_end]);
        out.push_str(&synthetic);
        out.push_str(&xml[run_open_end..]);
        return out;
    }
    let rpr_slice = &xml[rpr_open_start..rpr_open_end];
    if rpr_slice.ends_with("/>") {
        // Self-closing case: strip `/>` and add `>hlink</a:rPr>`.
        let (before_slash, _) = rpr_slice.split_at(rpr_slice.len() - 2);
        let new_rpr = format!("{}>{}</a:rPr>", before_slash, hlink);
        let mut out = String::with_capacity(xml.len() + new_rpr.len());
        out.push_str(&xml[..rpr_open_start]);
        out.push_str(&new_rpr);
        out.push_str(&xml[rpr_open_end..]);
        out
    } else {
        // Opening tag — find the matching `</a:rPr>` and insert hlinkClick
        // just before it. We search forward from rpr_open_end for the first
        // `</a:rPr>` (PPTX runs have at most one rPr, so depth isn't a concern).
        let bytes = xml.as_bytes();
        let close_rel = xml[rpr_open_end..]
            .find("</a:rPr>")
            .map(|p| p + rpr_open_end)
            .unwrap_or(rpr_open_end);
        let _ = bytes;
        let mut out = String::with_capacity(xml.len() + hlink.len());
        out.push_str(&xml[..close_rel]);
        out.push_str(&hlink);
        out.push_str(&xml[close_rel..]);
        out
    }
}

fn extract_a_t_text(body: &str) -> String {
    let mut out = String::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < body.len() {
        let open = match find_byte_substring(bytes, b"<a:t", i) {
            Some(p) => p,
            None => break,
        };
        let after_open = &body[open..];
        let gt = match after_open.find('>') {
            Some(p) => open + p + 1,
            None => break,
        };
        let close_rel = match body[gt..].find("</a:t>") {
            Some(p) => gt + p,
            None => break,
        };
        out.push_str(&body[gt..close_rel]);
        i = close_rel + 6;
    }
    out
}

fn find_byte_substring(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

// ─────────────────────────────────────────────────────────────────────────
// Transitions
//
// Three OOXML shapes:
//   (a) bare `<p:transition><p:fade/></p:transition>` for built-in
//       PresentationML 2006 transitions (fade, push, wipe, cut, dissolve,
//       cover, split, strips, blinds, checker, zoom, newsflash, plus,
//       wedge, circle, diamond, comb, pan, orson, pull, random, randomBar).
//   (b) `<mc:AlternateContent>` wrapping a `<p14:transition>` for the
//       PowerPoint 2010+ advanced transitions (vortex, switch, flip,
//       ripple, glitter, honeycomb, sparkle, gallery, etc.).
//   (c) `<mc:AlternateContent>` wrapping a `<p15:prstTrans prst="..."/>`
//       for the PowerPoint 2013+ "Exciting" preset transitions (box,
//       fallOver, drape, curtains, wind, prestige, fracture, crush,
//       peelOff, pageCurlDouble, pageCurlSingle, airplane, origami).
//
// Morph is a special case of (b): `<p14:transition><p14:morphPr/></p14:transition>`.

/// Known basic transitions that live in the `p:` namespace without a wrapper.
const BASIC_P_TRANSITIONS: &[&str] = &[
    "fade",
    "cut",
    "push",
    "wipe",
    "pull",
    "cover",
    "split",
    "dissolve",
    "strips",
    "blinds",
    "checker",
    "zoom",
    "newsflash",
    "plus",
    "wedge",
    "circle",
    "diamond",
    "comb",
    "orson",
    "pan",
    "random",
    "randomBar",
];

/// Known p14 advanced transitions. Each is written as a self-closing child
/// element of `<p14:transition>` in the Choice branch of an AlternateContent.
const P14_TRANSITIONS: &[&str] = &[
    "vortex",
    "switch",
    "flip",
    "ripple",
    "glitter",
    "honeycomb",
    "glitter",
    "sparkle",
    "gallery",
    "cube",
    "rotate",
    "box",
    "orbit",
    "wave",
];

/// Known p15 prstTrans tokens. These map a CLI key to the @prst value written
/// to `<p15:prstTrans prst="..."/>`.
const P15_PRST_TRANS: &[&str] = &[
    "box",
    "fallOver",
    "drape",
    "curtains",
    "wind",
    "prestige",
    "fracture",
    "crush",
    "peelOff",
    "pageCurlDouble",
    "pageCurlSingle",
    "airplane",
    "origami",
];

fn add_transition(
    package: &mut OxmlPackage,
    parent: &str,
    props: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_path = resolve_slide_path(package, parent)?;
    let kind = props
        .get("type")
        .or_else(|| props.get("transition"))
        .cloned()
        .unwrap_or_else(|| "fade".to_string());

    let xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let attrs = build_transition_attrs(props);
    let transition_xml = render_transition_xml(&kind, &attrs, props)?;

    // Replace any existing <p:transition>, <mc:AlternateContent> wrapping a
    // transition, or unknown-element transition. We do this byte-wise so we
    // also nuke legacy `<p:transition>...</p:transition>` siblings a caller
    // may have added earlier via raw-set.
    let cleaned = strip_existing_transition(&xml);
    let new_slide = inject_transition_xml(&cleaned, &transition_xml)?;

    package
        .write_part_xml(&slide_path, &new_slide)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(format!("Added transition '{}' on {}", kind, parent))
}

/// Resolve a slide path from a path like `/slide[2]` or `ppt/slides/slide2.xml`.
/// Falls back to the first slide when the parent isn't a slide path.
fn resolve_slide_path(package: &OxmlPackage, parent: &str) -> Result<String, HandlerError> {
    if let Some(n) = extract_slide_number(parent) {
        return crate::navigation::resolve_slide_part_path(package, n);
    }
    if parent.starts_with("ppt/slides/") {
        return Ok(parent.to_string());
    }
    // Default to first slide.
    let first = package
        .list_parts()
        .iter()
        .filter(|p| p.starts_with("ppt/slides/slide") && p.ends_with(".xml"))
        .min()
        .cloned()
        .ok_or_else(|| HandlerError::PathNotFound("no slides found".into()))?;
    Ok(first.to_string())
}

fn extract_slide_number(path: &str) -> Option<usize> {
    let open = path.find('[')?;
    let close = path[open..].find(']')? + open;
    path[open + 1..close].parse::<usize>().ok()
}

fn build_transition_attrs(props: &HashMap<String, String>) -> String {
    let mut attrs = String::new();
    if let Some(dur) = props.get("duration").or_else(|| props.get("dur")) {
        if validate_ms(dur) {
            attrs.push_str(&format!(" dur=\"{}\"", escape_xml_attr(dur)));
        }
    }
    if let Some(adv_t) = props.get("advanceTime").or_else(|| props.get("advTm")) {
        if validate_ms(adv_t) {
            attrs.push_str(&format!(" advTm=\"{}\"", escape_xml_attr(adv_t)));
        }
    }
    // advanceOnClick defaults to schema true; only write when explicitly false.
    let adv_click = props
        .get("advanceOnClick")
        .or_else(|| props.get("advClick"));
    if let Some(v) = adv_click {
        if matches!(v.to_ascii_lowercase().as_str(), "false" | "0" | "no") {
            attrs.push_str(" advClick=\"0\"");
        }
    }
    if let Some(speed) = props.get("speed") {
        if matches!(speed.as_str(), "slow" | "medium" | "fast") {
            attrs.push_str(&format!(" spd=\"{}\"", speed));
        }
    }
    attrs
}

fn validate_ms(v: &str) -> bool {
    v.parse::<i64>().map(|n| n >= 0).unwrap_or(false)
}

fn escape_xml_attr(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Build the XML fragment to splice into the slide just before `</p:cSld>`.
fn render_transition_xml(
    kind: &str,
    attrs: &str,
    props: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let kind_lc = kind.to_ascii_lowercase();
    let direction = props
        .get("direction")
        .or_else(|| props.get("dir"))
        .map(|s| s.as_str())
        .unwrap_or("l");

    // (a) basic transitions
    if BASIC_P_TRANSITIONS.contains(&kind_lc.as_str()) {
        // Direction-aware transitions take a `dir="…"` attribute on their child.
        let dir_attr = transition_dir_attr(&kind_lc, direction);
        return Ok(format!(
            "<p:transition{}><p:{}{}/></p:transition>",
            attrs, kind_lc, dir_attr
        ));
    }

    // (b) morph (special case)
    if kind_lc == "morph" {
        let option = props
            .get("option")
            .map(|s| s.as_str())
            .unwrap_or("byObject");
        return Ok(format!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
  <mc:Choice xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" Requires="p14">
    <p14:transition{}><p14:morphPr option="{}"/></p14:transition>
  </mc:Choice>
  <mc:Fallback>
    <p:transition{}/>
  </mc:Fallback>
</mc:AlternateContent>"#,
            attrs,
            escape_xml_attr(option),
            attrs
        ));
    }

    // (c) other p14 advanced transitions
    if P14_TRANSITIONS.contains(&kind_lc.as_str()) {
        return Ok(format!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
  <mc:Choice xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main" Requires="p14">
    <p14:transition{}><p14:{}/></p14:transition>
  </mc:Choice>
  <mc:Fallback>
    <p:transition{}/>
  </mc:Fallback>
</mc:AlternateContent>"#,
            attrs, kind_lc, attrs
        ));
    }

    // (d) p15 prstTrans preset transitions
    if P15_PRST_TRANS.iter().any(|t| t.eq_ignore_ascii_case(kind)) {
        // The CLI key pageCurlDouble → CLI tag pageCurlDouble. p15 stores the
        // exact same token in @prst, so no mapping needed.
        let prst_token = P15_PRST_TRANS
            .iter()
            .find(|t| t.eq_ignore_ascii_case(kind))
            .copied()
            .unwrap_or(kind);
        return Ok(format!(
            r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
  <mc:Choice xmlns:p15="http://schemas.microsoft.com/office/powerpoint/2012/main" Requires="p15">
    <p15:prstTrans{} prst="{}"/>
  </mc:Choice>
  <mc:Fallback>
    <p:transition{}/>
  </mc:Fallback>
</mc:AlternateContent>"#,
            attrs, prst_token, attrs
        ));
    }

    Err(HandlerError::InvalidArgument(format!(
        "Unknown transition type '{}'. Basic types: fade, cut, push, wipe, dissolve, \
         cover, split, strips, blinds, checker, zoom, newsflash, plus, wedge, circle, \
         diamond, comb, pan, orson, pull, random, randomBar. P14 advanced: vortex, \
         switch, flip, ripple, glitter, honeycomb, sparkle, gallery, cube, rotate, \
         box, orbit, wave. P15 presets: box, fallOver, drape, curtains, wind, \
         prestige, fracture, crush, peelOff, pageCurlDouble, pageCurlSingle, \
         airplane, origami. Morph: morph.",
        kind
    )))
}

/// Most direction-bearing transitions (push, wipe, cover, pull, split, strips,
/// blinds, checker, comb, pan) take `dir` with values l, r, u, d (left/right/
/// up/down). For wedge and zoom only specific values are valid.
fn transition_dir_attr(kind: &str, direction: &str) -> String {
    let valid_for = |k: &str| match k {
        "push" | "wipe" | "cover" | "pull" | "split" | "strips" | "blinds" | "checker" | "comb"
        | "pan" => matches!(direction, "l" | "r" | "u" | "d" | "lu" | "ru" | "ld" | "rd"),
        "zoom" => matches!(direction, "in" | "out"),
        _ => false,
    };
    if valid_for(kind) {
        format!(" dir=\"{}\"", direction)
    } else {
        String::new()
    }
}

/// Remove every existing transition element (bare `<p:transition>` OR
/// `<mc:AlternateContent>` that wraps a transition) from `slide_xml`.
fn strip_existing_transition(slide_xml: &str) -> String {
    let mut out = slide_xml.to_string();
    // Strip mc:AlternateContent blocks that contain a transition element.
    while let Some(open) = out.find("<mc:AlternateContent") {
        let close = match find_alt_content_close(&out, open) {
            Some(c) => c,
            None => break,
        };
        let block = &out[open..close];
        if block.contains("<p:transition")
            || block.contains("<p14:transition")
            || block.contains("prstTrans")
        {
            out.replace_range(open..close, "");
        } else {
            // Keep this AlternateContent block; advance past it so we don't
            // re-scan it forever.
            break;
        }
    }
    // Strip bare <p:transition ...> ... </p:transition> or <p:transition .../>.
    out = strip_named_element(&out, "p:transition");
    out
}

/// Find the index just past the closing `</mc:AlternateContent>` after the
/// opening tag at `open`. Scans forward, tracking depth via open vs close
/// occurrences of the element name.
fn find_alt_content_close(s: &str, open: usize) -> Option<usize> {
    // Start scanning just past `<mc:AlternateContent`.
    let mut cursor = open + "<mc:AlternateContent".len();
    let mut depth: i32 = 1;
    while cursor < s.len() {
        let next_open = s[cursor..].find("<mc:AlternateContent").map(|p| cursor + p);
        let next_close = s[cursor..]
            .find("</mc:AlternateContent>")
            .map(|p| cursor + p);
        match (next_open, next_close) {
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                cursor = o + "<mc:AlternateContent".len();
            }
            (_, Some(c)) => {
                depth -= 1;
                let end = c + "</mc:AlternateContent>".len();
                if depth == 0 {
                    return Some(end);
                }
                cursor = end;
            }
            (Some(o), None) => {
                depth += 1;
                cursor = o + "<mc:AlternateContent".len();
            }
            (None, None) => return None,
        }
    }
    None
}

/// Strip every `<prefix:name …>…</prefix:name>` or `<prefix:name …/>` from `xml`.
/// Walks the opening tag char-by-char to find its real close `>`, then either
/// consumes a self-closing form or scans to the matching close tag.
fn strip_named_element(xml: &str, qualified_name: &str) -> String {
    let mut out = xml.to_string();
    let open_tag_pat = format!("<{}", qualified_name);
    let close_tag_pat = format!("</{}>", qualified_name);
    loop {
        let Some(open) = out.find(&open_tag_pat) else {
            break;
        };
        // Make sure the tag really starts here — char after must be whitespace,
        // `/`, or `>` so we don't match `<p:transitionRule>` etc.
        let next = out.as_bytes().get(open + open_tag_pat.len()).copied();
        if !matches!(
            next,
            Some(b' ') | Some(b'/') | Some(b'>') | Some(b'\t') | Some(b'\n') | Some(b'\r')
        ) {
            // False positive; advance past it to avoid infinite loop.
            // Replace this occurrence's `<` with a sentinel we can find again.
            // Easier: break out of the loop because remaining matches would
            // be of the same shape.
            break;
        }
        // Find the close `>` of the opening tag, respecting any quoted attrs.
        let opening_close = match find_tag_close(&out, open) {
            Some(p) => p,
            None => break,
        };
        let opening_close_end = opening_close + 1; // include `>`
        let self_closing = out.as_bytes().get(opening_close).copied() == Some(b'/');
        if self_closing {
            out.replace_range(open..opening_close_end, "");
            continue;
        }
        // Paired form — scan to matching close tag.
        let Some(close_rel) = out[opening_close_end..].find(&close_tag_pat) else {
            // Unmatched open tag — abort to avoid eating the rest of the doc.
            break;
        };
        let close_start = opening_close_end + close_rel;
        let close_end = close_start + close_tag_pat.len();
        out.replace_range(open..close_end, "");
    }
    out
}

/// Find the byte index of the `>` (or `/` for self-close) that closes the
/// opening tag starting at `tag_open`. Walks through attribute values
/// respecting single/double quotes.
fn find_tag_close(s: &str, tag_open: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = tag_open;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
        } else if in_double {
            if b == b'"' {
                in_double = false;
            }
        } else {
            match b {
                b'\'' => in_single = true,
                b'"' => in_double = true,
                b'/' => {
                    // Self-close marker; the actual close is the next char.
                    if i + 1 < bytes.len() && bytes[i + 1] == b'>' {
                        return Some(i);
                    }
                }
                b'>' => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Splice `transition_xml` into the slide just before `</p:cSld>`. The slide
/// schema requires `<p:transition>` to be the last child of `<p:sld>` but after
/// `<p:cSld>` and optional `<p:clrMapOvr>`. We place it right after
/// `</p:cSld>` (before any `</p:sld>`), which is the standard position.
fn inject_transition_xml(slide_xml: &str, transition_xml: &str) -> Result<String, HandlerError> {
    if let Some(idx) = slide_xml.find("</p:cSld>") {
        let after = idx + "</p:cSld>".len();
        let mut out = String::with_capacity(slide_xml.len() + transition_xml.len() + 2);
        out.push_str(&slide_xml[..after]);
        out.push('\n');
        out.push_str(transition_xml);
        out.push_str(&slide_xml[after..]);
        return Ok(out);
    }
    // Slides without an explicit </p:cSld> are malformed — bail loudly.
    Err(HandlerError::OperationFailed(
        "slide XML missing </p:cSld> close tag".into(),
    ))
}

// ─────────────────────────────────────────────────────────────────────────
// Animations
//
// Minimal but real: emits a `<p:timing>` block that animates a shape on the
// slide with one of four preset classes — entrance, exit, emphasis, motion.
// The C# Animations.cs is 3020 lines because it supports dozens of preset
// effect tokens (Fade, Fly-In, Wipe, …), per-effect durations, repeat/restart,
// and rich motion paths. We expose the four class shapes plus a small preset
// token table for the most common effects; power users can `raw-set` custom
// timing trees on top.

const ANIM_PRESETS: &[(&str, &str, &str)] = &[
    // (class, preset_token, preset_id)
    ("entrance", "Fade", "10"),
    ("entrance", "Fly-In", "2"),
    ("entrance", "Wipe", "12"),
    ("entrance", "Zoom", "23"),
    ("exit", "Fade", "10"),
    ("exit", "Fly-Out", "2"),
    ("exit", "Wipe", "12"),
    ("emphasis", "Spin", "8"),
    ("emphasis", "Pulse", "1"),
    ("emphasis", "Grow/Shrink", "3"),
    ("motion", "Custom Path", "1"),
    ("motion", "Arc-Up", "10"),
];

fn add_animation(
    package: &mut OxmlPackage,
    parent: &str,
    props: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_path = resolve_slide_path(package, parent)?;

    let target_shape = props
        .get("shape")
        .or_else(|| props.get("target"))
        .ok_or_else(|| {
            HandlerError::InvalidArgument(
                "animation requires --prop shape=<shape-id or name>".into(),
            )
        })?;
    let class = props
        .get("class")
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "entrance".to_string());
    if !matches!(class.as_str(), "entrance" | "exit" | "emphasis" | "motion") {
        return Err(HandlerError::InvalidArgument(format!(
            "Invalid animation class '{}'. Valid values: entrance, exit, emphasis, motion.",
            class
        )));
    }
    let preset_name = props
        .get("preset")
        .cloned()
        .unwrap_or_else(|| match class.as_str() {
            "entrance" => "Fade".to_string(),
            "exit" => "Fade".to_string(),
            "emphasis" => "Spin".to_string(),
            "motion" => "Custom Path".to_string(),
            _ => "Fade".to_string(),
        });
    let preset_id = ANIM_PRESETS
        .iter()
        .find(|(c, n, _)| *c == class.as_str() && n.eq_ignore_ascii_case(&preset_name))
        .map(|(_, _, id)| *id)
        .unwrap_or("0");
    let duration = props
        .get("duration")
        .or_else(|| props.get("dur"))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(500);
    let delay = props
        .get("delay")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Resolve the target shape's spid (_spid in C# parlance) by id or name.
    let spid = match resolve_shape_id(&slide_xml, target_shape) {
        Some(id) => id,
        None => {
            return Err(HandlerError::PathNotFound(format!(
                "shape '{}' on slide {}",
                target_shape, parent
            )))
        }
    };

    let timing_xml = build_timing_xml(&spid, &class, &preset_name, preset_id, duration, delay);

    // Replace any existing <p:timing> block on the slide.
    let cleaned = strip_named_element(&slide_xml, "p:timing");
    // Insert before </p:sld>.
    let insert_pos = cleaned
        .find("</p:sld>")
        .or_else(|| cleaned.rfind("/p:sld>"))
        .ok_or_else(|| HandlerError::OperationFailed("slide missing </p:sld>".into()))?;
    // Back up over any trailing whitespace/newline so we don't pad with blanks.
    let mut out = String::with_capacity(cleaned.len() + timing_xml.len() + 2);
    out.push_str(&cleaned[..insert_pos]);
    out.push_str(&timing_xml);
    out.push_str(&cleaned[insert_pos..]);

    package
        .write_part_xml(&slide_path, &out)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(format!(
        "Added {} animation '{}' (id={}) on {} targeting shape {}",
        class, preset_name, preset_id, parent, spid
    ))
}

/// Resolve a shape identifier from a slide XML string. Accepts either the
/// numeric `id` attribute or the `name` attribute of an `<p:cNvPr>` element.
fn resolve_shape_id(slide_xml: &str, target: &str) -> Option<String> {
    // Iterate every `<p:cNvPr id="N" name="…"/>` and return the id whose
    // name matches (case-insensitive) or whose id equals target.
    let mut cursor = 0;
    while let Some(rel) = slide_xml[cursor..].find("<p:cNvPr") {
        let open = cursor + rel;
        let close = match slide_xml[open..]
            .find("/>")
            .or_else(|| slide_xml[open..].find('>'))
        {
            Some(p) => {
                open + p
                    + (if slide_xml[open..].find("/>").map(|p| open + p) == Some(open + p) {
                        2
                    } else {
                        1
                    })
            }
            None => {
                cursor = open + 1;
                continue;
            }
        };
        let chunk = &slide_xml[open..close];
        let id_attr = extract_attr(chunk, "id");
        let name_attr = extract_attr(chunk, "name");
        if let Some(id) = &id_attr {
            if id == target {
                return id_attr.clone();
            }
        }
        if let Some(name) = &name_attr {
            if name.eq_ignore_ascii_case(target) {
                return id_attr.clone();
            }
        }
        cursor = close;
    }
    None
}

/// Extract the value of an XML attribute from a small chunk. Handles both
/// single and double quotes.
fn extract_attr(chunk: &str, attr: &str) -> Option<String> {
    let pat_dq = format!("{}=\"", attr);
    if let Some(rel) = chunk.find(&pat_dq) {
        let start = rel + pat_dq.len();
        if let Some(end) = chunk[start..].find('"') {
            return Some(chunk[start..start + end].to_string());
        }
    }
    let pat_sq = format!("{}='", attr);
    if let Some(rel) = chunk.find(&pat_sq) {
        let start = rel + pat_sq.len();
        if let Some(end) = chunk[start..].find('\'') {
            return Some(chunk[start..start + end].to_string());
        }
    }
    None
}

/// Build a `<p:timing>` element that fires `preset_name` against shape `spid`.
/// This is the minimal OOXML timing tree PowerPoint accepts.
fn build_timing_xml(
    spid: &str,
    class: &str,
    preset_name: &str,
    preset_id: &str,
    duration_ms: u32,
    delay_ms: u32,
) -> String {
    let effect_id = match class {
        "entrance" => "1",
        "exit" => "2",
        "emphasis" => "3",
        "motion" => "4",
        _ => "1",
    };
    format!(
        r#"<p:timing>
  <p:tnLst>
    <p:par>
      <p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot">
        <p:childTnLst>
          <p:seq concurrent="1" nextAc="seek">
            <p:cTn id="2" dur="{dur}" nodeType="mainSeq">
              <p:childTnLst>
                <p:par>
                  <p:cTn id="3" fill="hold">
                    <p:stCondLst><p:cond delay="{delay}"/></p:stCondLst>
                    <p:childTnLst>
                      <p:par>
                        <p:cTn id="4" fill="hold">
                          <p:stCondLst><p:cond delay="0"/></p:stCondLst>
                          <p:childTnLst>
                            <p:par>
                              <p:cTn id="5" presetID="{preset_id}" presetClass="{class_token}" presetSubtype="0" fill="hold" grpId="0" nodeType="clickEffect">
                                <p:stCondLst><p:cond delay="0"/></p:stCondLst>
                                <p:childTnLst>
                                  <p:set>
                                    <p:cBhvr>
                                      <p:cTn id="6" dur="{dur}" fill="hold"/>
                                      <p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl>
                                      <p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst>
                                    </p:cBhvr>
                                    <p:to><p:strVal val="visible"/></p:to>
                                  </p:set>
                                  <p:anim>
                                    <p:cBhvr>
                                      <p:cTn id="7" dur="{dur}"/>
                                      <p:tgtEl><p:spTgt spid="{spid}"/></p:tgtEl>
                                    </p:cBhvr>
                                  </p:anim>
                                </p:childTnLst>
                              </p:cTn>
                            </p:par>
                          </p:childTnLst>
                        </p:cTn>
                      </p:par>
                    </p:childTnLst>
                  </p:cTn>
                </p:par>
              </p:childTnLst>
            </p:cTn>
            <p:prevCondLst><p:cond evt="onPrev" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:prevCondLst>
            <p:nextCondLst><p:cond evt="onNext" delay="0"><p:tgtEl><p:sldTgt/></p:tgtEl></p:cond></p:nextCondLst>
          </p:seq>
        </p:childTnLst>
      </p:cTn>
    </p:par>
  </p:tnLst>
  <p:bldLst>
    <p:bldP spid="{spid}" effectId="{effect_id}" presetId="{preset_id}" presetClass="{class_token}" presetSubtype="0" grpId="0"/>
  </p:bldLst>
</p:timing>
<!-- preset human name: {preset_name} -->"#,
        dur = duration_ms,
        delay = delay_ms,
        preset_id = preset_id,
        class_token = class,
        spid = spid,
        effect_id = effect_id,
        preset_name = preset_name,
    )
}

#[cfg(test)]
mod transition_tests {
    use super::*;

    #[test]
    fn basic_fade_emits_bare_transition() {
        let mut props = HashMap::new();
        props.insert("type".into(), "fade".into());
        let xml = render_transition_xml("fade", "", &props).unwrap();
        assert!(xml.contains("<p:transition><p:fade/></p:transition>"));
        assert!(!xml.contains("mc:AlternateContent"));
    }

    #[test]
    fn push_with_direction_writes_dir_attr() {
        let mut props = HashMap::new();
        props.insert("direction".into(), "r".into());
        let xml = render_transition_xml("push", "", &props).unwrap();
        assert!(xml.contains("<p:push dir=\"r\"/>"));
    }

    #[test]
    fn morph_uses_alternate_content() {
        let mut props = HashMap::new();
        props.insert("option".into(), "byObject".into());
        let xml = render_transition_xml("morph", "", &props).unwrap();
        assert!(xml.contains("mc:AlternateContent"));
        assert!(xml.contains("p14:morphPr option=\"byObject\""));
    }

    #[test]
    fn p14_vortex_uses_alternate_content() {
        let xml = render_transition_xml("vortex", "", &HashMap::new()).unwrap();
        assert!(xml.contains("p14:vortex"));
        assert!(xml.contains("Requires=\"p14\""));
    }

    #[test]
    fn p15_preset_uses_prst_trans() {
        let xml = render_transition_xml("pageCurlDouble", "", &HashMap::new()).unwrap();
        assert!(xml.contains("p15:prstTrans"));
        assert!(xml.contains("prst=\"pageCurlDouble\""));
    }

    #[test]
    fn unknown_transition_errors() {
        let err = render_transition_xml("bogusTransition", "", &HashMap::new()).unwrap_err();
        assert!(matches!(err, HandlerError::InvalidArgument(_)));
    }

    #[test]
    fn duration_and_advance_time_attrs() {
        let mut props = HashMap::new();
        props.insert("duration".into(), "500".into());
        props.insert("advanceTime".into(), "3000".into());
        let attrs = build_transition_attrs(&props);
        assert!(attrs.contains("dur=\"500\""));
        assert!(attrs.contains("advTm=\"3000\""));
    }

    #[test]
    fn strip_removes_bare_and_alt_content_transitions() {
        let slide = r#"<?xml version="1.0"?>
<p:sld xmlns:p="p">
  <p:cSld><p:spTree/></p:cSld>
  <p:transition dur="500"><p:fade/></p:transition>
  <mc:AlternateContent xmlns:mc="mc">
    <mc:Choice xmlns:p14="p14" Requires="p14"><p14:transition><p14:morphPr option="byObject"/></p14:transition></mc:Choice>
    <mc:Fallback><p:transition/></mc:Fallback>
  </mc:AlternateContent>
</p:sld>"#;
        let cleaned = strip_existing_transition(slide);
        assert!(!cleaned.contains("p:transition"));
        assert!(!cleaned.contains("mc:AlternateContent"));
        assert!(cleaned.contains("<p:cSld>"));
    }

    #[test]
    fn inject_inserts_after_csld_close() {
        let slide = r#"<p:sld><p:cSld><p:spTree/></p:cSld></p:sld>"#;
        let result = inject_transition_xml(slide, "<p:transition><p:cut/></p:transition>").unwrap();
        assert!(result.contains("</p:cSld>\n<p:transition><p:cut/></p:transition></p:sld>"));
    }

    #[test]
    fn animation_timing_includes_shape_spid() {
        let xml = build_timing_xml("42", "entrance", "Fade", "10", 500, 0);
        assert!(xml.contains("spid=\"42\""));
        assert!(xml.contains("presetClass=\"entrance\""));
        assert!(xml.contains("presetID=\"10\""));
        assert!(xml.contains("<!-- preset human name: Fade -->"));
    }

    #[test]
    fn shape_resolution_by_id_and_name() {
        let xml = r#"<p:sld><p:spTree>
          <p:sp><p:nvSpPr><p:cNvPr id="7" name="Title 1"/></p:nvSpPr></p:sp>
        </p:spTree></p:sld>"#;
        assert_eq!(resolve_shape_id(xml, "7").as_deref(), Some("7"));
        assert_eq!(resolve_shape_id(xml, "Title 1").as_deref(), Some("7"));
        assert!(resolve_shape_id(xml, "missing").is_none());
    }
}

#[cfg(test)]
mod glb_tests {
    use super::*;

    #[test]
    fn minimal_glb_passes_spec_invariants() {
        let bytes = minimal_glb_v2();
        // Khronos GLB v2: 12-byte header + first chunk header (8) + at least
        // the JSON asset object.
        assert!(bytes.len() >= 12 + 8 + 22);
        // magic
        assert_eq!(&bytes[..4], b"glTF");
        // version = 2
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
        // total length matches bytes written
        let total = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, bytes.len());
        // first chunk type = "JSON"
        assert_eq!(&bytes[16..20], b"JSON");
        // first chunk length covers the asset object plus padding
        let chunk_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        assert!(chunk_len >= 22);
        assert_eq!(chunk_len % 4, 0);
        // JSON asset.version present and parseable
        let json = std::str::from_utf8(&bytes[20..20 + chunk_len]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json.trim_end_matches(' ')).unwrap();
        assert_eq!(parsed["asset"]["version"], "2.0");
    }
}
