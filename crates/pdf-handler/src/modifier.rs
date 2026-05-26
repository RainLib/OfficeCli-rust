use std::collections::HashMap;
use handler_common::HandlerError;
use lopdf::Document as LopdfDocument;
use lopdf::ObjectId;
use crate::content_stream::{parse_page_content_stream, encode_pdf_string, ParsedContentStream, PdfTextBlock, FontInfo, estimate_text_width, PdfColor, TextStyle};

/// Replace text at a specific path like /page[1]/text[3].
/// Only modifies the Tj/TJ line for that specific text block.
pub fn replace_text_at_path(
    doc: &mut LopdfDocument,
    page_num: usize,
    text_index: usize,   // 1-based
    new_text: &str,
) -> Result<(), HandlerError> {
    let pages = doc.get_pages();
    let page_id = pages.get(&(page_num as u32))
        .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

    let content = doc.get_page_content(*page_id)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to get page content: {}", e)))?;

    let parsed = parse_page_content_stream(&content, *page_id, doc)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to parse content stream: {}", e)))?;

    let block_idx = text_index - 1;
    if block_idx >= parsed.text_blocks.len() {
        return Err(HandlerError::PathNotFound(format!(
            "text[{}] not found (page {} has {} text blocks)",
            text_index, page_num, parsed.text_blocks.len()
        )));
    }

    let target_block = &parsed.text_blocks[block_idx];
    let old_width = compute_block_width(&target_block.text, &parsed.font_map, &target_block.style);
    let new_width = compute_block_width(new_text, &parsed.font_map, &target_block.style);
    let width_delta = new_width - old_width;

    let mut modified_lines = parsed.lines.clone();
    let encoded = encode_pdf_string(new_text);
    modified_lines[target_block.text_line_index] = format!("{} Tj", encoded);

    // Adjust position of subsequent blocks in the same BT section if width changed
    if width_delta != 0.0 {
        let bt_start = target_block.bt_start_line;
        let bt_end = target_block.bt_end_line;
        for other_block in &parsed.text_blocks[block_idx + 1..] {
            if other_block.bt_start_line == bt_start && other_block.bt_end_line == bt_end {
                adjust_position_lines(&mut modified_lines, other_block, width_delta, &parsed);
            }
        }
    }

    let modified_content = modified_lines.join("\n");
    write_content_to_page(doc, *page_id, modified_content.as_bytes())?;
    Ok(())
}

/// Replace text at a specific path with style modifications.
/// After changing the target block's style, restores the original style for subsequent blocks
/// in the same BT section so they don't inherit the changed style.
pub fn replace_text_with_style(
    doc: &mut LopdfDocument,
    page_num: usize,
    text_index: usize,
    new_text: Option<&str>,
    font_name: Option<&str>,
    font_size: Option<f32>,
    fill_color: Option<&PdfColor>,
    char_spacing: Option<f32>,
    word_spacing: Option<f32>,
) -> Result<(), HandlerError> {
    let pages = doc.get_pages();
    let page_id = pages.get(&(page_num as u32))
        .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

    let content = doc.get_page_content(*page_id)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to get page content: {}", e)))?;

    let parsed = parse_page_content_stream(&content, *page_id, doc)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to parse content stream: {}", e)))?;

    let block_idx = text_index - 1;
    if block_idx >= parsed.text_blocks.len() {
        return Err(HandlerError::PathNotFound(format!("text[{}] not found", text_index)));
    }

    let target_block = &parsed.text_blocks[block_idx];
    let mut modified_lines = parsed.lines.clone();

    // Build style insertion lines
    let mut style_lines = Vec::new();
    let effective_font = font_name.or(target_block.style.font_name.as_deref()).unwrap_or("F1");
    let effective_size = font_size.or(target_block.style.font_size).unwrap_or(12.0);

    if font_name.is_some() || font_size.is_some() {
        style_lines.push(format!("/{} {} Tf", effective_font, effective_size));
    }

    if let Some(color) = fill_color {
        match color {
            PdfColor::Gray(g) => style_lines.push(format!("{} g", g)),
            PdfColor::Rgb(r, g, b) => style_lines.push(format!("{} {} {} rg", r, g, b)),
            PdfColor::Cmyk(c, m, y, k) => style_lines.push(format!("{} {} {} {} k", c, m, y, k)),
        }
    }

    if let Some(cs) = char_spacing {
        style_lines.push(format!("{} Tc", cs));
    }
    if let Some(ws) = word_spacing {
        style_lines.push(format!("{} Tw", ws));
    }

    // Build restore lines to reset the original style for subsequent blocks
    let mut restore_lines = Vec::new();
    let has_subsequent = parsed.text_blocks[block_idx + 1..]
        .iter()
        .any(|b| b.bt_start_line == target_block.bt_start_line && b.bt_end_line == target_block.bt_end_line);

    if has_subsequent {
        // Restore font if changed
        if font_name.is_some() || font_size.is_some() {
            let orig_font = target_block.style.font_name.as_deref().unwrap_or("F1");
            let orig_size = target_block.style.font_size.unwrap_or(12.0);
            restore_lines.push(format!("/{} {} Tf", orig_font, orig_size));
        }
        // Restore color if changed
        if let Some(_color) = fill_color {
            if let Some(ref orig_color) = target_block.style.fill_color {
                match orig_color {
                    PdfColor::Gray(g) => restore_lines.push(format!("{} g", g)),
                    PdfColor::Rgb(r, g, b) => restore_lines.push(format!("{} {} {} rg", r, g, b)),
                    PdfColor::Cmyk(c, m, y, k) => restore_lines.push(format!("{} {} {} {} k", c, m, y, k)),
                }
            }
        }
        // Restore char spacing if changed
        if char_spacing.is_some() {
            restore_lines.push(format!("{} Tc", target_block.style.char_spacing));
        }
        // Restore word spacing if changed
        if word_spacing.is_some() {
            restore_lines.push(format!("{} Tw", target_block.style.word_spacing));
        }
    }

    // Replace the text line
    let effective_text = new_text.map(|s| s.to_string())
        .unwrap_or_else(|| target_block.text.clone());
    modified_lines[target_block.text_line_index] = format!("{} Tj", encode_pdf_string(&effective_text));

    // Insert style lines before text line, restore lines after text line
    if !style_lines.is_empty() || !restore_lines.is_empty() {
        let insert_pos = target_block.text_line_index;
        let mut new_lines = modified_lines[..insert_pos].to_vec();
        for line in &style_lines {
            new_lines.push(line.clone());
        }
        // Insert the Tj line
        new_lines.push(modified_lines[insert_pos].clone());
        // Insert restore lines after Tj
        for line in &restore_lines {
            new_lines.push(line.clone());
        }
        new_lines.extend_from_slice(&modified_lines[insert_pos + 1..]);
        modified_lines = new_lines;
    }

    let modified_content = modified_lines.join("\n");
    write_content_to_page(doc, *page_id, modified_content.as_bytes())?;
    Ok(())
}

fn compute_block_width(text: &str, font_map: &HashMap<String, FontInfo>, style: &TextStyle) -> f32 {
    if let Some(ref font_name) = style.font_name {
        let font_info = font_map.get(font_name).cloned().unwrap_or_else(|| FontInfo {
            pdf_name: font_name.clone(),
            base_font: None,
            is_cid_font: false,
            char_widths: HashMap::new(),
            default_width: 500.0,
        });
        let font_size = style.font_size.unwrap_or(12.0);
        estimate_text_width(text, &font_info, font_size, style.char_spacing, style.word_spacing)
    } else {
        text.chars().count() as f32 * style.font_size.unwrap_or(12.0) * 0.5
    }
}

fn adjust_position_lines(
    modified_lines: &mut Vec<String>,
    block: &PdfTextBlock,
    delta: f32,
    parsed: &ParsedContentStream,
) {
    for line_idx in block.bt_start_line..block.text_line_index {
        let trimmed = parsed.lines[line_idx].trim();
        // Adjust Tm lines (absolute position): x = operands[4]
        if trimmed.ends_with("Tm") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() == 7 {
                if let Ok(x_val) = parts[4].parse::<f32>() {
                    let new_x = x_val + delta;
                    modified_lines[line_idx] = format!(
                        "{} {} {} {} {} {} Tm",
                        parts[0], parts[1], parts[2], parts[3], new_x, parts[5]
                    );
                }
            }
        }
        // Adjust Td/TD lines (relative offset): tx = operands[0]
        else if trimmed.ends_with("Td") || trimmed.ends_with("TD") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() == 3 {
                if let Ok(tx_val) = parts[0].parse::<f32>() {
                    let new_tx = tx_val + delta;
                    modified_lines[line_idx] = format!(
                        "{} {} {}",
                        new_tx, parts[1], parts[2]
                    );
                }
            }
        }
    }
}

fn write_content_to_page(doc: &mut LopdfDocument, page_id: ObjectId, content: &[u8]) -> Result<(), HandlerError> {
    let content_ids = doc.get_page_contents(page_id);
    for content_id in content_ids {
        if let Ok(obj) = doc.get_object_mut(content_id) {
            if let lopdf::Object::Stream(stream) = obj {
                stream.content = content.to_vec();
                stream.dict.set("Length", lopdf::Object::Integer(content.len() as i64));
            }
        }
    }
    Ok(())
}

/// Legacy: replace all Tj strings on a page with the same text.
pub fn replace_text_on_page(
    doc: &mut LopdfDocument,
    page_num: usize,
    new_text: &str,
) -> Result<(), HandlerError> {
    let pages = doc.get_pages();
    let page_id = pages.get(&(page_num as u32))
        .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

    let content = doc.get_page_content(*page_id)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to get page content: {}", e)))?;

    let content_str = String::from_utf8_lossy(&content);
    let modified = blanket_replace_strings(&content_str, new_text);

    write_content_to_page(doc, *page_id, modified.as_bytes())?;
    Ok(())
}

fn blanket_replace_strings(stream: &str, new_text: &str) -> String {
    let mut result = String::new();
    let mut in_text_object = false;
    let encoded = encode_pdf_string(new_text);

    for line in stream.lines() {
        let trimmed = line.trim();
        if trimmed == "BT" { in_text_object = true; result.push_str(line); result.push('\n'); continue; }
        if trimmed == "ET" { in_text_object = false; result.push_str(line); result.push('\n'); continue; }
        if !in_text_object { result.push_str(line); result.push('\n'); continue; }

        if trimmed.ends_with(" Tj") {
            let string_part = trimmed.trim_end_matches(" Tj").trim();
            if string_part.starts_with('(') && string_part.ends_with(')') {
                result.push_str(&format!("{} Tj", encoded));
                result.push('\n');
            } else {
                result.push_str(line); result.push('\n');
            }
        } else {
            result.push_str(line); result.push('\n');
        }
    }
    result
}

/// Replace entire page content with new content bytes.
pub fn replace_page_content(
    doc: &mut LopdfDocument,
    page_id: ObjectId,
    new_content: &[u8],
) -> Result<(), HandlerError> {
    write_content_to_page(doc, page_id, new_content)?;
    Ok(())
}

/// Delete a page from the PDF document.
pub fn delete_page(doc: &mut LopdfDocument, page_num: usize) -> Result<(), HandlerError> {
    doc.delete_pages(&[page_num as u32]);
    Ok(())
}