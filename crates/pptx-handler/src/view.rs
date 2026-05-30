use crate::dom_types::{Shape, Slide};
use crate::navigation::{build_presentation, find_paragraph, find_shape, find_slide};
use handler_common::{
    DocumentIssue, DocumentNode, HandlerError, IssueSeverity, ValidationError, ViewOptions,
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
        lines.push(format!(
            "  slide[{}]: {} shapes",
            slide.index,
            slide.shapes.len()
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
    let segments = crate::navigation::parse_path(path);

    // We need at least /slide[N]/shape[M]
    if segments.len() < 2 {
        return Err(HandlerError::InvalidPath(
            "path must be /slide[N]/shape[M] or deeper".to_string(),
        ));
    }

    let slide_idx = segments[0].index.unwrap_or(1);
    let shape_idx = segments[1].index.unwrap_or(1);

    // Get the text to set
    let new_text = properties
        .get("text")
        .ok_or_else(|| HandlerError::InvalidArgument("'text' property required".to_string()))?;

    // First, build the presentation to find the slide part path
    let pres = build_presentation(package)?;
    let slide = find_slide(&pres, slide_idx)
        .ok_or_else(|| HandlerError::PathNotFound(format!("/slide[{}]", slide_idx)))?;

    // Read the slide XML
    let slide_xml = package
        .read_part_xml(&slide.part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Modify the shape text using roxmltree + quick-xml writer
    let modified_xml = replace_shape_text_in_xml(&slide_xml, shape_idx, new_text)?;

    // Write the modified XML back
    package
        .write_part_xml(&slide.part_path, &modified_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let mut unsupported = Vec::new();
    for key in properties.keys() {
        if key != "text" {
            unsupported.push(key.clone());
        }
    }
    Ok(unsupported)
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

        // Check for empty slides
        if slide.shapes.is_empty() {
            issues.push(DocumentIssue {
                severity: IssueSeverity::Info,
                issue_type: "empty-slide".to_string(),
                description: format!("Slide {} has no shapes", slide.index),
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
        let slide_path = format!("ppt/slides/slide{}.xml", slide_num);
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
