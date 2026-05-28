use handler_common::{DocumentIssue, DocumentNode, HandlerError, IssueSeverity, ValidationError, ViewOptions};
use crate::dom_types::{Slide, Shape};
use crate::navigation::{build_presentation, find_slide, find_shape, find_paragraph};

/// ViewAsText: show all slide text content, similar to Word's view_as_text.
pub fn view_as_text(package: &oxml::OxmlPackage, opts: &ViewOptions) -> Result<String, HandlerError> {
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
        lines.push(format!("  slide[{}]: {} shapes", slide.index, slide.shapes.len()));
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
pub fn view_as_annotated(package: &oxml::OxmlPackage, opts: &ViewOptions) -> Result<String, HandlerError> {
    let pres = build_presentation(package)?;
    let mut lines = Vec::new();

    for slide in &pres.slides {
        lines.push(format!("[/slide[{}]] --- Slide {} ---", slide.index, slide.index));
        for (si, shape) in slide.shapes.iter().enumerate() {
            let label = match &shape.placeholder_type {
                Some(pt) => format!("({}) ", pt),
                None => String::new(),
            };
            lines.push(format!(
                "[/slide[{}]/shape[{}]] {}{}",
                slide.index, si + 1, label, shape.text
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
pub fn view_as_text_json(package: &oxml::OxmlPackage, _opts: &ViewOptions) -> Result<serde_json::Value, HandlerError> {
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
pub fn view_as_outline_json(package: &oxml::OxmlPackage) -> Result<serde_json::Value, HandlerError> {
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
pub fn get_node(package: &oxml::OxmlPackage, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
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
        return Err(HandlerError::InvalidPath(format!("expected 'slide' segment, got '{}'", first.name)));
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
        return Err(HandlerError::InvalidPath(format!("expected 'shape' segment, got '{}'", second.name)));
    }
    let shape_idx = second.index.unwrap_or(1);
    let shape = find_shape(slide, shape_idx)
        .ok_or_else(|| HandlerError::PathNotFound(format!("/slide[{}]/shape[{}]", slide_idx, shape_idx)))?;

    if segments.len() == 2 {
        // Shape node
        let node = make_shape_node(slide_idx, shape_idx, shape, depth > 0);
        return Ok(node);
    }

    // Third segment: "paragraph[K]"
    let third = &segments[2];
    if third.name != "paragraph" {
        return Err(HandlerError::InvalidPath(format!("expected 'paragraph' segment, got '{}'", third.name)));
    }
    let para_idx = third.index.unwrap_or(1);
    let para = find_paragraph(shape, para_idx)
        .ok_or_else(|| HandlerError::PathNotFound(
            format!("/slide[{}]/shape[{}]/paragraph[{}]", slide_idx, shape_idx, para_idx)
        ))?;

    let node = DocumentNode::new(
        &format!("/slide[{}]/shape[{}]/paragraph[{}]", slide_idx, shape_idx, para_idx),
        "paragraph",
    ).with_text(&para.text);
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
        return Err(HandlerError::InvalidPath("path must be /slide[N]/shape[M] or deeper".to_string()));
    }

    let slide_idx = segments[0].index.unwrap_or(1);
    let shape_idx = segments[1].index.unwrap_or(1);

    // Get the text to set
    let new_text = properties.get("text")
        .ok_or_else(|| HandlerError::InvalidArgument("'text' property required".to_string()))?;

    // First, build the presentation to find the slide part path
    let pres = build_presentation(package)?;
    let slide = find_slide(&pres, slide_idx)
        .ok_or_else(|| HandlerError::PathNotFound(format!("/slide[{}]", slide_idx)))?;

    // Read the slide XML
    let slide_xml = package.read_part_xml(&slide.part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Modify the shape text using roxmltree + quick-xml writer
    let modified_xml = replace_shape_text_in_xml(&slide_xml, shape_idx, new_text)?;

    // Write the modified XML back
    package.write_part_xml(&slide.part_path, &modified_xml)
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
fn replace_shape_text_in_xml(xml: &str, shape_idx: usize, new_text: &str) -> Result<String, HandlerError> {
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
                    writer.write_event(quick_xml::events::Event::Start(e.clone())).ok();
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

                writer.write_event(quick_xml::events::Event::Start(e.clone())).ok();
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let local_name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();

                if in_tx_body {
                    tx_body_depth -= 1;
                    if local_name == "txBody" && tx_body_depth == 0 {
                        // End of txBody — write the closing tag and stop skipping
                        writer.write_event(quick_xml::events::Event::End(e.clone())).ok();
                        in_tx_body = false;
                        skip_old_text = false;
                        continue;
                    }
                    // Skip old content inside txBody
                    continue;
                }

                writer.write_event(quick_xml::events::Event::End(e.clone())).ok();

                if local_name == "sp" && in_target_shape {
                    sp_depth -= 1;
                    if sp_depth == 0 {
                        in_target_shape = false;
                    }
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                if !in_tx_body {
                    writer.write_event(quick_xml::events::Event::Empty(e.clone())).ok();
                }
            }
            Ok(quick_xml::events::Event::Text(e)) => {
                if !skip_old_text {
                    writer.write_event(quick_xml::events::Event::Text(e.clone())).ok();
                }
            }
            Ok(quick_xml::events::Event::CData(e)) => {
                if !skip_old_text {
                    writer.write_event(quick_xml::events::Event::CData(e.clone())).ok();
                }
            }
            Ok(quick_xml::events::Event::Decl(e)) => {
                writer.write_event(quick_xml::events::Event::Decl(e.clone())).ok();
            }
            Ok(quick_xml::events::Event::Comment(e)) => {
                writer.write_event(quick_xml::events::Event::Comment(e.clone())).ok();
            }
            Ok(quick_xml::events::Event::PI(e)) => {
                writer.write_event(quick_xml::events::Event::PI(e.clone())).ok();
            }
            Ok(quick_xml::events::Event::DocType(e)) => {
                writer.write_event(quick_xml::events::Event::DocType(e.clone())).ok();
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => {
                return Err(HandlerError::OperationFailed(format!("XML rewrite error: {}", e)));
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
    writer.write_event(quick_xml::events::Event::Start(p_start)).ok();

    // <a:r>
    let r_start = quick_xml::events::BytesStart::new("a:r");
    writer.write_event(quick_xml::events::Event::Start(r_start)).ok();

    // <a:t>
    let t_start = quick_xml::events::BytesStart::new("a:t");
    writer.write_event(quick_xml::events::Event::Start(t_start)).ok();

    // Text content
    let text_event = quick_xml::events::BytesText::new(text);
    writer.write_event(quick_xml::events::Event::Text(text_event)).ok();

    // </a:t>
    writer.write_event(quick_xml::events::Event::End(quick_xml::events::BytesEnd::new("a:t"))).ok();

    // </a:r>
    writer.write_event(quick_xml::events::Event::End(quick_xml::events::BytesEnd::new("a:r"))).ok();

    // </a:p>
    writer.write_event(quick_xml::events::Event::End(quick_xml::events::BytesEnd::new("a:p"))).ok();
}

/// Apply line range from ViewOptions to the output text.
fn apply_line_range(text: &str, opts: &ViewOptions) -> String {
    let all_lines: Vec<&str> = text.lines().collect();
    let total = all_lines.len();

    let start = opts.start_line.map(|l| if l > 0 { l - 1 } else { 0 }).unwrap_or(0);
    let end = opts.end_line.map(|l| if l > total { total } else { l }).unwrap_or(total);

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
    let preview_parts: Vec<String> = slide.shapes.iter()
        .filter_map(|s| {
            if s.text.is_empty() { None } else { Some(s.text.clone()) }
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
fn make_shape_node(slide_idx: usize, shape_idx: usize, shape: &Shape, include_children: bool) -> DocumentNode {
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
            let para_path = format!("/slide[{}]/shape[{}]/paragraph[{}]", slide_idx, shape_idx, pi + 1);
            para_nodes.push(DocumentNode::new(&para_path, "paragraph").with_text(&para.text));
        }
        node = node.with_children(para_nodes);
    } else {
        node.child_count = shape.paragraphs.len();
    }

    node
}

/// Detect issues in the presentation.
pub fn view_as_issues(package: &oxml::OxmlPackage, issue_type: Option<&str>, limit: Option<usize>) -> Result<Vec<DocumentIssue>, HandlerError> {
    let pres = build_presentation(package)?;
    let mut issues = Vec::new();

    // Check for missing slide parts
    for slide in &pres.slides {
        if !package.has_part(&slide.part_path) {
            issues.push(DocumentIssue {
                severity: IssueSeverity::Warning,
                issue_type: "missing-slide".to_string(),
                description: format!("Slide {} part '{}' is missing from the package", slide.index, slide.part_path),
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
                    description: format!("Shape {} on slide {} has no ID attribute", si + 1, slide.index),
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
    let pres_xml = package.read_part_xml("ppt/presentation.xml")
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