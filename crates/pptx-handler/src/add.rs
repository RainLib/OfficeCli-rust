use handler_common::{HandlerError, InsertPosition};
use oxml::OxmlPackage;
use std::collections::HashMap;

/// Add an element to the PPTX presentation.
pub fn add_element(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    _position: InsertPosition,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    match element_type {
        "slide" => add_slide(package, parent),
        "shape" | "textbox" => add_shape(package, parent, element_type, properties),
        "text" => add_text_to_shape(package, parent, properties),
        other => Err(HandlerError::UnsupportedType(format!(
            "PPTX add '{}' not supported",
            other
        ))),
    }
}

fn add_slide(package: &mut OxmlPackage, _parent: &str) -> Result<String, HandlerError> {
    // Count existing slides to determine next slide number
    let pres = crate::navigation::build_presentation(package)?;
    let slide_num = pres.slides.len() + 1;
    let slide_path = format!("ppt/slides/slide{}.xml", slide_num);

    let slide_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
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
    );

    package
        .write_part_xml(&slide_path, &slide_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Update presentation.xml to add the new slide reference
    update_presentation_slides(package, slide_num)?;

    Ok(format!("/slide[{}]", slide_num))
}

fn add_shape(
    package: &mut OxmlPackage,
    parent: &str,
    element_type: &str,
    properties: &HashMap<String, String>,
) -> Result<String, HandlerError> {
    // Parse parent path to find slide
    let slide_num = parse_slide_num(parent)?;
    let slide_path = format!("ppt/slides/slide{}.xml", slide_num);

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
    slide_num: usize,
) -> Result<(), HandlerError> {
    let pres_xml = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Add slide ID entry: <p:sldId id="256+N" r:id="rIdN"/>
    // We need to find the next available rId and sldId
    let sld_id = 256 + slide_num;
    let r_id = format!("rId{}", slide_num + 2); // rId1 is usually the slide master

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

    // Update presentation relationships
    let rels_path = "ppt/_rels/presentation.xml.rels";
    let rels_xml = package
        .read_part_xml(rels_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let new_rel = format!(
        "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
        r_id, slide_num
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

fn create_text_shape_xml(id: usize, name: &str, text: &str) -> String {
    let escaped_text = xml_escape_text(text);
    format!(
        r#"<p:sp>
  <p:nvSpPr>
    <p:cNvPr id="{id}" name="{name}"/>
    <p:cNvSpPr txBox="1"/>
    <p:nvPr/>
  </p:nvSpPr>
  <p:spPr>
    <a:xfrm><a:off x="457200" y="274638"/><a:ext cx="8382000" cy="304800"/></a:xfrm>
    <a:prstGeom prst="rect"><a:avLst/></a:prstGeom>
  </p:spPr>
  <p:txBody>
    <a:bodyPr/>
    <a:lstStyle/>
    <a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{escaped_text}</a:t></a:r></a:p>
  </p:txBody>
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
