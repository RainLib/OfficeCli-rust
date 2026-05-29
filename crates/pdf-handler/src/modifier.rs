use crate::content_stream::{
    parse_page_content_stream, pick_fonts_for_text, FontSegment, PdfColor,
};
use handler_common::HandlerError;
use lopdf::Document as LopdfDocument;
use lopdf::ObjectId;

/// Build the replacement token sequence for the Tj line based on font segments.
/// If a single segment with the original font, just returns `[encoded_operand, "Tj"]`.
/// Otherwise emits `/<Font> <size> Tf <hex> Tj` per segment plus a final restore Tf.
fn build_segment_tokens(
    segments: &[FontSegment],
    orig_font: Option<&str>,
    orig_size: f32,
) -> Vec<String> {
    if segments.len() == 1 {
        let only = &segments[0];
        // If the segment already uses the original font, no Tf switching needed.
        if Some(only.font_name.as_str()) == orig_font {
            return vec![only.encoded_operand.clone(), "Tj".to_string()];
        }
    }

    let mut tokens = Vec::with_capacity(segments.len() * 5 + 3);
    for seg in segments {
        tokens.push(format!("/{}", seg.font_name));
        tokens.push(format_size(orig_size));
        tokens.push("Tf".to_string());
        tokens.push(seg.encoded_operand.clone());
        tokens.push("Tj".to_string());
    }

    if let Some(name) = orig_font {
        // Restore the original font so subsequent blocks in the same BT are unaffected.
        tokens.push(format!("/{}", name));
        tokens.push(format_size(orig_size));
        tokens.push("Tf".to_string());
    }

    tokens
}

fn format_size(size: f32) -> String {
    if size.fract().abs() < 1e-3 {
        format!("{}", size as i32)
    } else {
        format!("{}", size)
    }
}

/// Replace text at a specific path like /page[1]/text[3].
/// Only modifies the Tj/TJ line for that specific text block.
/// If the new text contains characters not in the target block's font,
/// it splits into multi-font segments using other fonts on the page.
pub fn replace_text_at_path(
    doc: &mut LopdfDocument,
    page_num: usize,
    text_index: usize, // 1-based
    new_text: &str,
    preferred_font: Option<&str>,
) -> Result<(), HandlerError> {
    let pages = doc.get_pages();
    let page_id = *pages
        .get(&(page_num as u32))
        .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

    let content = doc
        .get_page_content(page_id)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to get page content: {}", e)))?;

    let parsed = parse_page_content_stream(&content, page_id, doc).map_err(|e| {
        HandlerError::OperationFailed(format!("failed to parse content stream: {}", e))
    })?;

    let block_idx = text_index - 1;
    if block_idx >= parsed.text_blocks.len() {
        return Err(HandlerError::PathNotFound(format!(
            "text[{}] not found (page {} has {} text blocks)",
            text_index,
            page_num,
            parsed.text_blocks.len()
        )));
    }

    let target_block = &parsed.text_blocks[block_idx];
    let orig_font_owned = target_block.style.font_name.clone();
    let orig_font = orig_font_owned.as_deref();
    // Use the RAW Tf operand (without Tm scaling). The active Tm matrix from
    // the original content will still scale our re-emitted Tf; writing the
    // effective (already-scaled) size here would compound Tm twice and blow
    // up the rendered font size.
    let orig_size = target_block
        .style
        .raw_font_size
        .or(target_block.style.font_size)
        .unwrap_or(1.0);

    // Pick fonts: preferred_font wins; otherwise default to target block's font first.
    let pref = preferred_font.or(orig_font);
    let mut missing: Vec<char> = Vec::new();
    let segments = pick_fonts_for_text(doc, page_id, pref, new_text, &mut missing)?;

    if !missing.is_empty() {
        return Err(HandlerError::OperationFailed(format!(
            "characters not encodable in any page font: {}. Provide --prop fontFile=<path> or --prop font=<name> to override.",
            missing.iter().collect::<String>()
        )));
    }

    let mut modified_lines = parsed.lines.clone();
    let line = &modified_lines[target_block.text_line_index];
    let mut line_tokens = crate::content_stream::tokenize_pdf_line(line);

    let new_tokens = build_segment_tokens(&segments, orig_font, orig_size);

    if target_block.line_token_index < line_tokens.len() {
        // Replace the operand + operator (Tj/TJ) with our token sequence
        let op_idx = target_block.line_token_index;
        let consume_extra = matches!(
            line_tokens.get(op_idx + 1).map(|s| s.as_str()),
            Some("Tj") | Some("TJ")
        );
        let end = if consume_extra {
            op_idx + 2
        } else {
            op_idx + 1
        };
        line_tokens.splice(op_idx..end, new_tokens);
        modified_lines[target_block.text_line_index] = line_tokens.join(" ");
    } else {
        modified_lines[target_block.text_line_index] = new_tokens.join(" ");
    }

    let modified_content = modified_lines.join("\n");
    write_content_to_page(doc, page_id, modified_content.as_bytes())?;
    Ok(())
}

/// Replace text at a specific path with style modifications.
/// After changing the target block's style, restores the original style for subsequent blocks
/// in the same BT section so they don't inherit the changed style.
/// Also supports cross-font fallback via `preferred_font`.
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
    bg_color: Option<&PdfColor>,
) -> Result<(), HandlerError> {
    let pages = doc.get_pages();
    let page_id = *pages
        .get(&(page_num as u32))
        .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

    let content = doc
        .get_page_content(page_id)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to get page content: {}", e)))?;

    let parsed = parse_page_content_stream(&content, page_id, doc).map_err(|e| {
        HandlerError::OperationFailed(format!("failed to parse content stream: {}", e))
    })?;

    let block_idx = text_index - 1;
    if block_idx >= parsed.text_blocks.len() {
        return Err(HandlerError::PathNotFound(format!(
            "text[{}] not found",
            text_index
        )));
    }

    let target_block = parsed.text_blocks[block_idx].clone();
    let mut modified_lines = parsed.lines.clone();

    // Build style insertion lines (font/size/color/spacing changes)
    let mut style_lines = Vec::new();
    let effective_font = font_name
        .or(target_block.style.font_name.as_deref())
        .unwrap_or("F1")
        .to_string();
    // For Tf operands we want the RAW size, not the Tm-multiplied effective size.
    // User-supplied --prop size=X keeps the historical "raw operand" semantics.
    let effective_size = font_size
        .or(target_block.style.raw_font_size)
        .or(target_block.style.font_size)
        .unwrap_or(12.0);

    if font_name.is_some() || font_size.is_some() {
        style_lines.push(format!(
            "/{} {} Tf",
            effective_font,
            format_size(effective_size)
        ));
    }

    if let Some(color) = fill_color {
        match color {
            PdfColor::Gray(g) => style_lines.push(format!("{} g {} G", g, g)),
            PdfColor::Rgb(r, g, b) => {
                style_lines.push(format!("{} {} {} rg {} {} {} RG", r, g, b, r, g, b))
            }
            PdfColor::Cmyk(c, m, y, k) => style_lines.push(format!(
                "{} {} {} {} k {} {} {} {} K",
                c, m, y, k, c, m, y, k
            )),
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
    let has_subsequent = parsed.text_blocks[block_idx + 1..].iter().any(|b| {
        b.bt_start_line == target_block.bt_start_line && b.bt_end_line == target_block.bt_end_line
    });

    if has_subsequent {
        if font_name.is_some() || font_size.is_some() {
            let orig_font = target_block.style.font_name.as_deref().unwrap_or("F1");
            let orig_size = target_block
                .style
                .raw_font_size
                .or(target_block.style.font_size)
                .unwrap_or(12.0);
            restore_lines.push(format!("/{} {} Tf", orig_font, format_size(orig_size)));
        }
        if let Some(_color) = fill_color {
            if let Some(ref orig_color) = target_block.style.fill_color {
                match orig_color {
                    PdfColor::Gray(g) => restore_lines.push(format!("{} g {} G", g, g)),
                    PdfColor::Rgb(r, g, b) => {
                        restore_lines.push(format!("{} {} {} rg {} {} {} RG", r, g, b, r, g, b))
                    }
                    PdfColor::Cmyk(c, m, y, k) => restore_lines.push(format!(
                        "{} {} {} {} k {} {} {} {} K",
                        c, m, y, k, c, m, y, k
                    )),
                }
            }
        }
        if char_spacing.is_some() {
            restore_lines.push(format!("{} Tc", target_block.style.char_spacing));
        }
        if word_spacing.is_some() {
            restore_lines.push(format!("{} Tw", target_block.style.word_spacing));
        }
    }

    // Build the text Tj line — supports multi-font segments
    let effective_text = new_text
        .map(|s| s.to_string())
        .unwrap_or_else(|| target_block.text.clone());

    let mut missing: Vec<char> = Vec::new();
    let segments = pick_fonts_for_text(
        doc,
        page_id,
        Some(&effective_font),
        &effective_text,
        &mut missing,
    )?;
    if !missing.is_empty() {
        return Err(HandlerError::OperationFailed(format!(
            "characters not encodable in any page font: {}. Provide --prop fontFile=<path> or --prop font=<name> to override.",
            missing.iter().collect::<String>()
        )));
    }

    let new_tokens = build_segment_tokens(&segments, Some(&effective_font), effective_size);

    let mut final_tokens = Vec::new();
    final_tokens.extend(style_lines);
    final_tokens.extend(new_tokens);
    final_tokens.extend(restore_lines);

    let line = &modified_lines[target_block.text_line_index];
    let mut line_tokens = crate::content_stream::tokenize_pdf_line(line);

    if target_block.line_token_index < line_tokens.len() {
        let op_idx = target_block.line_token_index;
        let consume_extra = matches!(
            line_tokens.get(op_idx + 1).map(|s| s.as_str()),
            Some("Tj") | Some("TJ")
        );
        let end = if consume_extra {
            op_idx + 2
        } else {
            op_idx + 1
        };
        line_tokens.splice(op_idx..end, final_tokens);
        modified_lines[target_block.text_line_index] = line_tokens.join(" ");
    } else {
        modified_lines[target_block.text_line_index] = final_tokens.join(" ");
    }

    // Insert background-color rectangle BEFORE the BT block (outside text object)
    if let Some(bg) = bg_color {
        let bb = &target_block.user_bbox;
        let (r, g, b_val) = match bg {
            PdfColor::Gray(g) => (*g, *g, *g),
            PdfColor::Rgb(r, g, b) => (*r, *g, *b),
            PdfColor::Cmyk(c, m, y, k) => {
                // Approximate CMYK->RGB for bg rendering
                let r = (1.0 - c) * (1.0 - k);
                let g = (1.0 - m) * (1.0 - k);
                let b = (1.0 - y) * (1.0 - k);
                (r, g, b)
            }
        };
        let bg_lines = vec![
            "q".to_string(),
            format!("{} {} {} rg", r, g, b_val),
            format!("{} {} {} {} re", bb.x, bb.y, bb.width, bb.height),
            "f".to_string(),
            "Q".to_string(),
        ];

        let insert_pos = target_block.bt_start_line;
        let mut new_lines = modified_lines[..insert_pos].to_vec();
        for line in &bg_lines {
            new_lines.push(line.clone());
        }
        new_lines.extend_from_slice(&modified_lines[insert_pos..]);
        modified_lines = new_lines;
    }

    let modified_content = modified_lines.join("\n");
    write_content_to_page(doc, page_id, modified_content.as_bytes())?;
    Ok(())
}

fn write_content_to_page(
    doc: &mut LopdfDocument,
    page_id: ObjectId,
    content: &[u8],
) -> Result<(), HandlerError> {
    let content_ids = doc.get_page_contents(page_id);
    if content_ids.is_empty() {
        return Err(HandlerError::OperationFailed(
            "page has no content streams".to_string(),
        ));
    }

    // Write modified content to the first stream
    let first_id = content_ids[0];
    if let Ok(obj) = doc.get_object_mut(first_id) {
        if let lopdf::Object::Stream(stream) = obj {
            // Remove any existing compression filter first — the content bytes
            // we receive are already decompressed (lopdf transparently inflates
            // FlateDecode streams in get_page_content()). Setting raw bytes
            // while /Filter /FlateDecode remains in the dict causes blank pages
            // on the next load because lopdf tries to deflate raw data.
            stream.dict.remove(b"Filter");
            stream.content = content.to_vec();
            // Re-compress with FlateDecode so the saved PDF stays compact
            // and the /Filter + /Length are consistent.
            if stream.compress().is_err() {
                // Fallback: if compression fails, keep uncompressed but
                // update Length to match the raw content.
                stream
                    .dict
                    .set("Length", lopdf::Object::Integer(content.len() as i64));
            }
        }
    }

    // Clear subsequent streams to prevent duplicate content rendering and viewer corruption
    for &other_id in &content_ids[1..] {
        if let Ok(obj) = doc.get_object_mut(other_id) {
            if let lopdf::Object::Stream(stream) = obj {
                stream.dict.remove(b"Filter");
                stream.content = Vec::new();
                stream.dict.set("Length", lopdf::Object::Integer(0));
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
    let page_id = pages
        .get(&(page_num as u32))
        .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

    let content = doc
        .get_page_content(*page_id)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to get page content: {}", e)))?;

    let content_str = String::from_utf8_lossy(&content);
    let modified = blanket_replace_strings(doc, *page_id, &content_str, new_text)?;

    write_content_to_page(doc, *page_id, modified.as_bytes())?;
    Ok(())
}

fn blanket_replace_strings(
    doc: &LopdfDocument,
    page_id: ObjectId,
    stream: &str,
    new_text: &str,
) -> Result<String, HandlerError> {
    let mut result = String::new();
    let mut in_text_object = false;
    let mut active_font: Option<String> = None;
    let mut active_size: f32 = 1.0;

    for line in stream.lines() {
        let trimmed = line.trim();
        if trimmed == "BT" {
            in_text_object = true;
            result.push_str(line);
            result.push('\n');
            continue;
        }
        if trimmed == "ET" {
            in_text_object = false;
            result.push_str(line);
            result.push('\n');
            continue;
        }
        if !in_text_object {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        if trimmed.ends_with(" Tf") {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let font_name = parts[parts.len() - 3].trim_start_matches('/');
                active_font = Some(font_name.to_string());
                if let Ok(sz) = parts[parts.len() - 2].parse::<f32>() {
                    active_size = sz;
                }
            }
        }

        if trimmed.ends_with(" Tj") {
            let string_part = trimmed.trim_end_matches(" Tj").trim();
            if (string_part.starts_with('(') && string_part.ends_with(')'))
                || (string_part.starts_with('<') && string_part.ends_with('>'))
            {
                let mut missing = Vec::new();
                let segments = pick_fonts_for_text(
                    doc,
                    page_id,
                    active_font.as_deref(),
                    new_text,
                    &mut missing,
                )?;
                if !missing.is_empty() {
                    return Err(HandlerError::OperationFailed(format!(
                        "characters not encodable in any page font: {}",
                        missing.iter().collect::<String>()
                    )));
                }
                let tokens = build_segment_tokens(&segments, active_font.as_deref(), active_size);
                result.push_str(&tokens.join(" "));
                result.push('\n');
            } else {
                result.push_str(line);
                result.push('\n');
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }
    Ok(result)
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

/// Parse a text block path like /page[N]/text[M] into (page_num, text_index).
fn parse_text_block_path(path: &str) -> Option<(usize, usize)> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() != 2 {
        return None;
    }

    let page_part = parts[0];
    if !page_part.starts_with("page") {
        return None;
    }
    let page_num = page_part
        .strip_prefix("page[")
        .and_then(|s| s.strip_suffix("]"))
        .and_then(|s| s.parse::<usize>().ok())?;

    let text_part = parts[1];
    if !text_part.starts_with("text") {
        return None;
    }
    let text_index = text_part
        .strip_prefix("text[")
        .and_then(|s| s.strip_suffix("]"))
        .and_then(|s| s.parse::<usize>().ok())?;

    Some((page_num, text_index))
}

/// Apply foreground text colors to a specific character range of text blocks.
pub fn apply_range_text_colors(
    doc: &mut LopdfDocument,
    color: &PdfColor,
    segments: &[handler_common::PathRangeSegment],
) -> Result<(), HandlerError> {
    use std::collections::HashMap;

    // Helper to format color operators — sets BOTH fill (rg/g/k) and stroke (RG/G/K)
    // so that Tr=2 (fill+stroke) text also gets the target color.
    let format_color_op = |col: &PdfColor| -> String {
        match col {
            PdfColor::Gray(g) => format!("{} g {} G", g, g),
            PdfColor::Rgb(r, g, b) => format!("{} {} {} rg {} {} {} RG", r, g, b, r, g, b),
            PdfColor::Cmyk(c, m, y, k) => {
                format!("{} {} {} {} k {} {} {} {} K", c, m, y, k, c, m, y, k)
            }
        }
    };

    // Group segments by page
    let mut page_groups: HashMap<usize, Vec<handler_common::PathRangeSegment>> = HashMap::new();
    for seg in segments {
        if let Some((page_num, _)) = parse_text_block_path(&seg.path) {
            page_groups.entry(page_num).or_default().push(seg.clone());
        }
    }

    for (page_num, page_segs) in page_groups {
        let pages = doc.get_pages();
        let page_id = *pages
            .get(&(page_num as u32))
            .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

        let content = doc.get_page_content(page_id).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to get page content: {}", e))
        })?;

        let parsed = parse_page_content_stream(&content, page_id, doc).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to parse content stream: {}", e))
        })?;

        let mut modified_lines = parsed.lines.clone();

        for seg in page_segs {
            if let Some((_, text_index)) = parse_text_block_path(&seg.path) {
                let block_idx = text_index - 1;
                if block_idx >= parsed.text_blocks.len() {
                    return Err(HandlerError::PathNotFound(format!(
                        "text block {} not found on page {}",
                        text_index, page_num
                    )));
                }
                let block = &parsed.text_blocks[block_idx];

                let start = seg.start.unwrap_or(0);
                let char_count = block.text.chars().count();
                let end = seg.end.unwrap_or(char_count).min(char_count).max(start);

                let prefix_chars: String = block.text.chars().take(start).collect();
                let selected_chars: String =
                    block.text.chars().skip(start).take(end - start).collect();
                let suffix_chars: String = block.text.chars().skip(end).collect();

                let font_name = block.style.font_name.as_deref().unwrap_or("F1");

                let mut ops = Vec::new();

                if !prefix_chars.is_empty() {
                    let enc = crate::content_stream::encode_chunk_with_font(
                        doc,
                        page_id,
                        font_name,
                        &prefix_chars,
                    )?;
                    ops.push(format!("{} Tj", enc));
                }

                // Set new color
                ops.push(format_color_op(color));

                if !selected_chars.is_empty() {
                    let enc = crate::content_stream::encode_chunk_with_font(
                        doc,
                        page_id,
                        font_name,
                        &selected_chars,
                    )?;
                    ops.push(format!("{} Tj", enc));
                }

                // Restore original color
                let orig_color = block
                    .style
                    .fill_color
                    .clone()
                    .unwrap_or(PdfColor::Gray(0.0));
                ops.push(format_color_op(&orig_color));

                if !suffix_chars.is_empty() {
                    let enc = crate::content_stream::encode_chunk_with_font(
                        doc,
                        page_id,
                        font_name,
                        &suffix_chars,
                    )?;
                    ops.push(format!("{} Tj", enc));
                }

                // Splice ops into content stream
                let line = &modified_lines[block.text_line_index];
                let mut line_tokens = crate::content_stream::tokenize_pdf_line(line);

                if block.line_token_index < line_tokens.len() {
                    let op_idx = block.line_token_index;
                    let consume_extra = matches!(
                        line_tokens.get(op_idx + 1).map(|s| s.as_str()),
                        Some("Tj") | Some("TJ")
                    );
                    let end_token = if consume_extra {
                        op_idx + 2
                    } else {
                        op_idx + 1
                    };

                    let replacement = ops.join(" ");
                    line_tokens.splice(op_idx..end_token, vec![replacement]);
                    modified_lines[block.text_line_index] = line_tokens.join(" ");
                }
            }
        }

        // Save page content
        let new_content = modified_lines.join("\n").into_bytes();
        doc.change_page_content(page_id, new_content).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to save page content: {}", e))
        })?;
    }

    Ok(())
}

/// Apply native Highlight annotation for a cross-node text block range.
pub fn apply_range_highlights(
    doc: &mut LopdfDocument,
    color: &PdfColor,
    segments: &[handler_common::PathRangeSegment],
) -> Result<(), HandlerError> {
    use std::collections::HashMap;

    // Group segments by page
    let mut page_groups: HashMap<usize, Vec<handler_common::PathRangeSegment>> = HashMap::new();
    for seg in segments {
        if let Some((page_num, _)) = parse_text_block_path(&seg.path) {
            page_groups.entry(page_num).or_default().push(seg.clone());
        }
    }

    for (page_num, page_segs) in page_groups {
        let pages = doc.get_pages();
        let page_id = *pages
            .get(&(page_num as u32))
            .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

        let content = doc.get_page_content(page_id).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to get page content: {}", e))
        })?;

        let parsed = parse_page_content_stream(&content, page_id, doc).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to parse content stream: {}", e))
        })?;

        let mut rects = Vec::new();

        for seg in page_segs {
            if let Some((_, text_index)) = parse_text_block_path(&seg.path) {
                let block_idx = text_index - 1;
                if block_idx >= parsed.text_blocks.len() {
                    return Err(HandlerError::PathNotFound(format!(
                        "text block {} not found on page {}",
                        text_index, page_num
                    )));
                }
                let block = &parsed.text_blocks[block_idx];

                // Calculate sub-bounding boxes
                let start = seg.start.unwrap_or(0);
                let end = seg.end.unwrap_or(block.text.chars().count());

                // Safety checks for indices
                let char_count = block.text.chars().count();
                let start = start.min(char_count);
                let end = end.min(char_count).max(start);

                let font_name = block.style.font_name.as_deref().unwrap_or("F1");
                let font_info = parsed.font_map.get(font_name);

                let (sub_bbox_x, sub_bbox_width) = if start == 0 && end == char_count {
                    // Full highlight
                    (block.bbox.x, block.bbox.width)
                } else if let Some(fi) = font_info {
                    let font_size = block.style.font_size.unwrap_or(12.0);
                    let char_spacing = block.style.char_spacing;
                    let word_spacing = block.style.word_spacing;

                    // Prefix width
                    let prefix_chars: String = block.text.chars().take(start).collect();
                    let prefix_width = crate::content_stream::estimate_text_width(
                        &prefix_chars,
                        fi,
                        font_size,
                        char_spacing,
                        word_spacing,
                    );

                    // Selected width
                    let selected_chars: String =
                        block.text.chars().skip(start).take(end - start).collect();
                    let selected_width = crate::content_stream::estimate_text_width(
                        &selected_chars,
                        fi,
                        font_size,
                        char_spacing,
                        word_spacing,
                    );

                    (block.bbox.x + prefix_width, selected_width)
                } else {
                    // Fallback to proportional split
                    let ratio_start = start as f32 / char_count as f32;
                    let ratio_end = end as f32 / char_count as f32;
                    let prefix_width = block.bbox.width * ratio_start;
                    let selected_width = block.bbox.width * (ratio_end - ratio_start);
                    (block.bbox.x + prefix_width, selected_width)
                };

                eprintln!(
                    "[DEBUG highlight] block.bbox=({},{},{},{}), sub_bbox_x={}, sub_bbox_width={}",
                    block.bbox.x,
                    block.bbox.y,
                    block.bbox.width,
                    block.bbox.height,
                    sub_bbox_x,
                    sub_bbox_width
                );
                rects.push(crate::content_stream::BBox {
                    x: sub_bbox_x,
                    y: block.bbox.y,
                    width: sub_bbox_width,
                    height: block.bbox.height,
                });
            }
        }

        if rects.is_empty() {
            continue;
        }

        // Add Native Highlight Annotation to PDF page dictionary
        let mut annot_dict = lopdf::Dictionary::new();
        annot_dict.set("Type", lopdf::Object::Name(b"Annot".to_vec()));
        annot_dict.set("Subtype", lopdf::Object::Name(b"Highlight".to_vec()));

        let mut x_min = f32::MAX;
        let mut y_min = f32::MAX;
        let mut x_max = f32::MIN;
        let mut y_max = f32::MIN;

        let mut quad_points = Vec::new();
        for rect in &rects {
            x_min = x_min.min(rect.x);
            y_min = y_min.min(rect.y);
            x_max = x_max.max(rect.x + rect.width);
            y_max = y_max.max(rect.y + rect.height);

            // QuadPoints: top-left, top-right, bottom-left, bottom-right
            let x_tl = rect.x;
            let y_tl = rect.y + rect.height;
            let x_tr = rect.x + rect.width;
            let y_tr = rect.y + rect.height;
            let x_bl = rect.x;
            let y_bl = rect.y;
            let x_br = rect.x + rect.width;
            let y_br = rect.y;

            // Standard PDF Spec QuadPoints order: top-left, top-right, bottom-left, bottom-right
            quad_points.push(lopdf::Object::Real(x_tl as f32));
            quad_points.push(lopdf::Object::Real(y_tl as f32));
            quad_points.push(lopdf::Object::Real(x_tr as f32));
            quad_points.push(lopdf::Object::Real(y_tr as f32));
            quad_points.push(lopdf::Object::Real(x_bl as f32));
            quad_points.push(lopdf::Object::Real(y_bl as f32));
            quad_points.push(lopdf::Object::Real(x_br as f32));
            quad_points.push(lopdf::Object::Real(y_br as f32));
        }

        annot_dict.set(
            "Rect",
            lopdf::Object::Array(vec![
                lopdf::Object::Real(x_min as f32),
                lopdf::Object::Real(y_min as f32),
                lopdf::Object::Real(x_max as f32),
                lopdf::Object::Real(y_max as f32),
            ]),
        );
        annot_dict.set("QuadPoints", lopdf::Object::Array(quad_points));

        let (r, g, b) = match color {
            PdfColor::Gray(gray) => (*gray, *gray, *gray),
            PdfColor::Rgb(r, g, b) => (*r, *g, *b),
            PdfColor::Cmyk(c, m, y, k) => {
                let r = (1.0 - c) * (1.0 - k);
                let g = (1.0 - m) * (1.0 - k);
                let b = (1.0 - y) * (1.0 - k);
                (r, g, b)
            }
        };
        annot_dict.set(
            "C",
            lopdf::Object::Array(vec![
                lopdf::Object::Real(r as f32),
                lopdf::Object::Real(g as f32),
                lopdf::Object::Real(b as f32),
            ]),
        );

        // 1. Check if "Annots" exists on the page (immutable borrow of doc)
        let mut has_annots = false;
        let mut is_reference = None;
        let mut inline_array = None;

        if let Ok(page_dict) = doc.get_dictionary(page_id) {
            if let Ok(obj) = page_dict.get(b"Annots") {
                has_annots = true;
                match obj {
                    lopdf::Object::Reference(ref_id) => {
                        is_reference = Some(*ref_id);
                    }
                    lopdf::Object::Array(arr) => {
                        inline_array = Some(arr.clone());
                    }
                    _ => {}
                }
            }
        }

        // 2. Add the annotation object (mutable borrow of doc)
        let annot_id = doc.add_object(lopdf::Object::Dictionary(annot_dict));

        // 3. Insert annotation ID into Annots array
        if has_annots {
            if let Some(ref_id) = is_reference {
                if let Ok(lopdf::Object::Array(ref mut arr)) = doc.get_object_mut(ref_id) {
                    arr.push(lopdf::Object::Reference(annot_id));
                }
            } else if let Some(mut arr) = inline_array {
                arr.push(lopdf::Object::Reference(annot_id));
                if let Ok(page_dict) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
                    page_dict.set("Annots", lopdf::Object::Array(arr));
                }
            }
        } else {
            let arr = vec![lopdf::Object::Reference(annot_id)];
            let arr_id = doc.add_object(lopdf::Object::Array(arr));
            if let Ok(page_dict) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
                page_dict.set("Annots", lopdf::Object::Reference(arr_id));
            }
        }
    }

    Ok(())
}
