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
        "linebreak" | "br" | "line-break" => {
            crate::linebreak::add_linebreak(package, parent, _position)
        }
        "rectangle" | "rect" => add_rectangle(package, parent, properties),
        "ellipse" | "oval" | "circle" => add_ellipse(package, parent, properties),
        "line" | "lineShape" => add_line_shape(package, parent, properties),
        "connector" => add_connector(package, parent, properties),
        "group" | "grpSp" => add_group(package, parent, properties),
        "diagram" | "flowchart" => add_diagram(package, parent, properties),
        "picture" | "image" | "img" => add_picture(package, parent, properties),
        "video" | "media" => add_video(package, parent, properties),
        "audio" => add_audio(package, parent, properties),
        "table" | "graphicFrame" => add_table(package, parent, properties),
        "chart" => add_chart_real(package, parent, properties),
        "model3d" | "3dmodel" => add_model3d_real(package, parent, properties),
        "comment" => add_comment(package, parent, properties),
        "moderncomment" | "modernComment" | "modern-comment" => {
            add_modern_comment(package, parent, properties)
        }
        "note" | "notes" => add_note(package, parent, properties),
        "hyperlink" => add_hyperlink(package, parent, properties),
        "transition" => add_transition(package, parent, properties),
        "animation" | "anim" => add_animation(package, parent, properties),
        other => Err(HandlerError::UnsupportedType(format!(
            "PPTX add '{}' not supported. Supported types: slide, shape, textbox, text, \
             rectangle, ellipse, line, connector, group, picture/image, video, audio, \
             table, chart, model3d, comment, modernComment, note, hyperlink, linebreak, transition, animation, diagram",
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

fn insert_before_close(xml: &str, tag: &str, addition: &str) -> Result<String, HandlerError> {
    let close = format!("</{tag}>");
    if let Some(offset) = xml.rfind(&close) {
        let mut result = xml.to_string();
        result.insert_str(offset, addition);
        return Ok(result);
    }
    let start = xml
        .find(&format!("<{tag}"))
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing <{tag}>")))?;
    let end = xml[start..]
        .find("/>")
        .map(|offset| start + offset)
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing closing <{tag}>")))?;
    let mut result = String::with_capacity(xml.len() + addition.len() + close.len());
    result.push_str(&xml[..end]);
    result.push('>');
    result.push_str(addition);
    result.push_str(&close);
    result.push_str(&xml[end + 2..]);
    Ok(result)
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

/// Build an editable `p:grpSp` from Mermaid's common flowchart notation.
/// This is intentionally offline: no browser, JavaScript runtime or raster
/// fallback is required for the native diagram path.
fn add_diagram(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let source = if let Some(source) = properties
        .get("mermaid")
        .or_else(|| properties.get("text"))
        .or_else(|| properties.get("dsl"))
    {
        source.clone()
    } else if let Some(path) = properties.get("src").or_else(|| properties.get("path")) {
        std::fs::read_to_string(path).map_err(|e| {
            HandlerError::InvalidArgument(format!("diagram source file '{}': {}", path, e))
        })?
    } else {
        return Err(HandlerError::InvalidArgument(
            "diagram requires mermaid/text/dsl or src/path".to_string(),
        ));
    };
    if source
        .lines()
        .flat_map(|line| line.split(';'))
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))
        .is_some_and(|line| line.eq_ignore_ascii_case("sequenceDiagram"))
    {
        return add_sequence_diagram(package, parent, &source);
    }
    let left_to_right = source.lines().flat_map(|line| line.split(';')).any(|line| {
        let words: Vec<_> = line.split_whitespace().collect();
        words.len() >= 2
            && matches!(
                words[0].to_ascii_lowercase().as_str(),
                "flowchart" | "graph"
            )
            && matches!(words[1].to_ascii_lowercase().as_str(), "lr" | "rl")
    });
    let mut nodes: Vec<(String, String, &str, &str, &str)> = Vec::new();
    let mut node_positions: HashMap<String, usize> = HashMap::new();
    let mut edges = Vec::new();
    for statement in source.lines().flat_map(|line| line.split(';')) {
        let statement = statement.trim();
        let lower = statement.to_ascii_lowercase();
        if statement.is_empty()
            || statement.starts_with("%%")
            || lower.starts_with("flowchart")
            || lower.starts_with("graph")
            || lower.starts_with("subgraph")
            || matches!(lower.as_str(), "end")
            || lower.starts_with("style")
            || lower.starts_with("class")
            || lower.starts_with("click")
        {
            continue;
        }
        let parts: Vec<_> = statement.split("-->").collect();
        if parts.len() == 1 {
            let _ = pptx_diagram_node(parts[0], &mut nodes, &mut node_positions);
            continue;
        }
        let mut previous = pptx_diagram_node(parts[0], &mut nodes, &mut node_positions);
        for part in &parts[1..] {
            let next = pptx_diagram_node(part, &mut nodes, &mut node_positions);
            if let (Some(from), Some(to)) = (&previous, &next) {
                edges.push((from.clone(), to.clone()));
            }
            previous = next;
        }
    }
    if nodes.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "diagram has no nodes; use e.g. 'flowchart TD; A[Start] --> B[End]'".to_string(),
        ));
    }
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let base_id = find_max_id(&slide_xml) + 1;
    let node_w = 1_440_000i64;
    let node_h = 576_000i64;
    let gap = 864_000i64;
    let margin = 288_000i64;
    let mut children = String::new();
    let mut positions: HashMap<String, (i64, i64)> = HashMap::new();
    for (index, (id, label, geometry, fill, line)) in nodes.iter().enumerate() {
        let (x, y) = if left_to_right {
            (margin + index as i64 * (node_w + gap), margin)
        } else {
            (margin, margin + index as i64 * (node_h + gap))
        };
        positions.insert(id.clone(), (x, y));
        children.push_str(&pptx_diagram_shape_xml(
            base_id + index + 1,
            label,
            geometry,
            fill,
            line,
            x,
            y,
            node_w,
            node_h,
        ));
    }
    let edge_base = base_id + nodes.len() + 1;
    for (index, (from, to)) in edges.iter().enumerate() {
        let (sx, sy) = positions[from];
        let (tx, ty) = positions[to];
        let (x, y, w, h) = if left_to_right {
            (sx + node_w, sy + node_h / 2, tx - sx - node_w, ty - sy)
        } else {
            (sx + node_w / 2, sy + node_h, tx - sx, ty - sy - node_h)
        };
        children.push_str(&pptx_diagram_edge_xml(
            edge_base + index,
            x,
            y,
            w.max(12_700),
            h.max(12_700),
            false,
        ));
    }
    let width = if left_to_right {
        margin * 2 + nodes.len() as i64 * node_w + nodes.len().saturating_sub(1) as i64 * gap
    } else {
        margin * 2 + node_w
    };
    let height = if left_to_right {
        margin * 2 + node_h
    } else {
        margin * 2 + nodes.len() as i64 * node_h + nodes.len().saturating_sub(1) as i64 * gap
    };
    let group_count = slide_xml.matches("<p:grpSp>").count() + 1;
    let group_xml = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="{base_id}" name="Diagram {group_count}"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{width}" cy="{height}"/><a:chOff x="0" y="0"/><a:chExt cx="{width}" cy="{height}"/></a:xfrm></p:grpSpPr>{children}</p:grpSp>"#
    );
    package
        .write_part_xml(
            &slide_path,
            &insert_shape_in_sp_tree(&slide_xml, &group_xml),
        )
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/slide[{}]/group[{}]", slide_num, group_count))
}

fn add_sequence_diagram(
    package: &mut OxmlPackage,
    parent: &str,
    source: &str,
) -> Result<String, HandlerError> {
    let mut participants: Vec<(String, String)> = Vec::new();
    let mut participant_index: HashMap<String, usize> = HashMap::new();
    let mut messages: Vec<(String, String, String, bool)> = Vec::new();
    let mut see = |id: &str, label: Option<&str>| {
        if let Some(index) = participant_index.get(id).copied() {
            if let Some(label) = label {
                participants[index].1 = label.trim().to_string();
            }
        } else {
            participant_index.insert(id.to_string(), participants.len());
            participants.push((id.to_string(), label.unwrap_or(id).trim().to_string()));
        }
    };
    for statement in source.lines().flat_map(|line| line.split(';')) {
        let statement = statement.trim();
        if statement.is_empty()
            || statement.starts_with("%%")
            || statement.eq_ignore_ascii_case("sequenceDiagram")
        {
            continue;
        }
        let lower = statement.to_ascii_lowercase();
        if lower.starts_with("participant ") || lower.starts_with("actor ") {
            let declaration = statement
                .split_once(char::is_whitespace)
                .map(|(_, rest)| rest.trim())
                .unwrap_or("");
            let (id, label) = declaration
                .split_once(" as ")
                .unwrap_or((declaration, declaration));
            if !id.is_empty() {
                see(id.trim(), Some(label));
            }
            continue;
        }
        let Some((left, label)) = statement.split_once(':') else {
            continue;
        };
        if let Some((offset, operator)) = ["-->>", "->>", "-->", "->", "--x", "-x"]
            .iter()
            .find_map(|operator| left.find(operator).map(|offset| (offset, *operator)))
        {
            let from = left[..offset].trim();
            let to = left[offset + operator.len()..]
                .trim()
                .trim_start_matches(['+', '-']);
            if !from.is_empty() && !to.is_empty() {
                see(from, None);
                see(to, None);
                messages.push((
                    from.to_string(),
                    to.to_string(),
                    label.trim().to_string(),
                    operator.starts_with("--"),
                ));
            }
        }
    }
    if participants.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "sequence diagram has no participants".to_string(),
        ));
    }
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let base_id = find_max_id(&slide_xml) + 1;
    let margin = 288_000i64;
    let box_w = 864_000i64;
    let box_h = 396_000i64;
    let gap = 504_000i64;
    let body_top = margin + box_h + 324_000;
    let row = 414_000i64;
    let bottom = body_top + messages.len().max(1) as i64 * row + 216_000;
    let mut children = String::new();
    let mut centers = HashMap::new();
    for (index, (id, label)) in participants.iter().enumerate() {
        let x = margin + index as i64 * (box_w + gap);
        centers.insert(id.clone(), x + box_w / 2);
        children.push_str(&pptx_diagram_shape_xml(
            base_id + index + 1,
            label,
            "rect",
            "DAE8FC",
            "6C8EBF",
            x,
            margin,
            box_w,
            box_h,
        ));
    }
    let mut next_id = base_id + participants.len() + 1;
    for (id, _) in &participants {
        let x = centers[id];
        children.push_str(&pptx_diagram_edge_xml(
            next_id,
            x,
            margin + box_h,
            12_700,
            bottom - margin - box_h,
            true,
        ));
        next_id += 1;
    }
    for (index, (from, to, label, dashed)) in messages.iter().enumerate() {
        let y = body_top + index as i64 * row;
        let x1 = centers[from];
        let x2 = centers[to];
        children.push_str(&pptx_diagram_edge_xml(
            next_id,
            x1,
            y,
            (x2 - x1).unsigned_abs().max(12_700) as i64,
            12_700,
            *dashed,
        ));
        next_id += 1;
        if !label.is_empty() {
            children.push_str(&pptx_diagram_shape_xml(
                next_id,
                label,
                "rect",
                "FFFFFF",
                "FFFFFF",
                (x1 + x2) / 2 - 216_000,
                y - 180_000,
                432_000,
                144_000,
            ));
            next_id += 1;
        }
    }
    let width = margin * 2
        + participants.len() as i64 * box_w
        + participants.len().saturating_sub(1) as i64 * gap;
    let height = bottom + margin;
    let group_count = slide_xml.matches("<p:grpSp>").count() + 1;
    let group_xml = format!(
        r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="{base_id}" name="Sequence diagram {group_count}"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{width}" cy="{height}"/><a:chOff x="0" y="0"/><a:chExt cx="{width}" cy="{height}"/></a:xfrm></p:grpSpPr>{children}</p:grpSp>"#
    );
    package
        .write_part_xml(
            &slide_path,
            &insert_shape_in_sp_tree(&slide_xml, &group_xml),
        )
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/slide[{}]/group[{}]", slide_num, group_count))
}

fn pptx_diagram_node(
    token: &str,
    nodes: &mut Vec<(String, String, &'static str, &'static str, &'static str)>,
    positions: &mut HashMap<String, usize>,
) -> Option<String> {
    let token = token.trim().trim_matches('|').trim();
    let id_end = token
        .find(|character: char| !character.is_alphanumeric() && character != '_')
        .unwrap_or(token.len());
    let id = token[..id_end].trim();
    if id.is_empty() {
        return None;
    }
    let rest = token[id_end..].trim();
    let (label, geometry, fill, line) = if let Some(label) = rest
        .strip_prefix("{{")
        .and_then(|value| value.strip_suffix("}}"))
    {
        (label, "hexagon", "FFF2CC", "D6B656")
    } else if let Some(label) = rest
        .strip_prefix("((")
        .and_then(|value| value.strip_suffix("))"))
    {
        (label, "ellipse", "F8CECC", "B85450")
    } else if let Some(label) = rest
        .strip_prefix("{")
        .and_then(|value| value.strip_suffix("}"))
    {
        (label, "diamond", "FFF2CC", "D6B656")
    } else if let Some(label) = rest
        .strip_prefix("(")
        .and_then(|value| value.strip_suffix(")"))
    {
        (label, "roundRect", "D5E8D4", "82B366")
    } else if let Some(label) = rest
        .strip_prefix("[")
        .and_then(|value| value.strip_suffix("]"))
    {
        (label, "rect", "DAE8FC", "6C8EBF")
    } else {
        (id, "rect", "DAE8FC", "6C8EBF")
    };
    let id = id.to_string();
    let label = label.trim().trim_matches('"').to_string();
    if let Some(index) = positions.get(&id).copied() {
        nodes[index].1 = label;
    } else {
        positions.insert(id.clone(), nodes.len());
        nodes.push((id.clone(), label, geometry, fill, line));
    }
    Some(id)
}

#[allow(clippy::too_many_arguments)]
fn pptx_diagram_shape_xml(
    id: usize,
    text: &str,
    geometry: &str,
    fill: &str,
    line: &str,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Diagram node {id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:prstGeom prst="{geometry}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="{fill}"/></a:solidFill><a:ln w="12700"><a:solidFill><a:srgbClr val="{line}"/></a:solidFill></a:ln></p:spPr><p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr sz="1800"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#,
        text = xml_escape_text(text)
    )
}

fn pptx_diagram_edge_xml(
    id: usize,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    dashed: bool,
) -> String {
    let dash = if dashed {
        "<a:prstDash val=\"dash\"/>"
    } else {
        ""
    };
    format!(
        r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="{id}" name="Diagram edge {id}"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:ln w="12700"><a:solidFill><a:srgbClr val="4D4D4D"/></a:solidFill>{dash}<a:tailEnd type="triangle"/></a:ln></p:spPr></p:cxnSp>"#
    )
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

    let src = properties
        .get("src")
        .or_else(|| properties.get("path"))
        .or_else(|| properties.get("file"));

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
        Some(std::fs::read(src_path).map_err(|error| {
            HandlerError::OperationFailed(format!(
                "failed to read image source '{src_path}': {error}"
            ))
        })?)
    } else if let Some(b64) = properties.get("payloadBase64") {
        base64_decode(b64).ok()
    } else if let Some(hex) = properties.get("payloadHex") {
        hex_decode(hex).ok()
    } else {
        Some(Vec::new())
    };
    if let Some(bytes) = bytes_to_write {
        package
            .write_part(&media_part_path, bytes)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
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
    if !(1..=1_000).contains(&rows) || !(1..=1_000).contains(&cols) {
        return Err(HandlerError::InvalidArgument(
            "PPTX table rows and cols must each be in 1..=1000".to_string(),
        ));
    }
    if rows.saturating_mul(cols) > 1_000_000 {
        return Err(HandlerError::InvalidArgument(
            "PPTX table cannot exceed 1,000,000 cells".to_string(),
        ));
    }
    let frame_width = cx
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(8_382_000);
    let frame_height = cy
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .unwrap_or(1_143_000);
    let col_width = (frame_width / cols as i64).max(1);
    let row_height = (frame_height / rows as i64).max(1);

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
    <a:ext cx="{frame_width}" cy="{frame_height}"/>
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
    let new_xml = if let Some(pos) = xml
        .find("<Types")
        .and_then(|start| xml[start..].find('>').map(|offset| start + offset))
    {
        // Right after opening <Types ...>
        let close = pos + 1;
        let mut out = String::with_capacity(xml.len() + default_xml.len());
        out.push_str(&xml[..close]);
        out.push_str(&default_xml);
        out.push_str(&xml[close..]);
        out
    } else {
        return Err(HandlerError::OperationFailed(
            "invalid [Content_Types].xml: missing Types root".to_string(),
        ));
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

/// Add a legacy slide comment.  A comment part is only meaningful when it is
/// connected to both the slide and the presentation-level author list.
fn add_comment(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let author = properties
        .get("author")
        .map(String::as_str)
        .unwrap_or("OfficeCli");
    let initials = properties
        .get("initials")
        .cloned()
        .unwrap_or_else(|| derive_comment_initials(author));
    let text = properties.get("text").map(|s| s.as_str()).unwrap_or("");
    let escaped = xml_escape_text(text);
    let date = properties
        .get("date")
        .map(String::as_str)
        .unwrap_or("2024-01-01T00:00:00Z");
    let x = properties
        .get("x")
        .map(|v| unit_to_emu(v))
        .unwrap_or_else(|| "0".to_string());
    let y = properties
        .get("y")
        .map(|v| unit_to_emu(v))
        .unwrap_or_else(|| "0".to_string());

    let (author_id, next_index) = ensure_comment_author(package, author, &initials)?;
    let (comments_path, comments_xml) = resolve_or_create_comments_part(package, &slide_path)?;
    let comment = format!(
        r#"<p:cm authorId="{author_id}" dt="{}" idx="{next_index}"><p:pos x="{x}" y="{y}"/><p:text>{escaped}</p:text></p:cm>"#,
        xml_escape_text(date),
    );

    let updated = insert_before_close(&comments_xml, "p:cmLst", &comment)?;

    package
        .write_part_xml(&comments_path, &updated)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let count = roxmltree::Document::parse(&updated)
        .ok()
        .map(|document| {
            document
                .descendants()
                .filter(|node| node.has_tag_name("cm"))
                .count()
        })
        .unwrap_or(1);
    Ok(format!("/slide[{}]/comment[{}]", slide_num, count))
}

const PPT_COMMENTS_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
const PPT_COMMENT_AUTHORS_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/commentAuthors";

fn add_modern_comment(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide)?;
    let author = properties
        .get("author")
        .map(String::as_str)
        .unwrap_or("OfficeCli");
    let initials = properties
        .get("initials")
        .cloned()
        .unwrap_or_else(|| derive_comment_initials(author));
    let text = properties.get("text").cloned().unwrap_or_default();
    let created = modern_comment_created(properties)?;
    let author_id = ensure_modern_author(package, author, &initials)?;
    let (part, xml) = ensure_modern_comment_part(package, &slide_path)?;
    let id = format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase());
    if let Some(parent) = properties.get("parent").filter(|value| !value.is_empty()) {
        let prefix = format!("/slide[{slide}]/modernComment[");
        let index = parent
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(']'))
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| {
                HandlerError::InvalidArgument(format!("invalid modern comment parent: {parent}"))
            })?;
        let document = roxmltree::Document::parse(&xml)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        let top = document
            .descendants()
            .filter(|node| node.has_tag_name("cm"))
            .nth(index - 1)
            .ok_or_else(|| HandlerError::PathNotFound(parent.clone()))?;
        let reply = format!(
            r#"<p188:reply id="{id}" authorId="{author_id}" created="{}">{}</p188:reply>"#,
            xml_escape_text(&created),
            modern_text_body_xml(&text),
        );
        let replacement =
            if let Some(list) = top.children().find(|node| node.has_tag_name("replyLst")) {
                let mut value = xml.clone();
                value.insert_str(list.range().end - "</p188:replyLst>".len(), &reply);
                value
            } else {
                let mut value = xml.clone();
                value.insert_str(
                    top.range().end - "</p188:cm>".len(),
                    &format!("<p188:replyLst>{reply}</p188:replyLst>"),
                );
                value
            };
        package
            .write_part_xml(&part, &replacement)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        let replies = top
            .children()
            .find(|node| node.has_tag_name("replyLst"))
            .map(|node| {
                node.children()
                    .filter(|child| child.has_tag_name("reply"))
                    .count()
            })
            .unwrap_or(0);
        return Ok(format!("{parent}/reply[{}]", replies + 1));
    }
    let status = if properties
        .get("resolved")
        .is_some_and(|value| value == "true" || value == "1")
    {
        " status=\"resolved\""
    } else {
        ""
    };
    let comment = format!(
        r#"<p188:cm id="{id}" authorId="{author_id}" created="{}"{status}><p188:pos x="0" y="0"/>{}</p188:cm>"#,
        xml_escape_text(&created),
        modern_text_body_xml(&text),
    );
    let updated = insert_before_close(&xml, "p188:cmLst", &comment)?;
    let index = roxmltree::Document::parse(&updated)
        .ok()
        .map(|d| d.descendants().filter(|n| n.has_tag_name("cm")).count())
        .unwrap_or(1);
    package
        .write_part_xml(&part, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("/slide[{slide}]/modernComment[{index}]"))
}

fn ensure_modern_author(
    package: &mut OxmlPackage,
    name: &str,
    initials: &str,
) -> Result<String, HandlerError> {
    let existing_path = modern_authors_part_path(package)?;
    let path = existing_path
        .clone()
        .unwrap_or_else(|| "ppt/authors.xml".into());
    ensure_content_type_override(package, &path, "application/vnd.ms-powerpoint.authors+xml")?;
    if existing_path.is_none() {
        ensure_ppt_relationship(
            package,
            "ppt/_rels/presentation.xml.rels",
            PPT_MODERN_AUTHORS_REL_TYPE,
            "authors.xml",
        )?;
    }
    let xml = package.read_part_xml(&path).unwrap_or_else(|_| {
        "<p188:authorLst xmlns:p188=\"http://schemas.microsoft.com/office/powerpoint/2018/8/main\"/>"
            .into()
    });
    let doc = roxmltree::Document::parse(&xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    if let Some(node) = doc.descendants().find(|n| {
        n.has_tag_name("author")
            && n.attribute("name") == Some(name)
            && n.attribute("initials") == Some(initials)
    }) {
        return Ok(node.attribute("id").unwrap_or_default().to_string());
    }
    let id = format!("{{{}}}", uuid::Uuid::new_v4().to_string().to_uppercase());
    let item = format!(
        r#"<p188:author id="{id}" name="{}" initials="{}"/>"#,
        xml_escape_text(name),
        xml_escape_text(initials)
    );
    let updated = insert_before_close(&xml, "p188:authorLst", &item)?;
    package
        .write_part_xml(&path, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(id)
}

const PPT_MODERN_AUTHORS_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2018/10/relationships/authors";

fn modern_authors_part_path(package: &OxmlPackage) -> Result<Option<String>, HandlerError> {
    let rels = package
        .part_rels("ppt/presentation.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut rels: Vec<_> = rels
        .all()
        .values()
        .filter(|rel| rel.type_uri == PPT_MODERN_AUTHORS_REL_TYPE)
        .collect();
    rels.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(rels
        .first()
        .map(|rel| package.resolve_rel_target("ppt/presentation.xml", &rel.target)))
}

fn ensure_modern_comment_part(
    package: &mut OxmlPackage,
    slide: &str,
) -> Result<(String, String), HandlerError> {
    const REL: &str = "http://schemas.microsoft.com/office/2018/10/relationships/comment";
    if let Ok(rels) = package.part_rels(slide) {
        if let Some(rel) = rels.all().values().find(|rel| rel.type_uri == REL) {
            let path = package.resolve_rel_target(slide, &rel.target);
            return Ok((
                path.clone(),
                package
                    .read_part_xml(&path)
                    .map_err(|e| HandlerError::OperationFailed(e.to_string()))?,
            ));
        }
    }
    let path = next_modern_comment_part_path(package);
    ensure_content_type_override(package, &path, "application/vnd.ms-powerpoint.comments+xml")?;
    ensure_ppt_relationship(
        package,
        &crate::navigation::relationships_part_path(slide),
        REL,
        &format!(
            "../comments/{}",
            path.rsplit('/').next().unwrap_or("modernComment1.xml")
        ),
    )?;
    Ok((path, "<p188:cmLst xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:p188=\"http://schemas.microsoft.com/office/powerpoint/2018/8/main\"/>".into()))
}

fn next_modern_comment_part_path(package: &OxmlPackage) -> String {
    for index in 1.. {
        let path = format!("ppt/comments/modernComment{index}.xml");
        if !package.has_part(&path) {
            return path;
        }
    }
    unreachable!("unbounded modern comment part index")
}

fn modern_comment_created(properties: &HashMap<String, String>) -> Result<String, HandlerError> {
    use time::format_description::well_known::Rfc3339;
    let created = match properties.get("created") {
        Some(value) => time::OffsetDateTime::parse(value, &Rfc3339).map_err(|_| {
            HandlerError::InvalidArgument(format!("invalid created '{value}' (expected ISO 8601)"))
        })?,
        None => time::OffsetDateTime::now_utc(),
    };
    created
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))
}

fn modern_text_body_xml(text: &str) -> String {
    if text.is_empty() {
        return "<p188:txBody><a:bodyPr/><a:p><a:endParaRPr lang=\"en-US\"/></a:p></p188:txBody>"
            .to_string();
    }
    format!(
        "<p188:txBody><a:bodyPr/><a:p><a:r><a:rPr lang=\"en-US\"/><a:t>{}</a:t></a:r></a:p></p188:txBody>",
        xml_escape_text(text)
    )
}

const PPT_MODERN_COMMENT_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2018/10/relationships/comment";

#[derive(Debug, Clone, Copy)]
enum ModernCommentPath {
    Top {
        slide: usize,
        comment: usize,
    },
    Reply {
        slide: usize,
        comment: usize,
        reply: usize,
    },
}

#[derive(Debug, Clone)]
struct ModernCommentRecord {
    part_path: String,
    xml: String,
    range: std::ops::Range<usize>,
    parent_reply_list_range: Option<std::ops::Range<usize>>,
    reply_count: usize,
    author_id: String,
    created: String,
    resolved: bool,
}

fn parse_modern_comment_path(path: &str) -> Result<ModernCommentPath, HandlerError> {
    let segments = crate::navigation::parse_path(path);
    let valid_index = |index: Option<usize>| index.filter(|index| *index > 0);
    match segments.as_slice() {
        [slide, comment]
            if slide.name.eq_ignore_ascii_case("slide")
                && comment.name.eq_ignore_ascii_case("moderncomment") =>
        {
            match (valid_index(slide.index), valid_index(comment.index)) {
                (Some(slide), Some(comment)) => Ok(ModernCommentPath::Top { slide, comment }),
                _ => Err(HandlerError::InvalidPath(format!(
                    "modern comment paths are 1-based: {path}"
                ))),
            }
        }
        [slide, comment, reply]
            if slide.name.eq_ignore_ascii_case("slide")
                && comment.name.eq_ignore_ascii_case("moderncomment")
                && reply.name.eq_ignore_ascii_case("reply") =>
        {
            match (
                valid_index(slide.index),
                valid_index(comment.index),
                valid_index(reply.index),
            ) {
                (Some(slide), Some(comment), Some(reply)) => Ok(ModernCommentPath::Reply {
                    slide,
                    comment,
                    reply,
                }),
                _ => Err(HandlerError::InvalidPath(format!(
                    "modern comment paths are 1-based: {path}"
                ))),
            }
        }
        _ => Err(HandlerError::InvalidPath(format!(
            "expected /slide[N]/modernComment[M][/reply[R]], got: {path}"
        ))),
    }
}

fn modern_comment_parts_for_slide(
    package: &OxmlPackage,
    slide_index: usize,
) -> Result<Vec<(String, String)>, HandlerError> {
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_index)?;
    let rels_path = crate::navigation::relationships_part_path(&slide_path);
    let xml = match package.read_part_xml(&rels_path) {
        Ok(xml) => xml,
        Err(_) => return Ok(Vec::new()),
    };
    let document = roxmltree::Document::parse(&xml)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    document
        .descendants()
        .filter(|node| {
            node.has_tag_name("Relationship")
                && node.attribute("Type") == Some(PPT_MODERN_COMMENT_REL_TYPE)
        })
        .filter_map(|node| node.attribute("Target"))
        .map(|target| {
            let path = package.resolve_rel_target(&slide_path, target);
            let xml = package
                .read_part_xml(&path)
                .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
            Ok((path, xml))
        })
        .collect()
}

fn read_modern_text_body(node: roxmltree::Node<'_, '_>) -> String {
    let Some(body) = node.children().find(|child| child.has_tag_name("txBody")) else {
        return String::new();
    };
    body.children()
        .filter(|child| child.has_tag_name("p"))
        .map(|paragraph| {
            paragraph
                .descendants()
                .filter(|child| child.has_tag_name("t"))
                .filter_map(|child| child.text())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn modern_authors(
    package: &OxmlPackage,
) -> Result<std::collections::HashMap<String, (String, String)>, HandlerError> {
    let Some(path) = modern_authors_part_path(package)? else {
        return Ok(std::collections::HashMap::new());
    };
    let xml = match package.read_part_xml(&path) {
        Ok(xml) => xml,
        Err(_) => return Ok(std::collections::HashMap::new()),
    };
    let document = roxmltree::Document::parse(&xml)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    Ok(document
        .descendants()
        .filter(|node| node.has_tag_name("author"))
        .filter_map(|node| {
            Some((
                node.attribute("id")?.to_string(),
                (
                    node.attribute("name").unwrap_or_default().to_string(),
                    node.attribute("initials").unwrap_or_default().to_string(),
                ),
            ))
        })
        .collect())
}

fn modern_comment_node(
    slide_index: usize,
    comment_index: usize,
    comment: roxmltree::Node<'_, '_>,
    authors: &std::collections::HashMap<String, (String, String)>,
) -> handler_common::DocumentNode {
    let text = read_modern_text_body(comment);
    let mut node = handler_common::DocumentNode::new(
        &format!("/slide[{slide_index}]/modernComment[{comment_index}]"),
        "modernComment",
    )
    .with_text(&text)
    .with_format("text", serde_json::Value::String(text));
    let author_id = comment.attribute("authorId").unwrap_or_default();
    if let Some((author, initials)) = authors.get(author_id) {
        node.format.insert(
            "author".to_string(),
            Some(serde_json::Value::String(author.clone())),
        );
        node.format.insert(
            "initials".to_string(),
            Some(serde_json::Value::String(initials.clone())),
        );
    }
    if let Some(created) = comment.attribute("created") {
        node.format.insert(
            "created".to_string(),
            Some(serde_json::Value::String(created.to_string())),
        );
    }
    node.format.insert(
        "resolved".to_string(),
        Some(serde_json::Value::Bool(
            comment.attribute("status") == Some("resolved"),
        )),
    );
    node.format.insert("parent".to_string(), None);
    if let Some(id) = comment.attribute("id") {
        node.format.insert(
            "id".to_string(),
            Some(serde_json::Value::String(id.to_string())),
        );
    }
    let mut children = Vec::new();
    if let Some(replies) = comment
        .children()
        .find(|child| child.has_tag_name("replyLst"))
    {
        for (offset, reply) in replies
            .children()
            .filter(|child| child.has_tag_name("reply"))
            .enumerate()
        {
            let reply_index = offset + 1;
            let reply_text = read_modern_text_body(reply);
            let mut reply_node = handler_common::DocumentNode::new(
                &format!(
                    "/slide[{slide_index}]/modernComment[{comment_index}]/reply[{reply_index}]"
                ),
                "modernComment",
            )
            .with_text(&reply_text)
            .with_format("text", serde_json::Value::String(reply_text));
            if let Some((author, initials)) =
                authors.get(reply.attribute("authorId").unwrap_or_default())
            {
                reply_node.format.insert(
                    "author".to_string(),
                    Some(serde_json::Value::String(author.clone())),
                );
                reply_node.format.insert(
                    "initials".to_string(),
                    Some(serde_json::Value::String(initials.clone())),
                );
            }
            if let Some(created) = reply.attribute("created") {
                reply_node.format.insert(
                    "created".to_string(),
                    Some(serde_json::Value::String(created.to_string())),
                );
            }
            reply_node
                .format
                .insert("resolved".to_string(), Some(serde_json::Value::Bool(false)));
            reply_node.format.insert(
                "parent".to_string(),
                Some(serde_json::Value::String(format!(
                    "/slide[{slide_index}]/modernComment[{comment_index}]"
                ))),
            );
            if let Some(id) = reply.attribute("id") {
                reply_node.format.insert(
                    "id".to_string(),
                    Some(serde_json::Value::String(id.to_string())),
                );
            }
            children.push(reply_node);
        }
    }
    node.child_count = children.len();
    node.children = children;
    node
}

pub(crate) fn list_modern_comment_nodes(
    package: &OxmlPackage,
    slide_filter: Option<usize>,
) -> Result<Vec<handler_common::DocumentNode>, HandlerError> {
    let presentation = crate::navigation::build_presentation(package)?;
    let authors = modern_authors(package)?;
    let mut nodes = Vec::new();
    for slide in presentation.slides {
        if slide_filter.is_some_and(|filter| filter != slide.index) {
            continue;
        }
        let mut index = 0;
        for (_, xml) in modern_comment_parts_for_slide(package, slide.index)? {
            let document = roxmltree::Document::parse(&xml)
                .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
            for comment in document
                .descendants()
                .filter(|node| node.has_tag_name("cm"))
            {
                index += 1;
                nodes.push(modern_comment_node(slide.index, index, comment, &authors));
            }
        }
    }
    Ok(nodes)
}

pub(crate) fn get_modern_comment_node(
    package: &OxmlPackage,
    path: &str,
) -> Result<handler_common::DocumentNode, HandlerError> {
    let target = parse_modern_comment_path(path)?;
    let (slide, comment, reply) = match target {
        ModernCommentPath::Top { slide, comment } => (slide, comment, None),
        ModernCommentPath::Reply {
            slide,
            comment,
            reply,
        } => (slide, comment, Some(reply)),
    };
    let node = list_modern_comment_nodes(package, Some(slide)).and_then(|nodes| {
        nodes
            .into_iter()
            .nth(comment - 1)
            .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))
    })?;
    match reply {
        None => Ok(node),
        Some(reply) => node
            .children
            .into_iter()
            .nth(reply - 1)
            .ok_or_else(|| HandlerError::PathNotFound(path.to_string())),
    }
}

fn modern_comment_record(
    package: &OxmlPackage,
    target: ModernCommentPath,
) -> Result<ModernCommentRecord, HandlerError> {
    let (slide, target_comment, target_reply) = match target {
        ModernCommentPath::Top { slide, comment } => (slide, comment, None),
        ModernCommentPath::Reply {
            slide,
            comment,
            reply,
        } => (slide, comment, Some(reply)),
    };
    let mut comment_index = 0;
    for (part_path, xml) in modern_comment_parts_for_slide(package, slide)? {
        let document = roxmltree::Document::parse(&xml)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        for comment in document
            .descendants()
            .filter(|node| node.has_tag_name("cm"))
        {
            comment_index += 1;
            if comment_index != target_comment {
                continue;
            }
            let make_record = |node: roxmltree::Node<'_, '_>,
                               parent_reply_list_range: Option<std::ops::Range<usize>>,
                               reply_count: usize,
                               resolved: bool| {
                ModernCommentRecord {
                    part_path: part_path.clone(),
                    xml: xml.clone(),
                    range: node.range(),
                    parent_reply_list_range,
                    reply_count,
                    author_id: node.attribute("authorId").unwrap_or_default().to_string(),
                    created: node.attribute("created").unwrap_or_default().to_string(),
                    resolved,
                }
            };
            if let Some(reply_index) = target_reply {
                let reply_list = comment
                    .children()
                    .find(|node| node.has_tag_name("replyLst"))
                    .ok_or_else(|| {
                        HandlerError::PathNotFound(format!(
                            "/slide[{slide}]/modernComment[{target_comment}]/reply[{reply_index}]"
                        ))
                    })?;
                let replies: Vec<_> = reply_list
                    .children()
                    .filter(|node| node.has_tag_name("reply"))
                    .collect();
                let reply = replies.get(reply_index - 1).copied().ok_or_else(|| {
                    HandlerError::PathNotFound(format!(
                        "/slide[{slide}]/modernComment[{target_comment}]/reply[{reply_index}]"
                    ))
                })?;
                return Ok(make_record(
                    reply,
                    Some(reply_list.range()),
                    replies.len(),
                    false,
                ));
            }
            return Ok(make_record(
                comment,
                None,
                0,
                comment.attribute("status") == Some("resolved"),
            ));
        }
    }
    Err(HandlerError::PathNotFound(match target {
        ModernCommentPath::Top { slide, comment } => {
            format!("/slide[{slide}]/modernComment[{comment}]")
        }
        ModernCommentPath::Reply {
            slide,
            comment,
            reply,
        } => format!("/slide[{slide}]/modernComment[{comment}]/reply[{reply}]"),
    }))
}

fn set_xml_attribute(opening: &mut String, name: &str, value: Option<&str>) {
    let needle = format!("{name}=");
    if let Some(start) = opening.find(&needle) {
        let quote_start = start + needle.len();
        let quote = opening
            .as_bytes()
            .get(quote_start)
            .copied()
            .unwrap_or(b'\"');
        let value_start = quote_start + 1;
        if let Some(end) = opening[value_start..]
            .find(quote as char)
            .map(|offset| value_start + offset + 1)
        {
            match value {
                Some(value) => opening.replace_range(value_start..end - 1, &xml_escape_text(value)),
                None => {
                    let remove_start = opening[..start].rfind(char::is_whitespace).unwrap_or(start);
                    opening.replace_range(remove_start..end, "");
                }
            }
        }
    } else if let Some(value) = value {
        let end = opening
            .strip_suffix(">")
            .map(|value| value.len())
            .unwrap_or(opening.len());
        let insert_at = opening[..end]
            .strip_suffix('/')
            .map(|value| value.len())
            .unwrap_or(end);
        opening.insert_str(
            insert_at,
            &format!(" {name}=\"{}\"", xml_escape_text(value)),
        );
    }
}

fn update_modern_element_attributes(
    xml: &mut String,
    start: usize,
    attributes: &[(&str, Option<&str>)],
) -> Result<(), HandlerError> {
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| {
            HandlerError::OperationFailed("unterminated modern comment element".to_string())
        })?;
    let mut opening = xml[start..end].to_string();
    for (name, value) in attributes {
        set_xml_attribute(&mut opening, name, *value);
    }
    xml.replace_range(start..end, &opening);
    Ok(())
}

fn replace_modern_text_body(
    xml: &mut String,
    element_start: usize,
    text: &str,
) -> Result<(), HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let element = document
        .descendants()
        .find(|node| node.range().start == element_start)
        .ok_or_else(|| {
            HandlerError::OperationFailed("modern comment element disappeared".to_string())
        })?;
    let replacement = modern_text_body_xml(text);
    if let Some(body) = element.children().find(|node| node.has_tag_name("txBody")) {
        xml.replace_range(body.range(), &replacement);
        return Ok(());
    }
    let owned = xml[element.range()].to_string();
    let closing = owned.rfind("</").ok_or_else(|| {
        HandlerError::OperationFailed("modern comment element missing closing tag".to_string())
    })?;
    xml.insert_str(element.range().start + closing, &replacement);
    Ok(())
}

fn modern_author_reference_count(package: &OxmlPackage, author_id: &str) -> usize {
    let paths: Vec<String> = package
        .list_parts()
        .into_iter()
        .filter(|path| path.starts_with("ppt/comments/"))
        .cloned()
        .collect();
    paths
        .into_iter()
        .map(|path| {
            let Ok(xml) = package.read_part_xml(&path) else {
                return 0;
            };
            let Ok(document) = roxmltree::Document::parse(&xml) else {
                return 0;
            };
            document
                .descendants()
                .filter(|node| {
                    (node.has_tag_name("cm") || node.has_tag_name("reply"))
                        && node.attribute("authorId") == Some(author_id)
                })
                .count()
        })
        .sum()
}

fn update_modern_author(
    package: &mut OxmlPackage,
    author_id: &str,
    author: &str,
    initials: &str,
) -> Result<bool, HandlerError> {
    let Some(path) = modern_authors_part_path(package)? else {
        return Ok(false);
    };
    let xml = match package.read_part_xml(&path) {
        Ok(xml) => xml,
        Err(_) => return Ok(false),
    };
    let range = {
        let document = roxmltree::Document::parse(&xml)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let Some(existing) = document
            .descendants()
            .find(|node| node.has_tag_name("author") && node.attribute("id") == Some(author_id))
        else {
            return Ok(false);
        };
        existing.range()
    };
    let replacement = format!(
        r#"<p188:author id="{}" name="{}" initials="{}"/>"#,
        xml_escape_text(author_id),
        xml_escape_text(author),
        xml_escape_text(initials),
    );
    let mut updated = xml;
    updated.replace_range(range, &replacement);
    package
        .write_part_xml(&path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(true)
}

pub(crate) fn set_modern_comment(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let target = parse_modern_comment_path(path)?;
    let record = modern_comment_record(package, target)?;
    let authors = modern_authors(package)?;
    let (old_author, old_initials) = authors
        .get(&record.author_id)
        .cloned()
        .unwrap_or_else(|| ("OfficeCli".to_string(), "OC".to_string()));
    let author = properties
        .get("author")
        .cloned()
        .unwrap_or_else(|| old_author.clone());
    let initials = properties.get("initials").cloned().unwrap_or_else(|| {
        if properties.contains_key("author") {
            derive_comment_initials(&author)
        } else {
            old_initials.clone()
        }
    });
    let mut author_id = record.author_id.clone();
    if author != old_author || initials != old_initials {
        if !author_id.is_empty()
            && modern_author_reference_count(package, &author_id) <= 1
            && update_modern_author(package, &author_id, &author, &initials)?
        {
            // C# keeps an exclusive author id stable when changing its display fields.
        } else {
            author_id = ensure_modern_author(package, &author, &initials)?;
        }
    }
    let created = match properties.get("created") {
        Some(_) => modern_comment_created(properties)?,
        None => record.created.clone(),
    };
    let resolved = match target {
        ModernCommentPath::Top { .. } => Some(
            properties
                .get("resolved")
                .map(|value| value == "true" || value == "1")
                .unwrap_or(record.resolved),
        ),
        ModernCommentPath::Reply { .. } => None,
    };
    let mut updated = record.xml;
    let status = resolved.and_then(|value| value.then_some("resolved"));
    update_modern_element_attributes(
        &mut updated,
        record.range.start,
        &[
            ("authorId", Some(author_id.as_str())),
            ("created", Some(created.as_str())),
            ("status", status),
        ],
    )?;
    if let Some(text) = properties.get("text") {
        replace_modern_text_body(&mut updated, record.range.start, text)?;
    }
    package
        .write_part_xml(&record.part_path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(properties
        .keys()
        .filter(|key| match key.as_str() {
            "text" | "author" | "initials" | "created" => false,
            "resolved" => matches!(target, ModernCommentPath::Reply { .. }),
            _ => true,
        })
        .cloned()
        .collect())
}

fn remove_modern_comment_relationship(
    package: &mut OxmlPackage,
    slide_path: &str,
    part_path: &str,
) -> Result<(), HandlerError> {
    let rels_path = crate::navigation::relationships_part_path(slide_path);
    let xml = package
        .read_part_xml(&rels_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let range = {
        let document = roxmltree::Document::parse(&xml)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let Some(rel) = document.descendants().find(|rel| {
            rel.has_tag_name("Relationship")
                && rel.attribute("Type") == Some(PPT_MODERN_COMMENT_REL_TYPE)
                && package
                    .resolve_rel_target(slide_path, rel.attribute("Target").unwrap_or_default())
                    == part_path
        }) else {
            return Ok(());
        };
        rel.range()
    };
    let mut updated = xml;
    updated.replace_range(range, "");
    package
        .write_part_xml(&rels_path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))
}

fn remove_content_type_override(
    package: &mut OxmlPackage,
    part_path: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let part_name = format!("/{}", part_path.trim_start_matches('/'));
    let range = {
        let document = roxmltree::Document::parse(&xml)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let Some(override_node) = document.descendants().find(|node| {
            node.has_tag_name("Override") && node.attribute("PartName") == Some(part_name.as_str())
        }) else {
            return Ok(());
        };
        override_node.range()
    };
    let mut updated = xml;
    updated.replace_range(range, "");
    package
        .write_part_xml("[Content_Types].xml", &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))
}

pub(crate) fn remove_modern_comment(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<Option<String>, HandlerError> {
    let target = parse_modern_comment_path(path)?;
    let record = modern_comment_record(package, target)?;
    let mut updated = record.xml.clone();
    if matches!(target, ModernCommentPath::Reply { .. }) && record.reply_count == 1 {
        updated.replace_range(
            record
                .parent_reply_list_range
                .expect("reply record has a parent list"),
            "",
        );
    } else {
        updated.replace_range(record.range, "");
    }
    let remaining = roxmltree::Document::parse(&updated)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?
        .descendants()
        .any(|node| node.has_tag_name("cm"));
    if remaining {
        package
            .write_part_xml(&record.part_path, &updated)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    } else {
        let slide = match target {
            ModernCommentPath::Top { slide, .. } | ModernCommentPath::Reply { slide, .. } => slide,
        };
        let slide_path = crate::navigation::resolve_slide_part_path(package, slide)?;
        remove_modern_comment_relationship(package, &slide_path, &record.part_path)?;
        package
            .remove_part(&record.part_path)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
        remove_content_type_override(package, &record.part_path)?;
    }
    Ok(Some(format!("removed modern comment at {path}")))
}

fn ensure_comment_author(
    package: &mut OxmlPackage,
    author: &str,
    initials: &str,
) -> Result<(usize, usize), HandlerError> {
    const AUTHORS_PATH: &str = "ppt/commentAuthors.xml";
    ensure_content_type_override(
        package,
        AUTHORS_PATH,
        "application/vnd.openxmlformats-officedocument.presentationml.commentAuthors+xml",
    )?;
    let presentation_rels = "ppt/_rels/presentation.xml.rels";
    ensure_ppt_relationship(
        package,
        presentation_rels,
        PPT_COMMENT_AUTHORS_REL_TYPE,
        "commentAuthors.xml",
    )?;
    let xml = package.read_part_xml(AUTHORS_PATH).unwrap_or_else(|_| {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:cmAuthorLst xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"/>".to_string()
    });
    let document = roxmltree::Document::parse(&xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid comment authors part: {error}"))
    })?;
    let authors: Vec<_> = document
        .descendants()
        .filter(|node| node.has_tag_name("cmAuthor"))
        .collect();
    if let Some(existing) = authors.iter().find(|node| {
        node.attribute("name") == Some(author) && node.attribute("initials") == Some(initials)
    }) {
        let id = existing
            .attribute("id")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let last_index = existing
            .attribute("lastIdx")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
            + 1;
        let range = existing.range();
        let original = &xml[range.clone()];
        let replacement = if let Some(old) = existing.attribute("lastIdx") {
            original.replacen(
                &format!("lastIdx=\"{old}\""),
                &format!("lastIdx=\"{last_index}\""),
                1,
            )
        } else {
            original.replacen("/>", &format!(" lastIdx=\"{last_index}\"/>"), 1)
        };
        let mut updated = xml;
        updated.replace_range(range, &replacement);
        package
            .write_part_xml(AUTHORS_PATH, &updated)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        return Ok((id, last_index));
    }
    let id = authors
        .iter()
        .filter_map(|node| {
            node.attribute("id")
                .and_then(|value| value.parse::<usize>().ok())
        })
        .max()
        .map_or(0, |max| max + 1);
    let new_author = format!(
        r#"<p:cmAuthor id="{id}" name="{}" initials="{}" lastIdx="1" clrIdx="0"/>"#,
        xml_escape_text(author),
        xml_escape_text(initials)
    );
    let updated = insert_before_close(&xml, "p:cmAuthorLst", &new_author)?;
    package
        .write_part_xml(AUTHORS_PATH, &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok((id, 1))
}

fn resolve_or_create_comments_part(
    package: &mut OxmlPackage,
    slide_path: &str,
) -> Result<(String, String), HandlerError> {
    let slide_rels = crate::navigation::relationships_part_path(slide_path);
    if let Ok(rels) = package.part_rels(slide_path) {
        if let Some(rel) = rels
            .all()
            .values()
            .find(|rel| rel.type_uri == PPT_COMMENTS_REL_TYPE)
        {
            let path = package.resolve_rel_target(slide_path, &rel.target);
            let xml = package
                .read_part_xml(&path)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
            return Ok((path, xml));
        }
    }
    let mut index = 1;
    let comments_path = loop {
        let path = format!("ppt/comments/comment{index}.xml");
        if !package.has_part(&path) {
            break path;
        }
        index += 1;
    };
    ensure_content_type_override(
        package,
        &comments_path,
        "application/vnd.openxmlformats-officedocument.presentationml.comments+xml",
    )?;
    let target = format!(
        "../comments/{}",
        comments_path.rsplit('/').next().unwrap_or_default()
    );
    ensure_ppt_relationship(package, &slide_rels, PPT_COMMENTS_REL_TYPE, &target)?;
    Ok((comments_path, "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:cmLst xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"/>".to_string()))
}

fn ensure_ppt_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    relationship_type: &str,
    target: &str,
) -> Result<(), HandlerError> {
    let xml = package.read_part_xml(rels_path).unwrap_or_else(|_| {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>".to_string()
    });
    if xml.contains(&format!("Type=\"{relationship_type}\"")) {
        return Ok(());
    }
    let id = next_ppt_rel_id(package, rels_path);
    let relationship =
        format!(r#"<Relationship Id="{id}" Type="{relationship_type}" Target="{target}"/>"#);
    inject_ppt_relationship(package, rels_path, &relationship)
}

fn ensure_content_type_override(
    package: &mut OxmlPackage,
    part_path: &str,
    content_type: &str,
) -> Result<(), HandlerError> {
    let xml = package
        .read_part_xml("[Content_Types].xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let part_name = format!("/{}", part_path.trim_start_matches('/'));
    if xml.contains(&format!("PartName=\"{part_name}\"")) {
        return Ok(());
    }
    let close = xml.find("</Types>").ok_or_else(|| {
        HandlerError::OperationFailed("invalid [Content_Types].xml: missing </Types>".to_string())
    })?;
    let mut updated = xml;
    updated.insert_str(
        close,
        &format!(r#"<Override PartName="{part_name}" ContentType="{content_type}"/>"#),
    );
    package
        .write_part_xml("[Content_Types].xml", &updated)
        .map_err(|e| HandlerError::SaveError(e.to_string()))
}

fn derive_comment_initials(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().find(|ch| ch.is_alphanumeric()))
        .take(3)
        .flat_map(char::to_uppercase)
        .collect();
    if initials.is_empty() {
        "?".to_string()
    } else {
        initials
    }
}

/// Resolve a legacy slide comment into the common document-node representation.
pub(crate) fn get_comment_node(
    package: &OxmlPackage,
    path: &str,
) -> Result<handler_common::DocumentNode, HandlerError> {
    let (slide_index, comment_index) = parse_comment_path(path)?;
    let comments = list_slide_comments(package, slide_index)?;
    comments
        .into_iter()
        .nth(comment_index - 1)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))
}

pub(crate) fn list_comment_nodes(
    package: &OxmlPackage,
    slide_filter: Option<usize>,
) -> Result<Vec<handler_common::DocumentNode>, HandlerError> {
    let presentation = crate::navigation::build_presentation(package)?;
    let mut nodes = Vec::new();
    for slide in presentation.slides {
        if slide_filter.is_some_and(|index| index != slide.index) {
            continue;
        }
        nodes.extend(list_slide_comments(package, slide.index)?);
    }
    Ok(nodes)
}

fn list_slide_comments(
    package: &OxmlPackage,
    slide_index: usize,
) -> Result<Vec<handler_common::DocumentNode>, HandlerError> {
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_index)?;
    let rels = package
        .part_rels(&slide_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let Some(rel) = rels
        .all()
        .values()
        .find(|rel| rel.type_uri == PPT_COMMENTS_REL_TYPE)
    else {
        return Ok(Vec::new());
    };
    let comments_path = package.resolve_rel_target(&slide_path, &rel.target);
    let comments_xml = package
        .read_part_xml(&comments_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let document = roxmltree::Document::parse(&comments_xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid slide comments part: {error}"))
    })?;
    let authors = comment_authors(package)?;
    Ok(document
        .descendants()
        .filter(|node| node.has_tag_name("cm"))
        .enumerate()
        .map(|(offset, comment)| {
            let index = offset + 1;
            let text = comment
                .children()
                .find(|node| node.has_tag_name("text"))
                .and_then(|node| node.text())
                .unwrap_or_default()
                .to_string();
            let mut node = handler_common::DocumentNode::new(
                &format!("/slide[{slide_index}]/comment[{index}]"),
                "comment",
            )
            .with_text(&text)
            .with_format("text", serde_json::Value::String(text));
            if let Some(author_id) = comment.attribute("authorId") {
                if let Some((name, initials)) = authors.get(author_id) {
                    node.format.insert(
                        "author".to_string(),
                        Some(serde_json::Value::String(name.clone())),
                    );
                    node.format.insert(
                        "initials".to_string(),
                        Some(serde_json::Value::String(initials.clone())),
                    );
                }
            }
            for attribute in ["idx", "dt"] {
                if let Some(value) = comment.attribute(attribute) {
                    let key = if attribute == "idx" { "index" } else { "date" };
                    let value = value
                        .parse::<u64>()
                        .map(serde_json::Value::from)
                        .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
                    node.format.insert(key.to_string(), Some(value));
                }
            }
            if let Some(position) = comment.children().find(|node| node.has_tag_name("pos")) {
                for axis in ["x", "y"] {
                    if let Some(value) = position.attribute(axis) {
                        node.format.insert(
                            axis.to_string(),
                            Some(serde_json::Value::String(format_emu(value))),
                        );
                    }
                }
            }
            node
        })
        .collect())
}

fn comment_authors(
    package: &OxmlPackage,
) -> Result<std::collections::HashMap<String, (String, String)>, HandlerError> {
    let xml = match package.read_part_xml("ppt/commentAuthors.xml") {
        Ok(xml) => xml,
        Err(_) => return Ok(std::collections::HashMap::new()),
    };
    let document = roxmltree::Document::parse(&xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid comment authors part: {error}"))
    })?;
    Ok(document
        .descendants()
        .filter(|node| node.has_tag_name("cmAuthor"))
        .filter_map(|node| {
            Some((
                node.attribute("id")?.to_string(),
                (
                    node.attribute("name").unwrap_or_default().to_string(),
                    node.attribute("initials").unwrap_or_default().to_string(),
                ),
            ))
        })
        .collect())
}

fn parse_comment_path(path: &str) -> Result<(usize, usize), HandlerError> {
    let segments: Vec<_> = crate::navigation::parse_path(path);
    if segments.len() != 2 || segments[0].name != "slide" || segments[1].name != "comment" {
        return Err(HandlerError::InvalidPath(format!(
            "expected /slide[N]/comment[M], got: {path}"
        )));
    }
    let slide = segments[0]
        .index
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let comment = segments[1]
        .index
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    if slide == 0 || comment == 0 {
        return Err(HandlerError::InvalidPath(
            "comment paths are 1-based".to_string(),
        ));
    }
    Ok((slide, comment))
}

fn format_emu(value: &str) -> String {
    value
        .parse::<f64>()
        .map(|emu| format!("{}cm", emu / 360_000.0))
        .unwrap_or_else(|_| value.to_string())
}

pub(crate) fn set_comment(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let (slide_index, comment_index) = parse_comment_path(path)?;
    let (comments_path, xml, range, author_id, index, old_date, old_x, old_y, old_text) =
        comment_record(package, slide_index, comment_index)?;
    let authors = comment_authors(package)?;
    let (old_author, old_initials) = authors
        .get(&author_id.to_string())
        .cloned()
        .unwrap_or_else(|| ("OfficeCli".to_string(), "OC".to_string()));
    let author = properties
        .get("author")
        .cloned()
        .unwrap_or_else(|| old_author.clone());
    let initials = properties.get("initials").cloned().unwrap_or_else(|| {
        if properties.contains_key("author") {
            derive_comment_initials(&author)
        } else {
            old_initials.clone()
        }
    });
    let author_id = if author == old_author && initials == old_initials {
        author_id
    } else {
        ensure_comment_author(package, &author, &initials)?.0
    };
    let text = properties
        .get("text")
        .or_else(|| properties.get("comment"))
        .cloned()
        .unwrap_or(old_text);
    let date = properties.get("date").cloned().unwrap_or(old_date);
    let x = properties
        .get("x")
        .map(|value| unit_to_emu(value))
        .unwrap_or(old_x);
    let y = properties
        .get("y")
        .map(|value| unit_to_emu(value))
        .unwrap_or(old_y);
    let replacement = format!(
        r#"<p:cm authorId="{author_id}" dt="{}" idx="{index}"><p:pos x="{x}" y="{y}"/><p:text>{}</p:text></p:cm>"#,
        xml_escape_text(&date),
        xml_escape_text(&text),
    );
    let mut updated = xml;
    updated.replace_range(range, &replacement);
    package
        .write_part_xml(&comments_path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(properties
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "text" | "comment" | "author" | "initials" | "date" | "x" | "y"
            )
        })
        .cloned()
        .collect())
}

pub(crate) fn remove_comment(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<Option<String>, HandlerError> {
    let (slide_index, comment_index) = parse_comment_path(path)?;
    let (comments_path, xml, range, _, _, _, _, _, _) =
        comment_record(package, slide_index, comment_index)?;
    let mut updated = xml;
    updated.replace_range(range, "");
    package
        .write_part_xml(&comments_path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(Some(format!(
        "removed comment {comment_index} from slide {slide_index}"
    )))
}

#[allow(clippy::type_complexity)]
fn comment_record(
    package: &OxmlPackage,
    slide_index: usize,
    comment_index: usize,
) -> Result<
    (
        String,
        String,
        std::ops::Range<usize>,
        usize,
        String,
        String,
        String,
        String,
        String,
    ),
    HandlerError,
> {
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_index)?;
    let rels = package
        .part_rels(&slide_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let rel = rels
        .all()
        .values()
        .find(|rel| rel.type_uri == PPT_COMMENTS_REL_TYPE)
        .ok_or_else(|| {
            HandlerError::PathNotFound(format!("/slide[{slide_index}]/comment[{comment_index}]"))
        })?;
    let comments_path = package.resolve_rel_target(&slide_path, &rel.target);
    let xml = package
        .read_part_xml(&comments_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let (range, author_id, index, date, x, y, text) = {
        let document = roxmltree::Document::parse(&xml).map_err(|error| {
            HandlerError::OperationFailed(format!("invalid slide comments part: {error}"))
        })?;
        let comment = document
            .descendants()
            .filter(|node| node.has_tag_name("cm"))
            .nth(comment_index - 1)
            .ok_or_else(|| {
                HandlerError::PathNotFound(format!(
                    "/slide[{slide_index}]/comment[{comment_index}]"
                ))
            })?;
        let position = comment.children().find(|node| node.has_tag_name("pos"));
        (
            comment.range(),
            comment
                .attribute("authorId")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
            comment.attribute("idx").unwrap_or("0").to_string(),
            comment.attribute("dt").unwrap_or_default().to_string(),
            position
                .and_then(|node| node.attribute("x"))
                .unwrap_or("0")
                .to_string(),
            position
                .and_then(|node| node.attribute("y"))
                .unwrap_or("0")
                .to_string(),
            comment
                .children()
                .find(|node| node.has_tag_name("text"))
                .and_then(|node| node.text())
                .unwrap_or_default()
                .to_string(),
        )
    };
    Ok((
        comments_path,
        xml,
        range,
        author_id,
        index,
        date,
        x,
        y,
        text,
    ))
}

/// Add a speaker note.
fn add_note(
    package: &mut OxmlPackage,
    parent: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    let slide_num = parse_slide_num(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
    let notes_master_path = ensure_notes_master(package)?;
    let notes_path = ensure_notes_part(package, &slide_path, &notes_master_path)?;
    let existing_text = if package.has_part(&notes_path) {
        package
            .read_part_xml(&notes_path)
            .ok()
            .and_then(|xml| notes_text(&xml).ok())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let text = properties
        .get("text")
        .map(String::as_str)
        .unwrap_or(&existing_text);
    let direction = properties
        .get("direction")
        .or_else(|| properties.get("dir"))
        .or_else(|| properties.get("rtl"))
        .map(String::as_str);
    let lang = properties
        .get("lang")
        .map(String::as_str)
        .unwrap_or("en-US");
    let body_xml = notes_text_body_xml(text, direction, lang);

    if package.has_part(&notes_path) {
        let xml = package
            .read_part_xml(&notes_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        let range = notes_body_range(&xml)?;
        let mut updated = xml;
        updated.replace_range(range, &body_xml);
        package
            .write_part_xml(&notes_path, &updated)
            .map_err(|error| HandlerError::SaveError(error.to_string()))?;
        return Ok(format!("/slide[{}]/notes", slide_num));
    }

    let notes_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr/>
      <p:sp>
        <p:nvSpPr>
          <p:cNvPr id="2" name="Slide Image Placeholder"/>
          <p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>
          <p:nvPr><p:ph type="sldImg" idx="0"/></p:nvPr>
        </p:nvSpPr>
        <p:spPr/>
        <p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody>
      </p:sp>
      <p:sp>
        <p:nvSpPr>
          <p:cNvPr id="3" name="Notes Placeholder"/>
          <p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr>
          <p:nvPr><p:ph type="body" idx="1"/></p:nvPr>
        </p:nvSpPr>
        <p:spPr/>
        {body_xml}
      </p:sp>
    </p:spTree>
  </p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:notes>"#
    );

    package
        .write_part_xml(&notes_path, &notes_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    Ok(format!("/slide[{}]/notes", slide_num))
}

fn notes_text_body_xml(text: &str, direction: Option<&str>, lang: &str) -> String {
    let rtl = direction.is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "rtl" | "true" | "1" | "yes"
        )
    });
    let body_pr = if rtl {
        "<a:bodyPr rtlCol=\"1\"/>"
    } else {
        "<a:bodyPr/>"
    };
    let paragraph_pr = if rtl { "<a:pPr rtl=\"1\"/>" } else { "" };
    let paragraphs = text
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                format!(
                    "<a:p>{paragraph_pr}<a:endParaRPr lang=\"{}\"/></a:p>",
                    xml_escape_text(lang)
                )
            } else {
                format!(
                    "<a:p>{paragraph_pr}<a:r><a:rPr lang=\"{}\" dirty=\"0\"/><a:t>{}</a:t></a:r></a:p>",
                    xml_escape_text(lang),
                    xml_escape_text(line)
                )
            }
        })
        .collect::<String>();
    format!("<p:txBody>{body_pr}<a:lstStyle/>{paragraphs}</p:txBody>")
}

fn notes_body_range(xml: &str) -> Result<std::ops::Range<usize>, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid notes slide: {error}")))?;
    let shape = document
        .descendants()
        .filter(|node| node.has_tag_name("sp"))
        .find(|shape| {
            shape.descendants().any(|node| {
                node.has_tag_name("ph")
                    && (node.attribute("idx") == Some("1")
                        || node.attribute("type") == Some("body"))
            })
        })
        .ok_or_else(|| {
            HandlerError::OperationFailed("notes slide has no body placeholder".to_string())
        })?;
    shape
        .children()
        .find(|node| node.has_tag_name("txBody"))
        .map(|node| node.range())
        .ok_or_else(|| HandlerError::OperationFailed("notes body has no text body".to_string()))
}

const PPT_NOTES_SLIDE_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";
const PPT_NOTES_MASTER_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster";
const PPT_SLIDE_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
const PPT_SLIDE_MASTER_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
const PPT_THEME_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";

fn ensure_notes_master(package: &mut OxmlPackage) -> Result<String, HandlerError> {
    let existing_master = package
        .part_rels("ppt/presentation.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?
        .all()
        .values()
        .find(|rel| rel.type_uri == PPT_NOTES_MASTER_REL_TYPE)
        .map(|rel| package.resolve_rel_target("ppt/presentation.xml", &rel.target));
    let master_path = existing_master.unwrap_or_else(|| {
        let mut index = 1;
        loop {
            let candidate = format!("ppt/notesMasters/notesMaster{index}.xml");
            if !package.has_part(&candidate) {
                break candidate;
            }
            index += 1;
        }
    });
    ensure_content_type_override(
        package,
        &master_path,
        "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml",
    )?;
    ensure_ppt_relationship(
        package,
        "ppt/_rels/presentation.xml.rels",
        PPT_NOTES_MASTER_REL_TYPE,
        &format!(
            "notesMasters/{}",
            master_path.rsplit('/').next().unwrap_or("notesMaster1.xml")
        ),
    )?;
    if !package.has_part(&master_path) {
        package
            .write_part_xml(&master_path, &build_notes_master_xml(package)?)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        ensure_notes_master_theme(package, &master_path)?;
    }
    let rels = package
        .read_part_xml("ppt/_rels/presentation.xml.rels")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let rel_id = relationship_id_by_type(&rels, PPT_NOTES_MASTER_REL_TYPE).ok_or_else(|| {
        HandlerError::OperationFailed(
            "notes master relationship missing after creation".to_string(),
        )
    })?;
    let presentation = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let has_master_id = roxmltree::Document::parse(&presentation)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?
        .descendants()
        .any(|node| node.has_tag_name("notesMasterId"));
    if !has_master_id {
        let entry = format!(
            "<p:notesMasterIdLst><p:notesMasterId r:id=\"{rel_id}\"/></p:notesMasterIdLst>"
        );
        let insert = presentation
            .find("<p:sldIdLst")
            .or_else(|| presentation.find("</p:presentation>"))
            .ok_or_else(|| HandlerError::OperationFailed("invalid presentation.xml".to_string()))?;
        let mut updated = presentation;
        updated.insert_str(insert, &entry);
        package
            .write_part_xml("ppt/presentation.xml", &updated)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }
    Ok(master_path)
}

fn build_notes_master_xml(package: &OxmlPackage) -> Result<String, HandlerError> {
    let (page_width, page_height, slide_width, slide_height) = notes_master_sizes(package)?;
    let scale_x = |value: i64| value * page_width / 6_858_000;
    let scale_y = |value: i64| value * page_height / 9_144_000;
    let header_rect = (scale_x(0), scale_y(0), scale_x(2_971_800), scale_y(458_788));
    let date_rect = (
        scale_x(3_884_613),
        scale_y(0),
        scale_x(2_971_800),
        scale_y(458_788),
    );
    let image_x = scale_x(1_143_000);
    let image_y = scale_y(685_800);
    let image_width = scale_x(4_572_000);
    let image_height = if slide_width > 0 {
        image_width * slide_height / slide_width
    } else {
        scale_y(3_429_000)
    };
    let footer_rect = (
        scale_x(0),
        scale_y(8_685_213),
        scale_x(2_971_800),
        scale_y(458_787),
    );
    let number_rect = (
        scale_x(3_884_613),
        scale_y(8_685_213),
        scale_x(2_971_800),
        scale_y(458_787),
    );
    let body_x = scale_x(685_800);
    let body_width = scale_x(5_486_400);
    let nominal_body_height = scale_y(4_114_800);
    let quarter_inch = scale_y(228_600).max(1);
    let footer_y = footer_rect.1;
    let body_y = (((image_y + image_height) / quarter_inch) + 1) * quarter_inch;
    let body_y = body_y.min((footer_y - quarter_inch).max(image_y));
    let body_height = nominal_body_height
        .min((footer_y - body_y).max(quarter_inch))
        .max(quarter_inch);
    let mut notes_style = String::from("<p:notesStyle>");
    for level in 1..=9 {
        notes_style.push_str(&format!(
            "<a:lvl{level}pPr marL=\"{}\" algn=\"l\" defTabSz=\"914400\"><a:defRPr sz=\"1200\"/></a:lvl{level}pPr>",
            (level - 1) * 457_200
        ));
    }
    notes_style.push_str("</p:notesStyle>");
    let header = notes_master_placeholder(2, "Header Placeholder 1", "hdr", None, header_rect);
    let date = notes_master_placeholder(3, "Date Placeholder 2", "dt", Some(1), date_rect);
    let image = notes_master_placeholder(
        4,
        "Slide Image Placeholder 3",
        "sldImg",
        Some(2),
        (image_x, image_y, image_width, image_height),
    );
    let body = notes_master_placeholder(
        5,
        "Notes Placeholder 4",
        "body",
        Some(3),
        (body_x, body_y, body_width, body_height),
    );
    let footer = notes_master_placeholder(6, "Footer Placeholder 5", "ftr", Some(4), footer_rect);
    let number = notes_master_placeholder(
        7,
        "Slide Number Placeholder 6",
        "sldNum",
        Some(5),
        number_rect,
    );
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notesMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld name=""><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{header}{date}{image}{body}{footer}{number}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>{notes_style}</p:notesMaster>"#
    ))
}

fn notes_master_sizes(package: &OxmlPackage) -> Result<(i64, i64, i64, i64), HandlerError> {
    let xml = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let document = roxmltree::Document::parse(&xml)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let dimensions = |name: &str, default_width: i64, default_height: i64| {
        document
            .descendants()
            .find(|node| node.has_tag_name(name))
            .map(|node| {
                (
                    node.attribute("cx")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(default_width),
                    node.attribute("cy")
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(default_height),
                )
            })
            .unwrap_or((default_width, default_height))
    };
    let (notes_width, notes_height) = dimensions("notesSz", 6_858_000, 9_144_000);
    let (slide_width, slide_height) = dimensions("sldSz", 12_192_000, 6_858_000);
    Ok((notes_width, notes_height, slide_width, slide_height))
}

fn notes_master_placeholder(
    id: usize,
    name: &str,
    placeholder_type: &str,
    index: Option<usize>,
    rect: (i64, i64, i64, i64),
) -> String {
    let (x, y, width, height) = rect;
    let index = index
        .map(|index| format!(" idx=\"{index}\""))
        .unwrap_or_default();
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="{placeholder_type}"{index}/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{width}" cy="{height}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp>"#
    )
}

fn ensure_notes_master_theme(
    package: &mut OxmlPackage,
    master_path: &str,
) -> Result<(), HandlerError> {
    let master_rels_path = crate::navigation::relationships_part_path(master_path);
    if package
        .part_rels(master_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?
        .all()
        .values()
        .any(|rel| rel.type_uri == PPT_THEME_REL_TYPE)
    {
        return Ok(());
    }
    // A notes master owns its theme.  Cloning an existing presentation/master
    // theme preserves the deck palette, but blank/minimal decks do not have a
    // source to clone.  In that case create a schema-complete Office baseline:
    // the +mn-lt/+mn-ea/+mn-cs and tx1 references in notesStyle otherwise have
    // no relationship through which PowerPoint can resolve them.
    let source = match find_presentation_theme_source(package)? {
        Some(source_path) => package
            .read_part_bytes(&source_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?
            .clone(),
        None => default_notes_theme_xml().as_bytes().to_vec(),
    };
    let mut index = 1;
    let target_path = loop {
        let candidate = format!("ppt/theme/theme{index}.xml");
        if !package.has_part(&candidate) {
            break candidate;
        }
        index += 1;
    };
    package
        .write_part(&target_path, source)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    ensure_content_type_override(
        package,
        &target_path,
        "application/vnd.openxmlformats-officedocument.theme+xml",
    )?;
    ensure_ppt_relationship(
        package,
        &master_rels_path,
        PPT_THEME_REL_TYPE,
        &format!(
            "../theme/{}",
            target_path.rsplit('/').next().unwrap_or("theme1.xml")
        ),
    )
}

fn default_notes_theme_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme"><a:themeElements><a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350" cap="flat"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700" cap="flat"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050" cap="flat"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#
}

fn find_presentation_theme_source(package: &OxmlPackage) -> Result<Option<String>, HandlerError> {
    let presentation_path = "ppt/presentation.xml";
    let presentation_rels = package
        .part_rels(presentation_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    if let Some(theme) = presentation_rels
        .all()
        .values()
        .find(|rel| rel.type_uri == PPT_THEME_REL_TYPE)
    {
        return Ok(Some(
            package.resolve_rel_target(presentation_path, &theme.target),
        ));
    }
    for master in presentation_rels
        .all()
        .values()
        .filter(|rel| rel.type_uri == PPT_SLIDE_MASTER_REL_TYPE)
    {
        let master_path = package.resolve_rel_target(presentation_path, &master.target);
        let master_rels = package
            .part_rels(&master_path)
            .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
        if let Some(theme) = master_rels
            .all()
            .values()
            .find(|rel| rel.type_uri == PPT_THEME_REL_TYPE)
        {
            return Ok(Some(
                package.resolve_rel_target(&master_path, &theme.target),
            ));
        }
    }
    Ok(None)
}

fn ensure_notes_part(
    package: &mut OxmlPackage,
    slide_path: &str,
    master_path: &str,
) -> Result<String, HandlerError> {
    if let Ok(rels) = package.part_rels(slide_path) {
        if let Some(rel) = rels
            .all()
            .values()
            .find(|rel| rel.type_uri == PPT_NOTES_SLIDE_REL_TYPE)
        {
            return Ok(package.resolve_rel_target(slide_path, &rel.target));
        }
    }
    let mut index = 1;
    let notes_path = loop {
        let candidate = format!("ppt/notesSlides/notesSlide{index}.xml");
        if !package.has_part(&candidate) {
            break candidate;
        }
        index += 1;
    };
    ensure_content_type_override(
        package,
        &notes_path,
        "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml",
    )?;
    ensure_ppt_relationship(
        package,
        &crate::navigation::relationships_part_path(slide_path),
        PPT_NOTES_SLIDE_REL_TYPE,
        &format!(
            "../notesSlides/{}",
            notes_path.rsplit('/').next().unwrap_or_default()
        ),
    )?;
    let notes_rels = crate::navigation::relationships_part_path(&notes_path);
    ensure_ppt_relationship(
        package,
        &notes_rels,
        PPT_NOTES_MASTER_REL_TYPE,
        &format!(
            "../notesMasters/{}",
            master_path.rsplit('/').next().unwrap_or_default()
        ),
    )?;
    ensure_ppt_relationship(
        package,
        &notes_rels,
        PPT_SLIDE_REL_TYPE,
        &format!(
            "../slides/{}",
            slide_path.rsplit('/').next().unwrap_or_default()
        ),
    )?;
    Ok(notes_path)
}

fn relationship_id_by_type(xml: &str, relationship_type: &str) -> Option<String> {
    let document = roxmltree::Document::parse(xml).ok()?;
    document
        .descendants()
        .find(|node| {
            node.has_tag_name("Relationship") && node.attribute("Type") == Some(relationship_type)
        })
        .and_then(|node| node.attribute("Id"))
        .map(str::to_string)
}

pub(crate) fn get_notes_node(
    package: &OxmlPackage,
    path: &str,
) -> Result<handler_common::DocumentNode, HandlerError> {
    let slide_index = parse_notes_path(path)?;
    let (_, xml) = notes_part_xml(package, slide_index)?;
    let text = notes_text(&xml)?;
    let mut node = handler_common::DocumentNode::new(path, "notes")
        .with_text(&text)
        .with_format("text", serde_json::Value::String(text));
    for (key, value) in notes_format(&xml)? {
        node.format
            .insert(key, Some(serde_json::Value::String(value)));
    }
    Ok(node)
}

pub(crate) fn list_notes_nodes(
    package: &OxmlPackage,
) -> Result<Vec<handler_common::DocumentNode>, HandlerError> {
    let presentation = crate::navigation::build_presentation(package)?;
    let nodes = presentation
        .slides
        .into_iter()
        .filter_map(|slide| {
            let path = format!("/slide[{}]/notes", slide.index);
            get_notes_node(package, &path).ok()
        })
        .collect();
    Ok(nodes)
}

pub(crate) fn set_notes(
    package: &mut OxmlPackage,
    path: &str,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let slide_index = parse_notes_path(path)?;
    let unsupported: Vec<_> = properties
        .keys()
        .filter(|key| !matches!(key.as_str(), "text" | "direction" | "dir" | "rtl" | "lang"))
        .cloned()
        .collect();
    let (_, existing_xml) = notes_part_xml(package, slide_index)?;
    let existing_text = notes_text(&existing_xml)?;
    let existing_format = notes_format(&existing_xml)?;
    let text = properties.get("text").cloned().unwrap_or(existing_text);
    let mut add_props = HashMap::from([("text".to_string(), text)]);
    if let Some(direction) = properties
        .get("direction")
        .or_else(|| properties.get("dir"))
        .or_else(|| properties.get("rtl"))
    {
        add_props.insert("direction".to_string(), direction.clone());
    } else if let Some(direction) = existing_format.get("direction") {
        add_props.insert("direction".to_string(), direction.clone());
    }
    if let Some(lang) = properties.get("lang") {
        add_props.insert("lang".to_string(), lang.clone());
    } else if let Some(lang) = existing_format.get("lang") {
        add_props.insert("lang".to_string(), lang.clone());
    }
    add_note(package, &format!("/slide[{slide_index}]"), &add_props)?;
    Ok(unsupported)
}

pub(crate) fn remove_notes(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<Option<String>, HandlerError> {
    let slide_index = parse_notes_path(path)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_index)?;
    let rels_path = crate::navigation::relationships_part_path(&slide_path);
    let rels_xml = package
        .read_part_xml(&rels_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let (rel_range, target) = {
        let document = roxmltree::Document::parse(&rels_xml)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        let rel = document
            .descendants()
            .find(|node| {
                node.has_tag_name("Relationship")
                    && node.attribute("Type") == Some(PPT_NOTES_SLIDE_REL_TYPE)
            })
            .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
        (
            rel.range(),
            rel.attribute("Target").unwrap_or_default().to_string(),
        )
    };
    let notes_path = package.resolve_rel_target(&slide_path, &target);
    let mut updated_rels = rels_xml;
    updated_rels.replace_range(rel_range, "");
    package
        .write_part_xml(&rels_path, &updated_rels)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    package
        .remove_part(&notes_path)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    remove_content_type_override(package, &notes_path)?;
    let notes_rels = crate::navigation::relationships_part_path(&notes_path);
    let _ = package.remove_part(&notes_rels);
    Ok(Some(format!("removed notes from slide {slide_index}")))
}

fn parse_notes_path(path: &str) -> Result<usize, HandlerError> {
    let path = path.strip_suffix("/notes").ok_or_else(|| {
        HandlerError::InvalidPath(format!("expected /slide[N]/notes, got: {path}"))
    })?;
    parse_slide_num(path)
}

fn notes_part_xml(
    package: &OxmlPackage,
    slide_index: usize,
) -> Result<(String, String), HandlerError> {
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_index)?;
    let rels = package
        .part_rels(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let rel = rels
        .all()
        .values()
        .find(|rel| rel.type_uri == PPT_NOTES_SLIDE_REL_TYPE)
        .ok_or_else(|| HandlerError::PathNotFound(format!("/slide[{slide_index}]/notes")))?;
    let notes_path = package.resolve_rel_target(&slide_path, &rel.target);
    let xml = package
        .read_part_xml(&notes_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    Ok((notes_path, xml))
}

fn notes_text(xml: &str) -> Result<String, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid notes slide: {e}")))?;
    let shape = document
        .descendants()
        .filter(|node| node.has_tag_name("sp"))
        .find(|shape| {
            shape.descendants().any(|node| {
                node.has_tag_name("ph")
                    && (node.attribute("idx") == Some("1")
                        || node.attribute("type") == Some("body"))
            })
        })
        .ok_or_else(|| {
            HandlerError::OperationFailed("notes slide has no body placeholder".to_string())
        })?;
    Ok(shape
        .descendants()
        .filter(|node| node.has_tag_name("p"))
        .map(|paragraph| {
            paragraph
                .descendants()
                .filter(|node| node.has_tag_name("t"))
                .filter_map(|node| node.text())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn notes_format(xml: &str) -> Result<std::collections::HashMap<String, String>, HandlerError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid notes slide: {e}")))?;
    let Some(shape) = document
        .descendants()
        .filter(|node| node.has_tag_name("sp"))
        .find(|shape| {
            shape.descendants().any(|node| {
                node.has_tag_name("ph")
                    && (node.attribute("idx") == Some("1")
                        || node.attribute("type") == Some("body"))
            })
        })
    else {
        return Ok(std::collections::HashMap::new());
    };
    let mut format = std::collections::HashMap::new();
    if shape
        .descendants()
        .any(|node| node.has_tag_name("pPr") && matches!(node.attribute("rtl"), Some("1" | "true")))
    {
        format.insert("direction".to_string(), "rtl".to_string());
    }
    if let Some(lang) = shape
        .descendants()
        .find(|node| node.has_tag_name("rPr"))
        .and_then(|node| node.attribute("lang"))
        .or_else(|| {
            shape
                .descendants()
                .find(|node| node.has_tag_name("endParaRPr"))
                .and_then(|node| node.attribute("lang"))
        })
    {
        format.insert("lang".to_string(), lang.to_string());
    }
    Ok(format)
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
    fn add_comment_wires_author_part_slide_relationship_and_content_types() {
        let mut package = OxmlPackage::create("comment-test.pptx");
        package.add_part(
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#,
        );
        package.add_part("ppt/presentation.xml", br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#);
        package.add_part("ppt/_rels/presentation.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide7.xml"/></Relationships>"#);
        package.add_part("ppt/slides/slide7.xml", br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#);

        let props = HashMap::from([
            ("author".to_string(), "Jane Doe".to_string()),
            ("text".to_string(), "Review this".to_string()),
            ("x".to_string(), "2cm".to_string()),
        ]);
        assert_eq!(
            add_comment(&mut package, "/slide[1]", &props).unwrap(),
            "/slide[1]/comment[1]"
        );
        assert_eq!(
            add_comment(&mut package, "/slide[1]", &props).unwrap(),
            "/slide[1]/comment[2]"
        );

        let node = get_comment_node(&package, "/slide[1]/comment[2]").unwrap();
        assert_eq!(node.text.as_deref(), Some("Review this"));
        assert_eq!(
            node.format["author"],
            Some(serde_json::Value::String("Jane Doe".into()))
        );
        assert_eq!(list_comment_nodes(&package, None).unwrap().len(), 2);
        let update = HashMap::from([
            ("text".to_string(), "Updated".to_string()),
            ("y".to_string(), "1cm".to_string()),
        ]);
        assert!(set_comment(&mut package, "/slide[1]/comment[2]", &update)
            .unwrap()
            .is_empty());
        let updated = get_comment_node(&package, "/slide[1]/comment[2]").unwrap();
        assert_eq!(updated.text.as_deref(), Some("Updated"));
        assert_eq!(
            updated.format["y"],
            Some(serde_json::Value::String("1cm".into()))
        );
        assert!(remove_comment(&mut package, "/slide[1]/comment[1]")
            .unwrap()
            .is_some());
        assert_eq!(list_comment_nodes(&package, None).unwrap().len(), 1);

        let authors = package.read_part_xml("ppt/commentAuthors.xml").unwrap();
        assert!(authors.contains("name=\"Jane Doe\""));
        assert!(authors.contains("initials=\"JD\""));
        assert!(authors.contains("lastIdx=\"2\""));
        let comments = package.read_part_xml("ppt/comments/comment1.xml").unwrap();
        assert!(comments.contains("authorId=\"0\""));
        assert!(comments.contains("idx=\"2\""));
        assert!(comments.contains("<p:pos x=\"720000\" y=\"360000\"/>"));
        let slide_rels = package
            .read_part_xml("ppt/slides/_rels/slide7.xml.rels")
            .unwrap();
        assert!(slide_rels.contains(PPT_COMMENTS_REL_TYPE));
        let presentation_rels = package
            .read_part_xml("ppt/_rels/presentation.xml.rels")
            .unwrap();
        assert!(presentation_rels.contains(PPT_COMMENT_AUTHORS_REL_TYPE));
        let types = package.read_part_xml("[Content_Types].xml").unwrap();
        assert!(types.contains("/ppt/comments/comment1.xml"));
        assert!(types.contains("/ppt/commentAuthors.xml"));
    }

    #[test]
    fn add_note_wires_slide_master_backlink_and_content_types() {
        let mut package = OxmlPackage::create("notes-test.pptx");
        package.add_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#);
        package.add_part("ppt/presentation.xml", br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#);
        package.add_part("ppt/_rels/presentation.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide7.xml"/></Relationships>"#);
        package.add_part("ppt/slides/slide7.xml", br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#);

        assert_eq!(
            add_note(
                &mut package,
                "/slide[1]",
                &HashMap::from([("text".to_string(), "Speaker cue".to_string())])
            )
            .unwrap(),
            "/slide[1]/notes"
        );
        assert_eq!(
            get_notes_node(&package, "/slide[1]/notes")
                .unwrap()
                .text
                .as_deref(),
            Some("Speaker cue")
        );
        assert_eq!(list_notes_nodes(&package).unwrap().len(), 1);
        assert!(set_notes(
            &mut package,
            "/slide[1]/notes",
            &HashMap::from([
                ("text".to_string(), "Updated cue\nSecond line".to_string()),
                ("direction".to_string(), "rtl".to_string()),
                ("lang".to_string(), "ar-SA".to_string()),
            ]),
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            get_notes_node(&package, "/slide[1]/notes")
                .unwrap()
                .text
                .as_deref(),
            Some("Updated cue\nSecond line")
        );
        let notes = get_notes_node(&package, "/slide[1]/notes").unwrap();
        assert_eq!(
            notes.format["direction"],
            Some(serde_json::Value::String("rtl".into()))
        );
        assert_eq!(
            notes.format["lang"],
            Some(serde_json::Value::String("ar-SA".into()))
        );
        assert!(package
            .read_part_xml("ppt/notesSlides/notesSlide1.xml")
            .unwrap()
            .contains("<a:pPr rtl=\"1\"/>"));
        assert!(package
            .read_part_xml("ppt/notesSlides/notesSlide1.xml")
            .unwrap()
            .contains("Second line"));
        assert!(package
            .read_part_xml("ppt/notesMasters/notesMaster1.xml")
            .is_ok());
        let slide_rels = package
            .read_part_xml("ppt/slides/_rels/slide7.xml.rels")
            .unwrap();
        assert!(slide_rels.contains(PPT_NOTES_SLIDE_REL_TYPE));
        let notes_rels = package
            .read_part_xml("ppt/notesSlides/_rels/notesSlide1.xml.rels")
            .unwrap();
        assert!(notes_rels.contains(PPT_NOTES_MASTER_REL_TYPE));
        assert!(notes_rels.contains(PPT_SLIDE_REL_TYPE));
        let presentation = package.read_part_xml("ppt/presentation.xml").unwrap();
        assert!(presentation.contains("notesMasterIdLst"));
        let types = package.read_part_xml("[Content_Types].xml").unwrap();
        assert!(types.contains("/ppt/notesSlides/notesSlide1.xml"));
        assert!(types.contains("/ppt/notesMasters/notesMaster1.xml"));
        assert!(types.contains("/ppt/theme/theme1.xml"));
        assert!(package
            .read_part_xml("ppt/theme/theme1.xml")
            .unwrap()
            .contains("<a:themeElements>"));
        assert!(package
            .read_part_xml("ppt/notesMasters/_rels/notesMaster1.xml.rels")
            .unwrap()
            .contains(PPT_THEME_REL_TYPE));
        assert!(remove_notes(&mut package, "/slide[1]/notes")
            .unwrap()
            .is_some());
        assert!(get_notes_node(&package, "/slide[1]/notes").is_err());
        assert!(!package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains("/ppt/notesSlides/notesSlide1.xml"));
    }

    #[test]
    fn add_note_reuses_existing_related_notes_master() {
        let mut package = OxmlPackage::create("existing-notes-master.pptx");
        package.add_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#);
        package.add_part("ppt/presentation.xml", br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#);
        package.add_part("ppt/_rels/presentation.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId9" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster" Target="notesMasters/notesMaster7.xml"/></Relationships>"#);
        package.add_part("ppt/slides/slide1.xml", br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#);
        package.add_part("ppt/notesMasters/notesMaster7.xml", br#"<p:notesMaster xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:notesMaster>"#);

        add_note(
            &mut package,
            "/slide[1]",
            &HashMap::from([("text".to_string(), "Use existing master".to_string())]),
        )
        .unwrap();

        assert!(!package.has_part("ppt/notesMasters/notesMaster1.xml"));
        assert!(package.has_part("ppt/notesMasters/notesMaster7.xml"));
        assert!(package
            .read_part_xml("ppt/notesSlides/_rels/notesSlide1.xml.rels")
            .unwrap()
            .contains("../notesMasters/notesMaster7.xml"));
    }

    #[test]
    fn add_note_clones_slide_master_theme_for_new_notes_master() {
        let mut package = OxmlPackage::create("notes-theme.pptx");
        package.add_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#);
        package.add_part("ppt/presentation.xml", br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldMasterIdLst/><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#);
        package.add_part("ppt/_rels/presentation.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/></Relationships>"#);
        package.add_part("ppt/slides/slide1.xml", br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#);
        package.add_part("ppt/slideMasters/slideMaster1.xml", br#"<p:sldMaster xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"/>"#);
        package.add_part("ppt/slideMasters/_rels/slideMaster1.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/></Relationships>"#);
        package.add_part("ppt/theme/theme1.xml", br#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Source Theme"/>"#);

        add_note(&mut package, "/slide[1]", &HashMap::new()).unwrap();

        assert!(package.has_part("ppt/theme/theme2.xml"));
        assert_eq!(
            package.read_part_xml("ppt/theme/theme2.xml").unwrap(),
            package.read_part_xml("ppt/theme/theme1.xml").unwrap()
        );
        let master_rels = package
            .read_part_xml("ppt/notesMasters/_rels/notesMaster1.xml.rels")
            .unwrap();
        assert!(master_rels.contains(PPT_THEME_REL_TYPE));
        assert!(master_rels.contains("../theme/theme2.xml"));
        assert!(package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains("/ppt/theme/theme2.xml"));
    }

    #[test]
    fn new_notes_master_scales_placeholders_to_presentation_sizes() {
        let mut package = OxmlPackage::create("scaled-notes-master.pptx");
        package.add_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#);
        package.add_part("ppt/presentation.xml", br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst><p:sldSz cx="9144000" cy="6858000"/><p:notesSz cx="13716000" cy="9144000"/></p:presentation>"#);
        package.add_part("ppt/_rels/presentation.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#);
        package.add_part("ppt/slides/slide1.xml", br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#);

        add_note(&mut package, "/slide[1]", &HashMap::new()).unwrap();

        let master = package
            .read_part_xml("ppt/notesMasters/notesMaster1.xml")
            .unwrap();
        assert!(master.contains("<a:ext cx=\"5943600\" cy=\"458788\"/>"));
        assert!(master
            .contains("<a:off x=\"2286000\" y=\"685800\"/><a:ext cx=\"9144000\" cy=\"6858000\"/>"));
    }

    #[test]
    fn add_modern_comment_wires_authors_and_slide_part() {
        let mut package = OxmlPackage::create("modern-comment-test.pptx");
        package.add_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#);
        package.add_part("ppt/presentation.xml", br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#);
        package.add_part("ppt/_rels/presentation.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#);
        package.add_part("ppt/slides/slide1.xml", br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#);
        let result = add_modern_comment(
            &mut package,
            "/slide[1]",
            &HashMap::from([
                ("author".into(), "Ada Lovelace".into()),
                ("text".into(), "Thread start".into()),
                ("resolved".into(), "true".into()),
            ]),
        )
        .unwrap();
        assert_eq!(result, "/slide[1]/modernComment[1]");
        assert_eq!(
            add_modern_comment(
                &mut package,
                "/slide[1]",
                &HashMap::from([
                    ("text".into(), "Reply".into()),
                    ("parent".into(), "/slide[1]/modernComment[1]".into())
                ])
            )
            .unwrap(),
            "/slide[1]/modernComment[1]/reply[1]"
        );
        assert!(package
            .read_part_xml("ppt/authors.xml")
            .unwrap()
            .contains("Ada Lovelace"));
        let comments = package
            .read_part_xml("ppt/comments/modernComment1.xml")
            .unwrap();
        assert!(comments.contains("Thread start"));
        assert!(comments.contains("status=\"resolved\""));
        assert!(comments.contains("<p188:reply"));
        assert!(package
            .read_part_xml("ppt/slides/_rels/slide1.xml.rels")
            .unwrap()
            .contains("2018/10/relationships/comment"));

        let top = get_modern_comment_node(&package, "/slide[1]/modernComment[1]").unwrap();
        assert_eq!(top.text.as_deref(), Some("Thread start"));
        assert_eq!(top.child_count, 1);
        assert_eq!(top.format["resolved"], Some(serde_json::Value::Bool(true)));
        assert_eq!(
            get_modern_comment_node(&package, "/slide[1]/modernComment[1]/reply[1]")
                .unwrap()
                .text
                .as_deref(),
            Some("Reply")
        );
        assert_eq!(list_modern_comment_nodes(&package, None).unwrap().len(), 1);

        assert!(set_modern_comment(
            &mut package,
            "/slide[1]/modernComment[1]",
            &HashMap::from([
                ("text".to_string(), "Edited thread".to_string()),
                ("resolved".to_string(), "false".to_string()),
                (
                    "created".to_string(),
                    "2026-01-02T03:04:05+08:00".to_string()
                ),
            ]),
        )
        .unwrap()
        .is_empty());
        let edited = get_modern_comment_node(&package, "/slide[1]/modernComment[1]").unwrap();
        assert_eq!(edited.text.as_deref(), Some("Edited thread"));
        assert_eq!(
            edited.format["resolved"],
            Some(serde_json::Value::Bool(false))
        );
        assert_eq!(
            edited.format["created"],
            Some(serde_json::Value::String("2026-01-01T19:04:05Z".into()))
        );

        assert!(
            remove_modern_comment(&mut package, "/slide[1]/modernComment[1]/reply[1]")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            get_modern_comment_node(&package, "/slide[1]/modernComment[1]")
                .unwrap()
                .child_count,
            0
        );
        assert!(
            remove_modern_comment(&mut package, "/slide[1]/modernComment[1]")
                .unwrap()
                .is_some()
        );
        assert!(!package.has_part("ppt/comments/modernComment1.xml"));
        assert!(!package
            .read_part_xml("ppt/slides/_rels/slide1.xml.rels")
            .unwrap()
            .contains("2018/10/relationships/comment"));
        assert!(!package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains("/ppt/comments/modernComment1.xml"));
    }

    #[test]
    fn add_modern_comment_reuses_related_authors_part() {
        let mut package = OxmlPackage::create("existing-modern-authors.pptx");
        package.add_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"></Types>"#);
        package.add_part("ppt/presentation.xml", br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#);
        package.add_part("ppt/_rels/presentation.xml.rels", br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/><Relationship Id="rId8" Type="http://schemas.microsoft.com/office/2018/10/relationships/authors" Target="custom/authors2.xml"/></Relationships>"#);
        package.add_part("ppt/slides/slide1.xml", br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree/></p:cSld></p:sld>"#);
        package.add_part("ppt/custom/authors2.xml", br#"<p188:authorLst xmlns:p188="http://schemas.microsoft.com/office/powerpoint/2018/8/main"/>"#);

        add_modern_comment(
            &mut package,
            "/slide[1]",
            &HashMap::from([
                ("author".to_string(), "Related Author".to_string()),
                ("text".to_string(), "uses existing part".to_string()),
            ]),
        )
        .unwrap();

        assert!(!package.has_part("ppt/authors.xml"));
        assert!(package
            .read_part_xml("ppt/custom/authors2.xml")
            .unwrap()
            .contains("Related Author"));
    }

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

#[cfg(test)]
mod table_tests {
    use super::*;

    fn minimal_package() -> OxmlPackage {
        let mut package = OxmlPackage::create("table-test.pptx");
        package.add_part(
            "ppt/presentation.xml",
            br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#,
        );
        package.add_part(
            "ppt/_rels/presentation.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#,
        );
        package.add_part(
            "ppt/slides/slide1.xml",
            br#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld></p:sld>"#,
        );
        package
    }

    #[test]
    fn table_rejects_zero_dimensions_before_cell_generation() {
        let mut package = minimal_package();
        let properties = HashMap::from([
            ("rows".to_string(), "0".to_string()),
            ("cols".to_string(), "2".to_string()),
        ]);
        let error = add_table(&mut package, "/slide[1]", &properties).unwrap_err();
        assert!(matches!(error, HandlerError::InvalidArgument(_)));
    }

    #[test]
    fn table_uses_positive_frame_fallback_for_non_positive_geometry() {
        let mut package = minimal_package();
        let properties = HashMap::from([
            ("rows".to_string(), "2".to_string()),
            ("cols".to_string(), "2".to_string()),
            ("width".to_string(), "0".to_string()),
            ("height".to_string(), "-1".to_string()),
        ]);
        add_table(&mut package, "/slide[1]", &properties).unwrap();
        let xml = package.read_part_xml("ppt/slides/slide1.xml").unwrap();
        assert!(xml.contains("<a:ext cx=\"8382000\" cy=\"1143000\"/>"));
        assert!(!xml.contains("<a:ext cx=\"0\""));
        assert!(!xml.contains("cy=\"-1\""));
    }
}
