use crate::dom_types::{Shape, Slide};
use crate::navigation::{build_presentation, find_paragraph, find_shape, find_slide};
use handler_common::{
    self, extract_find_replace_props, replace_in_string, DocumentIssue, DocumentNode,
    FindReplaceOptions, HandlerError, IssueSeverity, ValidationError, ViewOptions,
};

/// ViewAsText: show all slide text content, similar to Word's view_as_text.
pub fn view_as_text(
    package: &oxml::OxmlPackage,
    opts: &ViewOptions,
) -> Result<String, HandlerError> {
    let pres = build_presentation(package)?;
    let mut lines = Vec::new();

    for slide in &pres.slides {
        lines.push(format!("--- Slide {} ---", slide.index));
        for shape in &slide.shapes {
            if shape.text.is_empty() {
                continue;
            }
            // For placeholder shapes, label them
            let label = match &shape.placeholder_type {
                Some(pt) => format!("({}) ", pt),
                None => String::new(),
            };
            lines.push(format!("  {}{}", label, shape.text));
        }
    }

    let full_text = lines.join("\n");
    Ok(apply_line_range(&full_text, opts))
}
pub fn view_as_outline(package: &oxml::OxmlPackage) -> Result<String, HandlerError> {
    let pres = build_presentation(package)?;
    let mut lines = Vec::new();

    lines.push(format!("Presentation: {} slides", pres.slides.len()));
    for slide in &pres.slides {
        let morph_tag = if slide.has_morph {
            format!(" [morph: {} candidates]", slide.morph_candidates)
        } else {
            String::new()
        };
        lines.push(format!(
            "  slide[{}]: {} shapes{}",
            slide.index,
            slide.shapes.len(),
            morph_tag
        ));
        for (si, shape) in slide.shapes.iter().enumerate() {
            let shape_type = shape.placeholder_type.as_deref().unwrap_or("shape");
            let preview = if shape.text.chars().count() > 60 {
                truncate_str(&shape.text, 60)
            } else if shape.text.is_empty() {
                "(no text)".to_string()
            } else {
                shape.text.clone()
            };
            lines.push(format!(
                "    shape[{}]: {} \"{}\" — {} paragraphs, id={} [{}]",
                si + 1,
                shape_type,
                preview,
                shape.paragraphs.len(),
                shape.id,
                shape.name,
            ));
        }
    }

    Ok(lines.join("\n"))
}

/// ViewAsAnnotated: show slide text with path annotations.
pub fn view_as_annotated(
    package: &oxml::OxmlPackage,
    opts: &ViewOptions,
) -> Result<String, HandlerError> {
    let pres = build_presentation(package)?;
    let mut lines = Vec::new();

    for slide in &pres.slides {
        lines.push(format!(
            "[/slide[{}]] --- Slide {} ---",
            slide.index, slide.index
        ));
        for (si, shape) in slide.shapes.iter().enumerate() {
            let label = match &shape.placeholder_type {
                Some(pt) => format!("({}) ", pt),
                None => String::new(),
            };
            lines.push(format!(
                "[/slide[{}]/shape[{}]] {}{}",
                slide.index,
                si + 1,
                label,
                shape.text
            ));
        }
    }

    let full_text = lines.join("\n");
    Ok(apply_line_range(&full_text, opts))
}

/// ViewAsStats: show presentation statistics.
pub fn view_as_stats(package: &oxml::OxmlPackage) -> Result<String, HandlerError> {
    let pres = build_presentation(package)?;
    let mut total_shapes = 0;
    let mut total_paragraphs = 0;
    let mut total_chars = 0;

    for slide in &pres.slides {
        total_shapes += slide.shapes.len();
        for shape in &slide.shapes {
            total_paragraphs += shape.paragraphs.len();
            total_chars += shape.text.len();
        }
    }

    let mut lines = Vec::new();
    lines.push(format!("Slides: {}", pres.slides.len()));
    lines.push(format!("Shapes: {}", total_shapes));
    lines.push(format!("Paragraphs: {}", total_paragraphs));
    lines.push(format!("Characters: {}", total_chars));

    let morph_count: usize = pres.slides.iter().filter(|s| s.has_morph).count();
    if morph_count > 0 {
        lines.push(format!("Morph transitions: {}", morph_count));
    }

    Ok(lines.join("\n"))
}

/// ViewAsTextJson: JSON output of slide text.
pub fn view_as_text_json(
    package: &oxml::OxmlPackage,
    _opts: &ViewOptions,
) -> Result<serde_json::Value, HandlerError> {
    let pres = build_presentation(package)?;
    let mut slide_data = Vec::new();

    for slide in &pres.slides {
        let mut shape_texts = Vec::new();
        for (si, shape) in slide.shapes.iter().enumerate() {
            if !shape.text.is_empty() {
                shape_texts.push(serde_json::json!({
                    "path": format!("/slide[{}]/shape[{}]", slide.index, si + 1),
                    "placeholder": shape.placeholder_type,
                    "text": shape.text,
                }));
            }
        }
        slide_data.push(serde_json::json!({
            "path": format!("/slide[{}]", slide.index),
            "shapes": shape_texts,
        }));
    }

    Ok(serde_json::json!({
        "slides": slide_data,
    }))
}

/// ViewAsOutlineJson: JSON output of slide structure.
pub fn view_as_outline_json(
    package: &oxml::OxmlPackage,
) -> Result<serde_json::Value, HandlerError> {
    let pres = build_presentation(package)?;
    let mut slide_data = Vec::new();

    for slide in &pres.slides {
        let mut shapes = Vec::new();
        for (si, shape) in slide.shapes.iter().enumerate() {
            shapes.push(serde_json::json!({
                "path": format!("/slide[{}]/shape[{}]", slide.index, si + 1),
                "type": shape.placeholder_type.as_deref().unwrap_or("shape"),
                "name": shape.name,
                "id": shape.id,
                "paragraph_count": shape.paragraphs.len(),
                "text_preview": truncate_str(&shape.text, 80),
            }));
        }
        slide_data.push(serde_json::json!({
            "path": format!("/slide[{}]", slide.index),
            "slide_id": slide.slide_id,
            "shape_count": slide.shapes.len(),
            "has_morph": slide.has_morph,
            "morph_candidates": slide.morph_candidates,
            "shapes": shapes,
        }));
    }

    Ok(serde_json::json!({
        "slides": slide_data,
    }))
}

/// ViewAsStatsJson: JSON output of statistics.
pub fn view_as_stats_json(package: &oxml::OxmlPackage) -> Result<serde_json::Value, HandlerError> {
    let pres = build_presentation(package)?;
    let mut total_shapes = 0;
    let mut total_paragraphs = 0;
    let mut total_chars = 0;

    for slide in &pres.slides {
        total_shapes += slide.shapes.len();
        for shape in &slide.shapes {
            total_paragraphs += shape.paragraphs.len();
            total_chars += shape.text.len();
        }
    }

    Ok(serde_json::json!({
        "slides": pres.slides.len(),
        "shapes": total_shapes,
        "paragraphs": total_paragraphs,
        "characters": total_chars,
        "morph_transitions": pres.slides.iter().filter(|s| s.has_morph).count(),
    }))
}

/// Get: retrieve a node at the given path.
pub fn get_node(
    package: &oxml::OxmlPackage,
    path: &str,
    depth: usize,
) -> Result<DocumentNode, HandlerError> {
    let pres = build_presentation(package)?;
    let segments = crate::navigation::parse_path(path);

    if segments.is_empty() {
        // Root node — show all slides
        let mut root = DocumentNode::new("/", "presentation");
        let mut slide_nodes = Vec::new();
        for slide in &pres.slides {
            let slide_node = make_slide_node(slide, depth > 0);
            slide_nodes.push(slide_node);
        }
        root = root.with_children(slide_nodes);
        root.text = Some(format!("{} slides", pres.slides.len()));
        return Ok(root);
    }

    // First segment must be "slide[N]"
    let first = &segments[0];
    if first.name != "slide" {
        return Err(HandlerError::InvalidPath(format!(
            "expected 'slide' segment, got '{}'",
            first.name
        )));
    }
    let slide_idx = first.index.unwrap_or(1);
    let slide = find_slide(&pres, slide_idx)
        .ok_or_else(|| HandlerError::PathNotFound(format!("/slide[{}]", slide_idx)))?;

    if segments.len() == 1 {
        // Just the slide node
        let node = make_slide_node(slide, depth > 0);
        return Ok(node);
    }

    // Second segment: "shape[M]"
    let second = &segments[1];
    if second.name != "shape" {
        return Err(HandlerError::InvalidPath(format!(
            "expected 'shape' segment, got '{}'",
            second.name
        )));
    }
    let shape_idx = second.index.unwrap_or(1);
    let shape = find_shape(slide, shape_idx).ok_or_else(|| {
        HandlerError::PathNotFound(format!("/slide[{}]/shape[{}]", slide_idx, shape_idx))
    })?;

    if segments.len() == 2 {
        // Shape node
        let node = make_shape_node(slide_idx, shape_idx, shape, depth > 0);
        return Ok(node);
    }

    // Third segment: "paragraph[K]"
    let third = &segments[2];
    if third.name != "paragraph" {
        return Err(HandlerError::InvalidPath(format!(
            "expected 'paragraph' segment, got '{}'",
            third.name
        )));
    }
    let para_idx = third.index.unwrap_or(1);
    let para = find_paragraph(shape, para_idx).ok_or_else(|| {
        HandlerError::PathNotFound(format!(
            "/slide[{}]/shape[{}]/paragraph[{}]",
            slide_idx, shape_idx, para_idx
        ))
    })?;

    let node = DocumentNode::new(
        &format!(
            "/slide[{}]/shape[{}]/paragraph[{}]",
            slide_idx, shape_idx, para_idx
        ),
        "paragraph",
    )
    .with_text(&para.text);
    Ok(node)
}

/// Set: modify shape text at the given path.
pub fn set_shape_text(
    package: &mut oxml::OxmlPackage,
    path: &str,
    properties: &std::collections::HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    // Find/replace short-circuit: when `find` is present, scan <a:t>...</a:t>
    // text runs in the targeted slide (or all slides when path is "/").
    if properties.contains_key("find") {
        return apply_pptx_find_replace(package, path, properties);
    }

    let segments = crate::navigation::parse_path(path);

    // We need at least /slide[N]/shape[M]
    if segments.len() < 2 {
        return Err(HandlerError::InvalidPath(
            "path must be /slide[N]/shape[M] or deeper".to_string(),
        ));
    }

    let slide_idx = segments[0].index.unwrap_or(1);
    let shape_idx = segments[1].index.unwrap_or(1);
    let target_type = segments[1].name.to_ascii_lowercase();

    // First, build the presentation to find the slide part path
    let pres = build_presentation(package)?;
    let slide = find_slide(&pres, slide_idx)
        .ok_or_else(|| HandlerError::PathNotFound(format!("/slide[{}]", slide_idx)))?;

    // Read the slide XML
    let slide_xml = package
        .read_part_xml(&slide.part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    if target_type == "group" && segments.len() == 2 {
        let (modified_xml, unsupported) =
            apply_group_properties(&slide_xml, shape_idx, properties)?;
        package
            .write_part_xml(&slide.part_path, &modified_xml)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        return Ok(unsupported);
    }

    let mut modified_xml = slide_xml.clone();
    let mut unsupported = Vec::new();

    // 1. Handle text property (replace shape text)
    if let Some(new_text) = properties.get("text") {
        modified_xml = replace_shape_text_in_xml(&modified_xml, shape_idx, new_text)?;
    }

    // 2. Handle shape-level properties (position, size, rotation, name)
    let shape_props: std::collections::HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| {
            matches!(
                k.as_str(),
                "x" | "left"
                    | "y"
                    | "top"
                    | "width"
                    | "w"
                    | "cx"
                    | "height"
                    | "h"
                    | "cy"
                    | "rotation"
                    | "name"
                    | "id"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !shape_props.is_empty() {
        modified_xml = apply_shape_geometry(&modified_xml, shape_idx, &shape_props)?;
    }

    // 3. Handle fill/color properties (background color of shape)
    let fill_props: std::collections::HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| {
            matches!(
                k.as_str(),
                "fill" | "fillColor" | "bg" | "bgColor" | "background"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !fill_props.is_empty() {
        modified_xml = apply_shape_fill(&modified_xml, shape_idx, &fill_props)?;
    }

    // 4. Handle text formatting properties (font, size, color, bold, italic)
    let text_fmt_props: std::collections::HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| {
            matches!(
                k.as_str(),
                "bold"
                    | "b"
                    | "italic"
                    | "i"
                    | "underline"
                    | "u"
                    | "strike"
                    | "strikeout"
                    | "font"
                    | "fontName"
                    | "font.name"
                    | "size"
                    | "fontSize"
                    | "font.size"
                    | "color"
                    | "fontColor"
                    | "font.color"
                    | "alignment"
                    | "align"
                    | "jc"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !text_fmt_props.is_empty() {
        modified_xml = apply_text_format(&modified_xml, shape_idx, &text_fmt_props)?;
    }

    // 5. Handle line/border properties
    let line_props: std::collections::HashMap<String, String> = properties
        .iter()
        .filter(|(k, _)| {
            matches!(
                k.as_str(),
                "line" | "border" | "borderColor" | "borderWidth" | "lineColor" | "lineWidth"
            )
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !line_props.is_empty() {
        modified_xml = apply_shape_line(&modified_xml, shape_idx, &line_props)?;
    }

    // Track recognized keys
    let recognized = [
        "text",
        "x",
        "left",
        "y",
        "top",
        "width",
        "w",
        "cx",
        "height",
        "h",
        "cy",
        "rotation",
        "name",
        "id",
        "fill",
        "fillColor",
        "bg",
        "bgColor",
        "background",
        "bold",
        "b",
        "italic",
        "i",
        "underline",
        "u",
        "strike",
        "strikeout",
        "font",
        "fontName",
        "font.name",
        "size",
        "fontSize",
        "font.size",
        "color",
        "fontColor",
        "font.color",
        "alignment",
        "align",
        "jc",
        "line",
        "border",
        "borderColor",
        "borderWidth",
        "lineColor",
        "lineWidth",
        "range_paths",
    ];
    for key in properties.keys() {
        if !recognized.contains(&key.as_str()) {
            unsupported.push(key.clone());
        }
    }

    // Write back the modified XML
    package
        .write_part_xml(&slide.part_path, &modified_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(unsupported)
}

/// Apply position/size/rotation changes to a shape's <p:spPr> element.
fn apply_shape_geometry(
    xml: &str,
    shape_idx: usize,
    props: &std::collections::HashMap<String, String>,
) -> Result<String, HandlerError> {
    // Build a new <p:xfrm> element with updated offsets/extents
    let mut xfrm_xml = String::from("<p:xfrm");
    if let Some(rot) = props.get("rotation") {
        let rot_deg: f64 = rot.parse().unwrap_or(0.0);
        let rot_units = (rot_deg * 60000.0) as i64;
        xfrm_xml.push_str(&format!(" rot=\"{}\"", rot_units));
    }
    xfrm_xml.push('>');

    let mut off_x = "0".to_string();
    let mut off_y = "0".to_string();
    let mut ext_cx = "914400".to_string(); // 1 inch default
    let mut ext_cy = "914400".to_string();

    if let Some(v) = props.get("x").or_else(|| props.get("left")) {
        off_x = pptx_unit_to_emu(v);
    }
    if let Some(v) = props.get("y").or_else(|| props.get("top")) {
        off_y = pptx_unit_to_emu(v);
    }
    if let Some(v) = props
        .get("width")
        .or_else(|| props.get("w"))
        .or_else(|| props.get("cx"))
    {
        ext_cx = pptx_unit_to_emu(v);
    }
    if let Some(v) = props
        .get("height")
        .or_else(|| props.get("h"))
        .or_else(|| props.get("cy"))
    {
        ext_cy = pptx_unit_to_emu(v);
    }
    let _ = (&mut off_x, &mut off_y, &mut ext_cx, &mut ext_cy); // mark used

    xfrm_xml.push_str(&format!("<a:off x=\"{}\" y=\"{}\"/>", off_x, off_y));
    xfrm_xml.push_str(&format!("<a:ext cx=\"{}\" cy=\"{}\"/>", ext_cx, ext_cy));
    xfrm_xml.push_str("</p:xfrm>");

    // Replace or insert <p:xfrm> inside the Nth shape's <p:spPr>
    // For now we use a regex-style replacement: find the shape's spPr and swap xfrm.
    replace_xfrm_in_nth_shape(xml, shape_idx, &xfrm_xml)
}

/// Apply position and size properties to the Nth top-level group shape.
///
/// Group children use `chOff`/`chExt` as their logical coordinate space. We
/// preserve that baseline while changing the outer `off`/`ext`, so changing a
/// single axis scales children only on that axis. `keepAspect=true` opts into
/// proportional scaling of the other axis.
fn apply_group_properties(
    xml: &str,
    group_idx: usize,
    props: &std::collections::HashMap<String, String>,
) -> Result<(String, Vec<String>), HandlerError> {
    if group_idx == 0 {
        return Err(HandlerError::InvalidPath(
            "group indices are 1-based".to_string(),
        ));
    }

    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid slide XML: {}", e)))?;
    let shape_tree = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "spTree")
        .ok_or_else(|| HandlerError::OperationFailed("slide has no shape tree".to_string()))?;
    let group = shape_tree
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "grpSp")
        .nth(group_idx - 1)
        .ok_or_else(|| HandlerError::PathNotFound(format!("group {}", group_idx)))?;
    let group_properties = group
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "grpSpPr")
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!("group {} has no grpSpPr", group_idx))
        })?;
    let transform = group_properties
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "xfrm")
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!("group {} has no transform", group_idx))
        })?;
    let offset = transform
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "off")
        .ok_or_else(|| HandlerError::OperationFailed("group transform has no off".to_string()))?;
    let extents = transform
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "ext")
        .ok_or_else(|| HandlerError::OperationFailed("group transform has no ext".to_string()))?;

    let pre_x = parse_emu_attribute(&offset, "x")?;
    let pre_y = parse_emu_attribute(&offset, "y")?;
    let pre_width = parse_emu_attribute(&extents, "cx")?;
    let pre_height = parse_emu_attribute(&extents, "cy")?;
    if pre_width <= 0 || pre_height <= 0 {
        return Err(HandlerError::InvalidArgument(
            "invalid group size: width and height must be positive".to_string(),
        ));
    }

    let x_value = property_value(props, &["x", "left"]);
    let y_value = property_value(props, &["y", "top"]);
    let width_value = property_value(props, &["width"]);
    let height_value = property_value(props, &["height"]);
    let has_width = width_value.is_some();
    let has_height = height_value.is_some();
    let keep_aspect = property_value(props, &["keepAspect"]).is_some_and(is_truthy_property);

    let new_x = x_value
        .map(|value| parse_group_length_emu(value, "x"))
        .transpose()?
        .unwrap_or(pre_x);
    let new_y = y_value
        .map(|value| parse_group_length_emu(value, "y"))
        .transpose()?
        .unwrap_or(pre_y);
    let mut new_width = width_value
        .map(|value| parse_group_length_emu(value, "width"))
        .transpose()?
        .unwrap_or(pre_width);
    let mut new_height = height_value
        .map(|value| parse_group_length_emu(value, "height"))
        .transpose()?
        .unwrap_or(pre_height);

    if keep_aspect && has_width && !has_height {
        new_height = (pre_height as f64 * (new_width as f64 / pre_width as f64)).round() as i64;
    } else if keep_aspect && has_height && !has_width {
        new_width = (pre_width as f64 * (new_height as f64 / pre_height as f64)).round() as i64;
    }
    if new_width <= 0 || new_height <= 0 {
        return Err(HandlerError::InvalidArgument(
            "invalid group size: width and height must be positive".to_string(),
        ));
    }

    let mut replacements = Vec::new();
    if x_value.is_some() {
        replacements.push((
            attribute_value_range(xml, offset.range(), "x")?,
            new_x.to_string(),
        ));
    }
    if y_value.is_some() {
        replacements.push((
            attribute_value_range(xml, offset.range(), "y")?,
            new_y.to_string(),
        ));
    }
    if has_width || (keep_aspect && has_height) {
        replacements.push((
            attribute_value_range(xml, extents.range(), "cx")?,
            new_width.to_string(),
        ));
    }
    if has_height || (keep_aspect && has_width) {
        replacements.push((
            attribute_value_range(xml, extents.range(), "cy")?,
            new_height.to_string(),
        ));
    }

    // Snapshot missing child-coordinate baselines before changing the outer
    // transform, matching the C# handler's first-resize behavior.
    let child_offset = transform
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "chOff");
    let child_extents = transform
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "chExt");
    let need_child_offset = (x_value.is_some() || y_value.is_some()) && child_offset.is_none();
    let need_child_extents = (has_width || has_height) && child_extents.is_none();
    if need_child_offset || need_child_extents {
        let mut insertion = String::new();
        if need_child_offset {
            insertion.push_str(&format!("<a:chOff x=\"{}\" y=\"{}\"/>", pre_x, pre_y));
        }
        if need_child_extents {
            insertion.push_str(&format!(
                "<a:chExt cx=\"{}\" cy=\"{}\"/>",
                pre_width, pre_height
            ));
        }
        let insert_at = if need_child_offset {
            child_extents
                .map(|node| node.range().start)
                .unwrap_or_else(|| transform_closing_tag_start(xml, transform.range()))
        } else {
            transform_closing_tag_start(xml, transform.range())
        };
        replacements.push((insert_at..insert_at, insertion));
    }

    if let Some(name) = property_value(props, &["name"]) {
        let non_visual = group
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "nvGrpSpPr")
            .and_then(|node| {
                node.descendants()
                    .find(|child| child.is_element() && child.tag_name().name() == "cNvPr")
            })
            .ok_or_else(|| {
                HandlerError::OperationFailed("group has no non-visual properties".to_string())
            })?;
        replacements.push((
            attribute_value_range(xml, non_visual.range(), "name")?,
            escape_xml_attribute(name),
        ));
    }

    if has_width || has_height {
        let font_ratio =
            (new_width as f64 / pre_width as f64).min(new_height as f64 / pre_height as f64);
        if (font_ratio - 1.0).abs() > 1e-6 {
            for font_properties in group.descendants().filter(|node| {
                node.is_element()
                    && matches!(node.tag_name().name(), "rPr" | "endParaRPr" | "defRPr")
                    && node.attribute("sz").is_some()
            }) {
                let old_size = font_properties
                    .attribute("sz")
                    .and_then(|value| value.parse::<i64>().ok())
                    .ok_or_else(|| {
                        HandlerError::OperationFailed(
                            "group contains an invalid font size".to_string(),
                        )
                    })?;
                let new_size = (old_size as f64 * font_ratio).round().max(100.0) as i64;
                replacements.push((
                    attribute_value_range(xml, font_properties.range(), "sz")?,
                    new_size.to_string(),
                ));
            }
        }
    }

    let recognized = [
        "x",
        "left",
        "y",
        "top",
        "width",
        "height",
        "keepAspect",
        "name",
    ];
    let unsupported = props
        .keys()
        .filter(|key| {
            !recognized
                .iter()
                .any(|recognized| key.eq_ignore_ascii_case(recognized))
        })
        .cloned()
        .collect();

    replacements.sort_by(|left, right| right.0.start.cmp(&left.0.start));
    let mut result = xml.to_string();
    for (range, replacement) in replacements {
        result.replace_range(range, &replacement);
    }
    Ok((result, unsupported))
}

fn property_value<'a>(
    props: &'a std::collections::HashMap<String, String>,
    names: &[&str],
) -> Option<&'a str> {
    props.iter().find_map(|(key, value)| {
        names
            .iter()
            .any(|name| key.eq_ignore_ascii_case(name))
            .then_some(value.as_str())
    })
}

fn is_truthy_property(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

fn parse_group_length_emu(value: &str, property: &str) -> Result<i64, HandlerError> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let units = [
        ("px", 9_525.0),
        ("in", 914_400.0),
        ("cm", 360_000.0),
        ("mm", 36_000.0),
        ("pt", 12_700.0),
    ];
    let parsed = if let Some((number, multiplier)) =
        units.iter().find_map(|(suffix, multiplier)| {
            lower
                .strip_suffix(suffix)
                .map(|number| (number, multiplier))
        }) {
        number
            .trim()
            .parse::<f64>()
            .map(|value| (value * multiplier).round() as i64)
            .map_err(|_| {
                HandlerError::InvalidArgument(format!(
                    "invalid {} '{}': expected a length such as 12cm or an EMU integer",
                    property, value
                ))
            })?
    } else {
        trimmed.parse::<i64>().map_err(|_| {
            HandlerError::InvalidArgument(format!(
                "invalid {} '{}': expected a length such as 12cm or an EMU integer",
                property, value
            ))
        })?
    };
    if matches!(property, "width" | "height") && parsed <= 0 {
        return Err(HandlerError::InvalidArgument(format!(
            "invalid {} '{}': size must be positive",
            property, value
        )));
    }
    Ok(parsed)
}

fn parse_emu_attribute(node: &roxmltree::Node<'_, '_>, name: &str) -> Result<i64, HandlerError> {
    node.attribute(name)
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!(
                "group transform element '{}' has no {} attribute",
                node.tag_name().name(),
                name
            ))
        })?
        .parse::<i64>()
        .map_err(|_| {
            HandlerError::OperationFailed(format!(
                "group transform element '{}' has invalid {} attribute",
                node.tag_name().name(),
                name
            ))
        })
}

fn attribute_value_range(
    xml: &str,
    node_range: std::ops::Range<usize>,
    attribute_name: &str,
) -> Result<std::ops::Range<usize>, HandlerError> {
    let opening = &xml[node_range.clone()];
    let opening_end = opening.find('>').ok_or_else(|| {
        HandlerError::OperationFailed("malformed group XML opening tag".to_string())
    })?;
    let opening = &opening[..opening_end];
    let mut search_from = 0;
    while let Some(relative) = opening[search_from..].find(attribute_name) {
        let name_start = search_from + relative;
        let before_ok = name_start > 0 && opening.as_bytes()[name_start - 1].is_ascii_whitespace();
        let mut cursor = name_start + attribute_name.len();
        while opening
            .as_bytes()
            .get(cursor)
            .is_some_and(u8::is_ascii_whitespace)
        {
            cursor += 1;
        }
        if before_ok && opening.as_bytes().get(cursor) == Some(&b'=') {
            cursor += 1;
            while opening
                .as_bytes()
                .get(cursor)
                .is_some_and(u8::is_ascii_whitespace)
            {
                cursor += 1;
            }
            let quote = *opening.as_bytes().get(cursor).ok_or_else(|| {
                HandlerError::OperationFailed("malformed group XML attribute".to_string())
            })?;
            if quote != b'\'' && quote != b'"' {
                return Err(HandlerError::OperationFailed(
                    "malformed group XML attribute quoting".to_string(),
                ));
            }
            let value_start = cursor + 1;
            let value_end = opening.as_bytes()[value_start..]
                .iter()
                .position(|byte| *byte == quote)
                .map(|position| value_start + position)
                .ok_or_else(|| {
                    HandlerError::OperationFailed("unterminated group XML attribute".to_string())
                })?;
            return Ok((node_range.start + value_start)..(node_range.start + value_end));
        }
        search_from = name_start + attribute_name.len();
    }
    Err(HandlerError::OperationFailed(format!(
        "group XML has no {} attribute",
        attribute_name
    )))
}

fn transform_closing_tag_start(xml: &str, range: std::ops::Range<usize>) -> usize {
    let transform_xml = &xml[range.clone()];
    range.start + transform_xml.rfind("</").unwrap_or(transform_xml.len())
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Convert various units (px, in, cm, mm, pt) to EMU (English Metric Units).
/// 1 inch = 914400 EMU; 1 cm = 360000 EMU; 1 pt = 12700 EMU; 1 px ≈ 9525 EMU.
fn pptx_unit_to_emu(value: &str) -> String {
    let v = value.trim();
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
    // Assume EMU if just a number
    v.to_string()
}

/// Find the Nth <p:sp> in the slide XML, then replace its <p:xfrm> element.
fn replace_xfrm_in_nth_shape(
    xml: &str,
    shape_idx: usize,
    new_xfrm: &str,
) -> Result<String, HandlerError> {
    let mut pos = 0;
    let mut count = 0;

    while let Some(found) = xml[pos..].find("<p:sp") {
        let abs_pos = pos + found;
        // Verify it's actually a sp element start
        let next_char = xml.get(abs_pos + 5..abs_pos + 6).unwrap_or("");
        if next_char == ">" || next_char == " " {
            count += 1;
            if count == shape_idx {
                // Found the target shape; find its end (</p:sp>)
                let sp_end = xml[abs_pos..]
                    .find("</p:sp>")
                    .ok_or_else(|| HandlerError::OperationFailed("no </p:sp> found".to_string()))?
                    + abs_pos;

                // Look for existing <p:spPr>...</p:spPr> within this shape
                let sp_slice = &xml[abs_pos..sp_end];
                if let Some(sppr_start) = sp_slice.find("<p:spPr") {
                    let sppr_abs = abs_pos + sppr_start;
                    let sppr_end_rel = sp_slice[sppr_start..].find(">").ok_or_else(|| {
                        HandlerError::OperationFailed("malformed <p:spPr>".to_string())
                    })?;
                    let sppr_inner_start = sppr_abs + sppr_end_rel + 1;

                    // Find existing xfrm or insert
                    let sppr_close = sp_slice[sppr_start..]
                        .find("</p:spPr>")
                        .or_else(|| sp_slice[sppr_start..].find("/>"));

                    if let Some(close_rel) = sppr_close {
                        let sppr_close_abs = abs_pos + sppr_start + close_rel;
                        let sppr_inner = &xml[sppr_inner_start..sppr_close_abs];

                        if let Some(xfrm_start) = sppr_inner.find("<p:xfrm") {
                            // Replace existing xfrm
                            let xfrm_end_rel =
                                sppr_inner[xfrm_start..].find("</p:xfrm>").ok_or_else(|| {
                                    HandlerError::OperationFailed("malformed <p:xfrm>".to_string())
                                })?;
                            let xfrm_end_abs =
                                sppr_inner_start + xfrm_start + xfrm_end_rel + "</p:xfrm>".len();

                            let mut result = xml[..sppr_inner_start + xfrm_start].to_string();
                            result.push_str(new_xfrm);
                            result.push_str(&xml[xfrm_end_abs..]);
                            return Ok(result);
                        } else {
                            // Insert new xfrm at start of spPr inner
                            let mut result = xml[..sppr_inner_start].to_string();
                            result.push_str(new_xfrm);
                            result.push_str(&xml[sppr_inner_start..]);
                            return Ok(result);
                        }
                    }
                }
            }
        }
        pos = abs_pos + 5;
    }

    // Fallback: no shape found, leave XML unchanged
    Ok(xml.to_string())
}

/// Apply fill color to a shape.
fn apply_shape_fill(
    xml: &str,
    _shape_idx: usize,
    props: &std::collections::HashMap<String, String>,
) -> Result<String, HandlerError> {
    let color = props
        .get("fill")
        .or_else(|| props.get("fillColor"))
        .or_else(|| props.get("bg"))
        .or_else(|| props.get("bgColor"))
        .or_else(|| props.get("background"));

    if let Some(color) = color {
        let hex = color.strip_prefix('#').unwrap_or(color);
        // Replace any existing <p:solidFill> in the slide with the new color.
        // For simplicity we just prepend a solidFill to the slide's spPr if shape_idx matches.
        // Full implementation would target only the specific shape's spPr.
        if let Some(solid_start) = xml.find("<a:srgbClr") {
            // Find the val="..." attribute within the first srgbClr
            let val_start = xml[solid_start..]
                .find("val=\"")
                .map(|p| solid_start + p + 5)
                .ok_or_else(|| HandlerError::OperationFailed("malformed srgbClr".to_string()))?;
            let val_end = xml[val_start..]
                .find('"')
                .map(|p| val_start + p)
                .ok_or_else(|| {
                    HandlerError::OperationFailed("malformed srgbClr val".to_string())
                })?;

            let mut result = xml[..val_start].to_string();
            result.push_str(hex);
            result.push_str(&xml[val_end..]);
            return Ok(result);
        }
    }
    Ok(xml.to_string())
}

/// Apply text formatting (bold, italic, font, size, color, alignment) to text runs.
fn apply_text_format(
    xml: &str,
    _shape_idx: usize,
    props: &std::collections::HashMap<String, String>,
) -> Result<String, HandlerError> {
    let mut result = xml.to_string();

    // Apply bold/italic by modifying <a:rPr> elements
    if let Some(val) = props.get("bold").or_else(|| props.get("b")) {
        let b_val = if val == "true" || val == "1" {
            "1"
        } else {
            "0"
        };
        // Insert or update b="..." in <a:rPr> tags
        result = inject_attr_in_tag(&result, "a:rPr", "b", b_val);
    }
    if let Some(val) = props.get("italic").or_else(|| props.get("i")) {
        let i_val = if val == "true" || val == "1" {
            "1"
        } else {
            "0"
        };
        result = inject_attr_in_tag(&result, "a:rPr", "i", i_val);
    }
    if let Some(size) = props.get("size").or_else(|| props.get("fontSize")) {
        let pt: f64 = size.parse().unwrap_or(18.0);
        let hundredths = (pt * 100.0) as i64;
        result = inject_attr_in_tag(&result, "a:rPr", "sz", &hundredths.to_string());
    }
    if let Some(color) = props.get("color").or_else(|| props.get("fontColor")) {
        let hex = color.strip_prefix('#').unwrap_or(color);
        // Inject solidFill into rPr — for simplicity, replace existing srgbClr
        if let Some(start) = result.find("<a:rPr") {
            let end_rel = result[start..]
                .find('>')
                .ok_or_else(|| HandlerError::OperationFailed("malformed rPr".to_string()))?;
            let end_abs = start + end_rel + 1;
            let mut new_result = result[..end_abs].to_string();
            new_result.push_str(&format!(
                "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
                hex
            ));
            new_result.push_str(&result[end_abs..]);
            result = new_result;
        }
    }

    // Apply alignment to paragraphs (<a:pPr> inside <a:p>)
    if let Some(align) = props
        .get("alignment")
        .or_else(|| props.get("align"))
        .or_else(|| props.get("jc"))
    {
        let algn_val = match align.as_str() {
            "left" | "l" => "l",
            "center" | "c" | "centre" => "ctr",
            "right" | "r" => "r",
            "justify" | "justified" | "j" => "just",
            other => other,
        };
        result = inject_attr_in_tag(&result, "a:pPr", "algn", algn_val);
    }

    Ok(result)
}

/// Inject an attribute into all opening tags matching `tag_name` in the XML.
fn inject_attr_in_tag(xml: &str, tag_name: &str, attr_name: &str, attr_val: &str) -> String {
    let mut result = xml.to_string();
    let pattern = format!("<{}", tag_name);

    // For simplicity, only modify the first match per call.
    // (A future PR could iterate properly if needed.)
    if let Some(start) = result.find(&pattern) {
        let after_pattern = start + pattern.len();
        // Find end of the tag (> or />)
        let tag_end = result[after_pattern..]
            .find('>')
            .map(|p| after_pattern + p)
            .unwrap_or(after_pattern);
        let tag_inner = &result[after_pattern..tag_end];

        // Check if attr already exists
        let attr_pattern = format!("{}=\"", attr_name);
        if tag_inner.contains(&attr_pattern) {
            // Update existing
            if let Some(attr_start) = result[after_pattern..tag_end].find(&attr_pattern) {
                let attr_abs = after_pattern + attr_start + attr_pattern.len();
                let val_end = result[attr_abs..]
                    .find('"')
                    .map(|p| attr_abs + p)
                    .unwrap_or(attr_abs);

                let mut new_result = result[..attr_abs].to_string();
                new_result.push_str(attr_val);
                new_result.push_str(&result[val_end..]);
                result = new_result;
            }
        } else {
            // Insert new attribute just before tag_end (handle self-closing)
            let insert_pos = if result.as_bytes().get(tag_end - 1) == Some(&b'/') {
                tag_end - 1
            } else {
                tag_end
            };
            let mut new_result = result[..insert_pos].to_string();
            new_result.push_str(&format!(" {}=\"{}\"", attr_name, attr_val));
            new_result.push_str(&result[insert_pos..]);
            result = new_result;
        }
    }
    result
}

/// Apply line/border properties to a shape.
fn apply_shape_line(
    xml: &str,
    _shape_idx: usize,
    props: &std::collections::HashMap<String, String>,
) -> Result<String, HandlerError> {
    if let Some(color) = props
        .get("line")
        .or_else(|| props.get("lineColor"))
        .or_else(|| props.get("border"))
        .or_else(|| props.get("borderColor"))
    {
        let hex = color.strip_prefix('#').unwrap_or(color);
        let width_emu = props
            .get("lineWidth")
            .or_else(|| props.get("borderWidth"))
            .map(|v| pptx_unit_to_emu(v))
            .unwrap_or_else(|| "12700".to_string()); // 1pt default

        // Find <a:ln> or insert one in spPr
        if let Some(ln_start) = xml.find("<a:ln") {
            // Find </a:ln>
            let ln_close = xml[ln_start..]
                .find("</a:ln>")
                .map(|p| ln_start + p)
                .ok_or_else(|| HandlerError::OperationFailed("no </a:ln>".to_string()))?;

            let new_ln = format!(
                "<a:ln w=\"{}\"><a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill></a:ln>",
                width_emu, hex
            );
            let mut result = xml[..ln_start].to_string();
            result.push_str(&new_ln);
            // Skip past old ln (including end tag)
            result.push_str(&xml[ln_close + "</a:ln>".len()..]);
            return Ok(result);
        }
    }
    Ok(xml.to_string())
}

/// Replace text in the Nth shape of a slide XML document.
fn replace_shape_text_in_xml(
    xml: &str,
    shape_idx: usize,
    new_text: &str,
) -> Result<String, HandlerError> {
    // Parse the new text into paragraphs (split by newline)
    let new_paragraphs: Vec<&str> = new_text.split('\n').collect();

    // Use quick-xml Reader/Writer for a streaming rewrite approach
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false); // Preserve whitespace in XML
    let mut writer = quick_xml::Writer::new(Vec::new());

    let mut current_shape_count = 0;
    let mut in_target_shape = false;
    let mut in_tx_body = false;
    let mut skip_old_text = false;

    // State tracking for nesting
    let mut sp_depth = 0;
    let mut tx_body_depth = 0;

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                let local_name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();

                if local_name == "sp" {
                    if !in_target_shape {
                        current_shape_count += 1;
                        if current_shape_count == shape_idx {
                            in_target_shape = true;
                            sp_depth = 1;
                        }
                    } else {
                        sp_depth += 1;
                    }
                }

                if local_name == "txBody" && in_target_shape {
                    in_tx_body = true;
                    tx_body_depth = 1;
                    // Write the <p:txBody> start tag
                    writer
                        .write_event(quick_xml::events::Event::Start(e.clone()))
                        .ok();
                    // Write new paragraphs
                    for para_text in &new_paragraphs {
                        write_new_paragraph(&mut writer, para_text);
                    }
                    // Now skip the old content until </p:txBody>
                    skip_old_text = true;
                    continue;
                }

                if in_tx_body {
                    tx_body_depth += 1;
                    // Skip writing old content inside txBody
                    continue;
                }

                writer
                    .write_event(quick_xml::events::Event::Start(e.clone()))
                    .ok();
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let local_name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();

                if in_tx_body {
                    tx_body_depth -= 1;
                    if local_name == "txBody" && tx_body_depth == 0 {
                        // End of txBody — write the closing tag and stop skipping
                        writer
                            .write_event(quick_xml::events::Event::End(e.clone()))
                            .ok();
                        in_tx_body = false;
                        skip_old_text = false;
                        continue;
                    }
                    // Skip old content inside txBody
                    continue;
                }

                writer
                    .write_event(quick_xml::events::Event::End(e.clone()))
                    .ok();

                if local_name == "sp" && in_target_shape {
                    sp_depth -= 1;
                    if sp_depth == 0 {
                        in_target_shape = false;
                    }
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                if !in_tx_body {
                    writer
                        .write_event(quick_xml::events::Event::Empty(e.clone()))
                        .ok();
                }
            }
            Ok(quick_xml::events::Event::Text(e)) => {
                if !skip_old_text {
                    writer
                        .write_event(quick_xml::events::Event::Text(e.clone()))
                        .ok();
                }
            }
            Ok(quick_xml::events::Event::CData(e)) => {
                if !skip_old_text {
                    writer
                        .write_event(quick_xml::events::Event::CData(e.clone()))
                        .ok();
                }
            }
            Ok(quick_xml::events::Event::Decl(e)) => {
                writer
                    .write_event(quick_xml::events::Event::Decl(e.clone()))
                    .ok();
            }
            Ok(quick_xml::events::Event::Comment(e)) => {
                writer
                    .write_event(quick_xml::events::Event::Comment(e.clone()))
                    .ok();
            }
            Ok(quick_xml::events::Event::PI(e)) => {
                writer
                    .write_event(quick_xml::events::Event::PI(e.clone()))
                    .ok();
            }
            Ok(quick_xml::events::Event::DocType(e)) => {
                writer
                    .write_event(quick_xml::events::Event::DocType(e.clone()))
                    .ok();
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => {
                return Err(HandlerError::OperationFailed(format!(
                    "XML rewrite error: {}",
                    e
                )));
            }
        }
        buf.clear();
    }

    let result = writer.into_inner();
    Ok(String::from_utf8_lossy(&result).to_string())
}

/// Write a new <a:p> paragraph with <a:r>/<a:t> text.
fn write_new_paragraph(writer: &mut quick_xml::Writer<Vec<u8>>, text: &str) {
    // <a:p>
    let p_start = quick_xml::events::BytesStart::new("a:p");
    writer
        .write_event(quick_xml::events::Event::Start(p_start))
        .ok();

    // <a:r>
    let r_start = quick_xml::events::BytesStart::new("a:r");
    writer
        .write_event(quick_xml::events::Event::Start(r_start))
        .ok();

    // <a:t>
    let t_start = quick_xml::events::BytesStart::new("a:t");
    writer
        .write_event(quick_xml::events::Event::Start(t_start))
        .ok();

    // Text content
    let text_event = quick_xml::events::BytesText::new(text);
    writer
        .write_event(quick_xml::events::Event::Text(text_event))
        .ok();

    // </a:t>
    writer
        .write_event(quick_xml::events::Event::End(
            quick_xml::events::BytesEnd::new("a:t"),
        ))
        .ok();

    // </a:r>
    writer
        .write_event(quick_xml::events::Event::End(
            quick_xml::events::BytesEnd::new("a:r"),
        ))
        .ok();

    // </a:p>
    writer
        .write_event(quick_xml::events::Event::End(
            quick_xml::events::BytesEnd::new("a:p"),
        ))
        .ok();
}

/// Apply line range from ViewOptions to the output text.
fn apply_line_range(text: &str, opts: &ViewOptions) -> String {
    let all_lines: Vec<&str> = text.lines().collect();
    let total = all_lines.len();

    let start = opts
        .start_line
        .map(|l| if l > 0 { l - 1 } else { 0 })
        .unwrap_or(0);
    let end = opts
        .end_line
        .map(|l| if l > total { total } else { l })
        .unwrap_or(total);

    let max = opts.max_lines.unwrap_or(total);

    let selected: Vec<&str> = all_lines[start..end.min(start + max + total)]
        .iter()
        .take(max)
        .copied()
        .collect();

    selected.join("\n")
}

/// Truncate a string to max_chars characters (safe for UTF-8).
fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

/// Create a DocumentNode for a slide.
fn make_slide_node(slide: &Slide, include_children: bool) -> DocumentNode {
    let path = format!("/slide[{}]", slide.index);
    let mut node = DocumentNode::new(&path, "slide");

    // Build text preview from shapes
    let preview_parts: Vec<String> = slide
        .shapes
        .iter()
        .filter_map(|s| {
            if s.text.is_empty() {
                None
            } else {
                Some(s.text.clone())
            }
        })
        .collect();
    node.preview = Some(if preview_parts.is_empty() {
        "(empty slide)".to_string()
    } else {
        preview_parts.join(" | ")
    });

    if include_children {
        let mut shape_nodes = Vec::new();
        for (si, shape) in slide.shapes.iter().enumerate() {
            shape_nodes.push(make_shape_node(slide.index, si + 1, shape, false));
        }
        node = node.with_children(shape_nodes);
    } else {
        node.child_count = slide.shapes.len();
    }

    node
}

/// Create a DocumentNode for a shape.
fn make_shape_node(
    slide_idx: usize,
    shape_idx: usize,
    shape: &Shape,
    include_children: bool,
) -> DocumentNode {
    let path = format!("/slide[{}]/shape[{}]", slide_idx, shape_idx);
    let mut node = DocumentNode::new(&path, shape.placeholder_type.as_deref().unwrap_or("shape"));
    node.text = Some(shape.text.clone());
    node.preview = Some(truncate_str(&shape.text, 80));
    node = node.with_format("name", serde_json::Value::String(shape.name.clone()));
    node = node.with_format("id", serde_json::Value::String(shape.id.clone()));
    if let Some(pt) = &shape.placeholder_type {
        node = node.with_format("placeholderType", serde_json::Value::String(pt.clone()));
    }

    if include_children {
        let mut para_nodes = Vec::new();
        for (pi, para) in shape.paragraphs.iter().enumerate() {
            let para_path = format!(
                "/slide[{}]/shape[{}]/paragraph[{}]",
                slide_idx,
                shape_idx,
                pi + 1
            );
            para_nodes.push(DocumentNode::new(&para_path, "paragraph").with_text(&para.text));
        }
        node = node.with_children(para_nodes);
    } else {
        node.child_count = shape.paragraphs.len();
    }

    node
}

/// Detect issues in the presentation.
pub fn view_as_issues(
    package: &oxml::OxmlPackage,
    issue_type: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<DocumentIssue>, HandlerError> {
    let pres = build_presentation(package)?;
    let mut issues = Vec::new();

    // Check for missing slide parts
    for slide in &pres.slides {
        if !package.has_part(&slide.part_path) {
            issues.push(DocumentIssue {
                severity: IssueSeverity::Warning,
                issue_type: "missing-slide".to_string(),
                description: format!(
                    "Slide {} part '{}' is missing from the package",
                    slide.index, slide.part_path
                ),
                path: Some(format!("/slide[{}]", slide.index)),
            });
        }

        // The semantic shape model intentionally omits some visual OOXML
        // children, notably graphicFrame tables. Inspect the raw shape tree so
        // a table-only, picture-only, connector-only, or grouped slide is not
        // incorrectly reported as empty.
        let has_other_visual_content = package
            .read_part_xml(&slide.part_path)
            .ok()
            .is_some_and(|xml| slide_xml_has_visual_content(&xml));
        if slide.shapes.is_empty() && !has_other_visual_content {
            issues.push(DocumentIssue {
                severity: IssueSeverity::Info,
                issue_type: "empty-slide".to_string(),
                description: format!("Slide {} has no visual content", slide.index),
                path: Some(format!("/slide[{}]", slide.index)),
            });
        }

        // Check for shapes without IDs
        for (si, shape) in slide.shapes.iter().enumerate() {
            if shape.id.is_empty() {
                issues.push(DocumentIssue {
                    severity: IssueSeverity::Warning,
                    issue_type: "missing-id".to_string(),
                    description: format!(
                        "Shape {} on slide {} has no ID attribute",
                        si + 1,
                        slide.index
                    ),
                    path: Some(format!("/slide[{}]/shape[{}]", slide.index, si + 1)),
                });
            }
        }
    }

    // Filter by issue type if specified
    if let Some(filter_type) = issue_type {
        issues.retain(|i| i.issue_type == filter_type);
    }

    // Apply limit
    if let Some(max) = limit {
        issues.truncate(max);
    }

    Ok(issues)
}

fn slide_xml_has_visual_content(xml: &str) -> bool {
    let Ok(document) = roxmltree::Document::parse(xml) else {
        return false;
    };
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "spTree")
        .is_some_and(|shape_tree| {
            shape_tree.children().any(|node| {
                node.is_element()
                    && matches!(
                        node.tag_name().name(),
                        "sp" | "pic" | "graphicFrame" | "grpSp" | "cxnSp" | "contentPart"
                    )
            })
        })
}

/// Validate the presentation structure.
pub fn validate(package: &oxml::OxmlPackage) -> Result<Vec<ValidationError>, HandlerError> {
    let mut errors = Vec::new();

    // Check for required parts
    if !package.has_part("ppt/presentation.xml") {
        errors.push(ValidationError {
            error_type: "missing-part".to_string(),
            description: "required presentation part".to_string(),
            path: Some("ppt/presentation.xml".to_string()),
            part: Some("ppt/presentation.xml".to_string()),
        });
    }

    // Check that presentation.xml has a valid sldIdLst
    let pres_xml = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    if !pres_xml.contains("<p:sldIdLst") && !pres_xml.contains("<sldIdLst") {
        errors.push(ValidationError {
            error_type: "structure".to_string(),
            description: "no <sldIdLst> element found".to_string(),
            path: Some("ppt/presentation.xml".to_string()),
            part: Some("ppt/presentation.xml".to_string()),
        });
    }

    // Check that each referenced slide part exists
    let pres = build_presentation(package)?;
    for slide in &pres.slides {
        if !package.has_part(&slide.part_path) {
            errors.push(ValidationError {
                error_type: "missing-part".to_string(),
                description: format!("slide {} part is missing", slide.index),
                path: Some(format!("/slide[{}]", slide.index)),
                part: Some(slide.part_path.clone()),
            });
        }
    }

    Ok(errors)
}

enum PptxParaElement {
    Run {
        text: String,
        r_pr_xml: Option<String>,
    },
    Break {
        raw_xml: String,
    },
    Other {
        raw_xml: String,
    },
}

struct PptxPara {
    p_pr_xml: Option<String>,
    elements: Vec<PptxParaElement>,
}

pub fn apply_pptx_range_highlights(
    package: &mut oxml::OxmlPackage,
    properties: &std::collections::HashMap<String, String>,
    segments: &[handler_common::PathRangeSegment],
) -> Result<Vec<String>, HandlerError> {
    let mut unsupported = Vec::new();

    let mut format_props = properties.clone();
    format_props.remove("range_paths");
    if !format_props.contains_key("bgColor")
        && !format_props.contains_key("highlight")
        && !format_props.contains_key("bg")
    {
        format_props.insert("highlight".to_string(), "yellow".to_string());
    }

    // Group segments by slide number
    let mut slide_segs: std::collections::HashMap<usize, Vec<&handler_common::PathRangeSegment>> =
        std::collections::HashMap::new();
    for seg in segments {
        let slide_num = parse_slide_num_from_full_path(&seg.path)?;
        slide_segs.entry(slide_num).or_default().push(seg);
    }

    for (slide_num, segs) in slide_segs {
        let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;
        let slide_xml = package
            .read_part_xml(&slide_path)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

        let doc = roxmltree::Document::parse(&slide_xml).map_err(|e| {
            HandlerError::OperationFailed(format!(
                "roxmltree parse error on slide {}: {}",
                slide_num, e
            ))
        })?;

        let mut replacements = Vec::new();

        let sp_tree = doc.descendants().find(|n| {
            n.has_tag_name((crate::dom_types::NS_P, "spTree")) || n.has_tag_name("spTree")
        });

        if let Some(tree) = sp_tree {
            let mut sp_idx = 0;
            for child in tree.children() {
                if child.has_tag_name((crate::dom_types::NS_P, "sp")) || child.has_tag_name("sp") {
                    sp_idx += 1;

                    for seg in &segs {
                        let target_shape_idx = parse_shape_idx(&seg.path)?;
                        if target_shape_idx == sp_idx {
                            if let Some(tx_body) = child.children().find(|n| {
                                n.has_tag_name((crate::dom_types::NS_P, "txBody"))
                                    || n.has_tag_name("txBody")
                            }) {
                                let tx_body_range = tx_body.range();
                                let tx_body_xml_str = &slide_xml[tx_body_range.clone()];

                                let mut total_chars = 0;
                                for p in tx_body.descendants().filter(|n| n.has_tag_name("p")) {
                                    for r in p.children().filter(|n| n.has_tag_name("r")) {
                                        if let Some(t) = r.children().find(|n| n.has_tag_name("t"))
                                        {
                                            total_chars += t.text().unwrap_or("").chars().count();
                                        }
                                    }
                                }

                                let start = seg.start.unwrap_or(0);
                                let end = seg.end.unwrap_or(total_chars);

                                let new_tx_body_xml = highlight_tx_body_xml(
                                    tx_body_xml_str,
                                    &format_props,
                                    start,
                                    end,
                                )?;
                                replacements.push((tx_body_range, new_tx_body_xml));
                            }
                        }
                    }
                }
            }
        }

        if !replacements.is_empty() {
            replacements.sort_by_key(|(range, _)| range.start);

            let mut modified_slide_xml = slide_xml.clone();
            for (range, new_xml) in replacements.into_iter().rev() {
                modified_slide_xml.replace_range(range, &new_xml);
            }

            package
                .write_part_xml(&slide_path, &modified_slide_xml)
                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        }
    }

    for key in properties.keys() {
        if !matches!(
            key.as_str(),
            "range_paths" | "bgColor" | "highlight" | "bg" | "color" | "fontColor"
        ) {
            unsupported.push(key.clone());
        }
    }

    Ok(unsupported)
}

fn parse_slide_num_from_full_path(path: &str) -> Result<usize, HandlerError> {
    path.split('/')
        .find(|s| !s.is_empty())
        .and_then(|s| s.strip_prefix("slide["))
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))
}

fn parse_shape_idx(path: &str) -> Result<usize, HandlerError> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .nth(1)
        .and_then(|s| s.strip_prefix("shape["))
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))
}

fn highlight_tx_body_xml(
    tx_body_xml: &str,
    format_props: &std::collections::HashMap<String, String>,
    target_start: usize,
    target_end: usize,
) -> Result<String, HandlerError> {
    let wrapped = format!(
        "<p:dummy xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">{}</p:dummy>",
        tx_body_xml
    );
    let doc = roxmltree::Document::parse(&wrapped).map_err(|e| {
        HandlerError::OperationFailed(format!("roxmltree parse error in txBody: {}", e))
    })?;
    let root = doc.root_element().first_element_child().ok_or_else(|| {
        HandlerError::OperationFailed("txBody element not found inside dummy root".to_string())
    })?;

    let mut body_pr_xml = None;
    let mut lst_style_xml = None;
    let mut paras = Vec::new();

    for child in root.children() {
        if child.is_element() {
            let tag = child.tag_name().name();
            if tag == "bodyPr" {
                body_pr_xml = Some(serialize_roxml_node(&child));
            } else if tag == "lstStyle" {
                lst_style_xml = Some(serialize_roxml_node(&child));
            } else if tag == "p" {
                let mut p_pr_xml = None;
                let mut elements = Vec::new();
                for p_child in child.children() {
                    if p_child.is_element() {
                        let p_tag = p_child.tag_name().name();
                        if p_tag == "pPr" {
                            p_pr_xml = Some(serialize_roxml_node(&p_child));
                        } else if p_tag == "r" {
                            let text = p_child
                                .children()
                                .find(|n| n.has_tag_name("t"))
                                .and_then(|n| n.text())
                                .unwrap_or("")
                                .to_string();
                            let r_pr_xml = p_child
                                .children()
                                .find(|n| n.has_tag_name("rPr"))
                                .map(|n| serialize_roxml_node(&n));
                            elements.push(PptxParaElement::Run { text, r_pr_xml });
                        } else if p_tag == "br" {
                            elements.push(PptxParaElement::Break {
                                raw_xml: serialize_roxml_node(&p_child),
                            });
                        } else {
                            elements.push(PptxParaElement::Other {
                                raw_xml: serialize_roxml_node(&p_child),
                            });
                        }
                    }
                }
                paras.push(PptxPara { p_pr_xml, elements });
            }
        }
    }

    let mut runs_meta = Vec::new();
    let mut global_start = 0;
    for (p_idx, para) in paras.iter().enumerate() {
        for (el_idx, el) in para.elements.iter().enumerate() {
            if let PptxParaElement::Run { text, .. } = el {
                let len = text.chars().count();
                let global_end = global_start + len;
                runs_meta.push((p_idx, el_idx, global_start, global_end, len));
                global_start = global_end;
            }
        }
    }

    for (p_idx, el_idx, r_start, r_end, _r_len) in runs_meta.into_iter().rev() {
        let overlap_start = r_start.max(target_start);
        let overlap_end = r_end.min(target_end);

        if overlap_start < overlap_end {
            let local_start = overlap_start - r_start;
            let local_end = overlap_end - r_start;

            let (run_text, r_pr_xml) = match &paras[p_idx].elements[el_idx] {
                PptxParaElement::Run { text, r_pr_xml } => (text.clone(), r_pr_xml.clone()),
                _ => continue,
            };

            let byte_start = run_text
                .char_indices()
                .nth(local_start)
                .map(|(i, _)| i)
                .unwrap_or(run_text.len());
            let byte_end = run_text
                .char_indices()
                .nth(local_end)
                .map(|(i, _)| i)
                .unwrap_or(run_text.len());

            let mut split_elements = Vec::new();

            if byte_start > 0 {
                split_elements.push(PptxParaElement::Run {
                    text: run_text[..byte_start].to_string(),
                    r_pr_xml: r_pr_xml.clone(),
                });
            }

            let mid_text = run_text[byte_start..byte_end].to_string();
            if !mid_text.is_empty() {
                let merged_r_pr = merge_pptx_run_properties(r_pr_xml.as_deref(), format_props);
                split_elements.push(PptxParaElement::Run {
                    text: mid_text,
                    r_pr_xml: Some(merged_r_pr),
                });
            }

            if byte_end < run_text.len() {
                split_elements.push(PptxParaElement::Run {
                    text: run_text[byte_end..].to_string(),
                    r_pr_xml: r_pr_xml.clone(),
                });
            }

            paras[p_idx]
                .elements
                .splice(el_idx..=el_idx, split_elements);
        }
    }

    let mut result = String::new();
    result.push_str("<p:txBody>");
    if let Some(bp) = body_pr_xml {
        result.push_str(&bp);
    }
    if let Some(ls) = lst_style_xml {
        result.push_str(&ls);
    }
    for para in paras {
        result.push_str("<a:p>");
        if let Some(pp) = para.p_pr_xml {
            result.push_str(&pp);
        }
        for el in para.elements {
            match el {
                PptxParaElement::Run { text, r_pr_xml } => {
                    result.push_str("<a:r>");
                    if let Some(rp) = r_pr_xml {
                        result.push_str(&rp);
                    }
                    result.push_str(&format!("<a:t>{}</a:t>", escape_xml_text(&text)));
                    result.push_str("</a:r>");
                }
                PptxParaElement::Break { raw_xml } => {
                    result.push_str(&raw_xml);
                }
                PptxParaElement::Other { raw_xml } => {
                    result.push_str(&raw_xml);
                }
            }
        }
        result.push_str("</a:p>");
    }
    result.push_str("</p:txBody>");
    Ok(result)
}

fn serialize_roxml_node(node: &roxmltree::Node) -> String {
    let mut result = String::new();
    let name = node.tag_name().name();
    let prefix = node
        .tag_name()
        .namespace()
        .map(|ns| {
            if ns.contains("drawingml") {
                "a:"
            } else if ns.contains("presentationml") {
                "p:"
            } else {
                ""
            }
        })
        .unwrap_or("");

    let prefixed_name = format!("{}{}", prefix, name);

    let mut attrs = Vec::new();
    for attr in node.attributes() {
        attrs.push(format!("{}=\"{}\"", attr.name(), attr.value()));
    }
    let attr_str = if attrs.is_empty() {
        String::new()
    } else {
        format!(" {}", attrs.join(" "))
    };

    if node.children().any(|c| c.is_element()) || node.text().is_some() {
        result.push_str(&format!("<{}{}>", prefixed_name, attr_str));
        for child in node.children() {
            if child.is_element() {
                result.push_str(&serialize_roxml_node(&child));
            } else if child.is_text() {
                result.push_str(&escape_xml_text(child.text().unwrap_or("")));
            }
        }
        result.push_str(&format!("</{}>", prefixed_name));
    } else {
        result.push_str(&format!("<{}{} />", prefixed_name, attr_str));
    }
    result
}

fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn merge_pptx_run_properties(
    r_pr_xml: Option<&str>,
    format_props: &std::collections::HashMap<String, String>,
) -> String {
    let mut attrs = Vec::new();
    let mut children_xml = String::new();

    if let Some(xml) = r_pr_xml {
        if let Ok(doc) = roxmltree::Document::parse(xml) {
            let root = doc.root_element();
            for attr in root.attributes() {
                attrs.push(format!("{}=\"{}\"", attr.name(), attr.value()));
            }
            for child in root.children() {
                if child.is_element() {
                    let tag = child.tag_name().name();
                    if tag == "solidFill"
                        && (format_props.contains_key("color")
                            || format_props.contains_key("fontColor"))
                    {
                        continue;
                    }
                    if tag == "highlight"
                        && (format_props.contains_key("bgColor")
                            || format_props.contains_key("highlight")
                            || format_props.contains_key("bg"))
                    {
                        continue;
                    }
                    children_xml.push_str(&serialize_roxml_node(&child));
                }
            }
        }
    }

    if let Some(color_val) = format_props
        .get("color")
        .or_else(|| format_props.get("fontColor"))
    {
        let hex = color_val.strip_prefix('#').unwrap_or(color_val);
        children_xml.push_str(&format!(
            "<a:solidFill><a:srgbClr val=\"{}\"/></a:solidFill>",
            hex
        ));
    }

    if let Some(bg_val) = format_props
        .get("bgColor")
        .or_else(|| format_props.get("highlight"))
        .or_else(|| format_props.get("bg"))
    {
        let hex = bg_val.strip_prefix('#').unwrap_or(bg_val);
        let hex_lower = hex.to_lowercase();
        let resolved_hex = match hex_lower.as_str() {
            "yellow" => "FFFF00",
            "green" => "00FF00",
            "blue" => "0000FF",
            "magenta" => "FF00FF",
            "cyan" => "00FFFF",
            "red" => "FF0000",
            "white" => "FFFFFF",
            "black" => "000000",
            other => other,
        };
        children_xml.push_str(&format!(
            "<a:highlight><a:srgbClr val=\"{}\"/></a:highlight>",
            resolved_hex
        ));
    }

    let attr_str = if attrs.is_empty() {
        String::new()
    } else {
        format!(" {}", attrs.join(" "))
    };

    if children_xml.is_empty() {
        format!("<a:rPr{} />", attr_str)
    } else {
        format!("<a:rPr{}>{}</a:rPr>", attr_str, children_xml)
    }
}

// ─── Find & Replace ──────────────────────────────────────────────────

/// Apply find/replace to PPT text runs (`<a:t>...</a:t>`).
///
/// Scope:
///   - Path "/" or empty → all slides
///   - Path "/slide[N]" → that slide only
///   - Path "/slide[N]/shape[M]" → that shape only
pub fn apply_pptx_find_replace(
    package: &mut oxml::OxmlPackage,
    path: &str,
    properties: &std::collections::HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let (find, replace, opts) = extract_find_replace_props(properties).ok_or_else(|| {
        HandlerError::InvalidArgument(
            "find/replace requires at least a 'find=<text>' property".to_string(),
        )
    })?;

    // Determine slide scope. We accept "/" or empty (all) and "/slide[N]" or
    // "/slide[N]/shape[...]" (one slide).
    let path_lc = path.trim().to_lowercase();
    let slide_idx: Option<usize> = if path_lc.is_empty() || path_lc == "/" {
        None
    } else {
        let segs = crate::navigation::parse_path(path);
        if let Some(first) = segs.first() {
            if first.name.eq_ignore_ascii_case("slide") {
                first.index
            } else {
                None
            }
        } else {
            None
        }
    };

    let pres = build_presentation(package)?;
    let mut total = 0usize;

    for slide in &pres.slides {
        if let Some(idx) = slide_idx {
            if slide.index != idx {
                continue;
            }
        }
        let part = match slide.part_path.strip_prefix('/') {
            Some(p) => p.to_string(),
            None => slide.part_path.clone(),
        };
        let xml = match package.read_part_xml(&part) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let (new_xml, n) = replace_in_xml_text_nodes(&xml, &find, &replace, &opts, "</a:t>");
        if n > 0 {
            total += n;
            package
                .write_part_xml(&part, &new_xml)
                .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        }
    }

    Ok(vec![format!("replaced={}", total)])
}

/// Walk every `<a:t>...</a:t>` block in `xml` and run replace_in_string on
/// its inner text. Returns (new_xml, count). Conservative: does not span
/// across runs, but matches the common case where users search literal text.
fn replace_in_xml_text_nodes(
    xml: &str,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
    close_tag: &str,
) -> (String, usize) {
    let mut out = String::with_capacity(xml.len());
    let mut cursor = 0;
    let mut total = 0usize;

    while let Some(close_start) = xml[cursor..].find(close_tag) {
        let close_abs = cursor + close_start;
        // Walk back to find the matching `<a:t>` or `<a:t ...>` opening tag.
        let prefix = &xml[..close_abs];
        let mut search_from = 0;
        let mut open_idx: Option<usize> = None;
        while let Some(o) = prefix[search_from..].find("<a:t") {
            let abs = search_from + o;
            let after = &prefix[abs + 4..];
            let c = after.as_bytes().first().copied();
            match c {
                Some(b'>') | Some(b' ') | Some(b'/') | Some(b'\t') | Some(b'\n') => {
                    open_idx = Some(abs);
                }
                _ => {}
            }
            search_from = abs + 4;
        }

        let Some(open_abs) = open_idx else {
            out.push_str(&xml[cursor..close_abs + close_tag.len()]);
            cursor = close_abs + close_tag.len();
            continue;
        };

        // Find the close of the opening tag.
        let Some(gt_rel) = xml[open_abs..close_abs].find('>') else {
            out.push_str(&xml[cursor..close_abs + close_tag.len()]);
            cursor = close_abs + close_tag.len();
            continue;
        };
        let open_close = open_abs + gt_rel + 1;
        let inner = &xml[open_close..close_abs];
        let (new_inner, n) = replace_in_string(inner, find, replace, opts);
        total += n;

        out.push_str(&xml[cursor..open_close]);
        out.push_str(&new_inner);
        cursor = close_abs;
        out.push_str(&xml[cursor..cursor + close_tag.len()]);
        cursor += close_tag.len();
    }
    out.push_str(&xml[cursor..]);
    (out, total)
}

// Re-export for symmetry with docx/xlsx handler surfaces.
pub use handler_common::find_replace_property_keys;

#[cfg(test)]
mod group_resize_tests {
    use super::{apply_group_properties, slide_xml_has_visual_content};
    use std::collections::HashMap;

    const SLIDE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree>
    <p:nvGrpSpPr><p:cNvPr id="1" name="Root"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
    <p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
    <p:grpSp>
      <p:nvGrpSpPr><p:cNvPr id="2" name="Diagram"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="1000" cy="500"/><a:chOff x="100" y="200"/><a:chExt cx="1000" cy="500"/></a:xfrm></p:grpSpPr>
      <p:sp>
        <p:nvSpPr><p:cNvPr id="3" name="Node"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr>
        <p:spPr><a:xfrm><a:off x="100" y="200"/><a:ext cx="1000" cy="500"/></a:xfrm></p:spPr>
        <p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr><a:defRPr sz="1200"/></a:lvl1pPr></a:lstStyle><a:p><a:r><a:rPr sz="2000"/><a:t>Node</a:t></a:r><a:endParaRPr sz="1800"/></a:p></p:txBody>
      </p:sp>
    </p:grpSp>
  </p:spTree></p:cSld>
</p:sld>"#;

    #[test]
    fn table_only_slide_is_visual_content() {
        let xml = r#"<p:sld xmlns:p="urn:p"><p:cSld><p:spTree><p:nvGrpSpPr/><p:grpSpPr/><p:graphicFrame/></p:spTree></p:cSld></p:sld>"#;
        assert!(slide_xml_has_visual_content(xml));
    }

    #[test]
    fn metadata_only_shape_tree_is_empty() {
        let xml = r#"<p:sld xmlns:p="urn:p"><p:cSld><p:spTree><p:nvGrpSpPr/><p:grpSpPr/></p:spTree></p:cSld></p:sld>"#;
        assert!(!slide_xml_has_visual_content(xml));
    }

    fn group_transform_attr(xml: &str, element: &str, attribute: &str) -> i64 {
        let doc = roxmltree::Document::parse(xml).unwrap();
        let shape_tree = doc
            .descendants()
            .find(|node| node.tag_name().name() == "spTree")
            .unwrap();
        let group = shape_tree
            .children()
            .find(|node| node.tag_name().name() == "grpSp")
            .unwrap();
        let transform = group
            .children()
            .find(|node| node.tag_name().name() == "grpSpPr")
            .unwrap()
            .children()
            .find(|node| node.tag_name().name() == "xfrm")
            .unwrap();
        transform
            .children()
            .find(|node| node.tag_name().name() == element)
            .unwrap()
            .attribute(attribute)
            .unwrap()
            .parse()
            .unwrap()
    }

    fn font_sizes(xml: &str) -> Vec<i64> {
        let doc = roxmltree::Document::parse(xml).unwrap();
        let mut sizes: Vec<i64> = doc
            .descendants()
            .filter(|node| matches!(node.tag_name().name(), "rPr" | "endParaRPr" | "defRPr"))
            .filter_map(|node| node.attribute("sz"))
            .map(|value| value.parse().unwrap())
            .collect();
        sizes.sort_unstable();
        sizes
    }

    #[test]
    fn group_width_only_changes_one_axis_by_default() {
        let properties = HashMap::from([("width".to_string(), "2000".to_string())]);

        let (updated, unsupported) = apply_group_properties(SLIDE_XML, 1, &properties).unwrap();

        assert!(unsupported.is_empty());
        assert_eq!(group_transform_attr(&updated, "ext", "cx"), 2000);
        assert_eq!(group_transform_attr(&updated, "ext", "cy"), 500);
        assert_eq!(group_transform_attr(&updated, "chExt", "cx"), 1000);
        assert_eq!(group_transform_attr(&updated, "chExt", "cy"), 500);
        assert_eq!(font_sizes(&updated), [1200, 1800, 2000]);
    }

    #[test]
    fn group_keep_aspect_scales_other_axis_and_fonts() {
        let properties = HashMap::from([
            ("width".to_string(), "2000".to_string()),
            ("keepAspect".to_string(), "true".to_string()),
        ]);

        let (updated, unsupported) = apply_group_properties(SLIDE_XML, 1, &properties).unwrap();

        assert!(unsupported.is_empty());
        assert_eq!(group_transform_attr(&updated, "ext", "cx"), 2000);
        assert_eq!(group_transform_attr(&updated, "ext", "cy"), 1000);
        assert_eq!(font_sizes(&updated), [2400, 3600, 4000]);
    }

    #[test]
    fn group_single_axis_shrink_rebakes_fonts_by_minimum_ratio() {
        let properties = HashMap::from([("height".to_string(), "250".to_string())]);

        let (updated, _) = apply_group_properties(SLIDE_XML, 1, &properties).unwrap();

        assert_eq!(group_transform_attr(&updated, "ext", "cx"), 1000);
        assert_eq!(group_transform_attr(&updated, "ext", "cy"), 250);
        assert_eq!(font_sizes(&updated), [600, 900, 1000]);
    }

    #[test]
    fn group_first_mutation_snapshots_missing_child_baselines() {
        let without_baselines = SLIDE_XML
            .replace("<a:chOff x=\"100\" y=\"200\"/>", "")
            .replace("<a:chExt cx=\"1000\" cy=\"500\"/>", "");
        let properties = HashMap::from([
            ("x".to_string(), "300".to_string()),
            ("width".to_string(), "2000".to_string()),
        ]);

        let (updated, _) = apply_group_properties(&without_baselines, 1, &properties).unwrap();

        assert_eq!(group_transform_attr(&updated, "off", "x"), 300);
        assert_eq!(group_transform_attr(&updated, "chOff", "x"), 100);
        assert_eq!(group_transform_attr(&updated, "chOff", "y"), 200);
        assert_eq!(group_transform_attr(&updated, "chExt", "cx"), 1000);
        assert_eq!(group_transform_attr(&updated, "chExt", "cy"), 500);
    }

    #[test]
    fn group_rejects_non_positive_size() {
        let properties = HashMap::from([("width".to_string(), "0".to_string())]);

        let error = apply_group_properties(SLIDE_XML, 1, &properties).unwrap_err();

        assert!(error.to_string().contains("size must be positive"));
    }
}
