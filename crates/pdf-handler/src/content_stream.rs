use std::collections::HashMap;
use handler_common::HandlerError;
use lopdf::{Document as LopdfDocument, ObjectId, Object, Dictionary};

/// Bounding box for a text block in PDF coordinate space.
/// PDF origin is bottom-left, y increases upward.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// PDF color representation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PdfColor {
    Gray(f32),
    Rgb(f32, f32, f32),
    Cmyk(f32, f32, f32, f32),
}

/// Style properties extracted from PDF operators for a text block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TextStyle {
    pub font_name: Option<String>,
    pub font_size: Option<f32>,
    pub fill_color: Option<PdfColor>,
    pub char_spacing: f32,
    pub word_spacing: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_name: None,
            font_size: None,
            fill_color: None,
            char_spacing: 0.0,
            word_spacing: 0.0,
        }
    }
}

/// A structured text block extracted from a BT...ET section.
#[derive(Debug, Clone)]
pub struct PdfTextBlock {
    /// 1-based index within the page, corresponding to /page[N]/text[M]
    pub index: usize,
    /// Decoded text content
    pub text: String,
    /// Bounding box computed from Tm position + text width + font size
    pub bbox: BBox,
    /// Style properties active at the time of text rendering
    pub style: TextStyle,
    /// Starting line index of the BT section in the raw content stream
    pub bt_start_line: usize,
    /// Ending line index of the ET section
    pub bt_end_line: usize,
    /// Line index that contains the Tj/TJ string
    pub text_line_index: usize,
    /// Whether the text comes from TJ (array with kerning) or Tj (simple)
    pub is_array_text: bool,
}

/// Font info extracted from the page's /Resources /Font dictionary.
#[derive(Debug, Clone)]
pub struct FontInfo {
    pub pdf_name: String,
    pub base_font: Option<String>,
    pub is_cid_font: bool,
    pub char_widths: HashMap<u32, f32>,
    pub default_width: f32,
}

/// Parsed content stream for a page — tracks line-level positions for modification.
#[derive(Debug, Clone)]
pub struct ParsedContentStream {
    /// Raw content stream lines (for targeted modification)
    pub lines: Vec<String>,
    /// Text blocks extracted from BT...ET sections
    pub text_blocks: Vec<PdfTextBlock>,
    /// Font name -> FontInfo
    pub font_map: HashMap<String, FontInfo>,
}

/// Estimate text width using font metrics.
pub fn estimate_text_width(
    text: &str,
    font_info: &FontInfo,
    font_size: f32,
    char_spacing: f32,
    word_spacing: f32,
) -> f32 {
    let mut total_width_units = 0.0;
    let mut space_count = 0;
    let char_count = text.chars().count();

    for byte in text.bytes() {
        let w = font_info.char_widths
            .get(&(byte as u32))
            .copied()
            .unwrap_or(font_info.default_width);
        total_width_units += w;
        if byte == 32 { space_count += 1; }
    }

    let base_width = total_width_units * font_size / 1000.0;
    let spacing_width = char_spacing * (char_count.saturating_sub(1) as f32);
    let word_spacing_width = word_spacing * (space_count as f32);
    base_width + spacing_width + word_spacing_width
}

fn standard_font_avg_width(font_name: &str) -> f32 {
    match font_name {
        n if n.contains("Helvetica") || n.contains("Arial") => 580.0,
        n if n.contains("Times") => 500.0,
        n if n.contains("Courier") => 600.0,
        n if n.contains("Symbol") => 580.0,
        _ => 500.0,
    }
}

/// Extract font dictionaries from a page's /Resources.
fn extract_page_fonts(doc: &LopdfDocument, page_id: ObjectId) -> HashMap<String, FontInfo> {
    let mut font_map = HashMap::new();

    if let Ok((resources_dict, _parent_chain)) = doc.get_page_resources(page_id) {
        if let Some(resources) = resources_dict {
            if let Ok(font_dict) = resources.get(b"Font") {
                if let Object::Dictionary(dict) = font_dict {
                    for (name, value) in dict.iter() {
                        let pdf_name = String::from_utf8_lossy(name).to_string();
                        if let Ok((_, font_obj)) = doc.dereference(value) {
                            if let Object::Dictionary(font_dict) = font_obj {
                                let info = build_font_info(doc, font_dict, &pdf_name);
                                font_map.insert(pdf_name, info);
                            }
                        }
                    }
                }
            }
        }
    }

    font_map
}

fn build_font_info(doc: &LopdfDocument, font_dict: &Dictionary, pdf_name: &str) -> FontInfo {
    let base_font = font_dict.get(b"BaseFont")
        .ok()
        .and_then(|v| v.as_name_str().ok())
        .map(|s| s.to_string());

    let is_cid = font_dict.get(b"Subtype")
        .ok()
        .and_then(|v| v.as_name_str().ok())
        .map(|s| s == "Type0")
        .unwrap_or(false);

    let (char_widths, default_width) = extract_font_widths(doc, font_dict, &base_font, is_cid);

    FontInfo {
        pdf_name: pdf_name.to_string(),
        base_font,
        is_cid_font: is_cid,
        char_widths,
        default_width,
    }
}

fn extract_font_widths(
    doc: &LopdfDocument,
    font_dict: &Dictionary,
    base_font: &Option<String>,
    is_cid: bool,
) -> (HashMap<u32, f32>, f32) {
    let default_width = base_font
        .as_ref()
        .map(|n| standard_font_avg_width(n))
        .unwrap_or(500.0);

    let mut widths = HashMap::new();

    if is_cid {
        let dw = font_dict.get(b"DW")
            .ok()
            .and_then(|v| v.as_float().ok().or(v.as_i64().ok().map(|i| i as f32)))
            .unwrap_or(1000.0);

        if let Ok(w_obj) = font_dict.get(b"W") {
            if let Ok((_, resolved)) = doc.dereference(w_obj) {
                if let Object::Array(arr) = resolved {
                    parse_cid_width_array(&arr, &mut widths);
                }
            }
        }
        (widths, dw)
    } else {
        let first_char = font_dict.get(b"FirstChar")
            .ok()
            .and_then(|v| v.as_i64().ok())
            .unwrap_or(0) as u32;

        if let Ok(w_obj) = font_dict.get(b"Widths") {
            if let Ok((_, resolved)) = doc.dereference(w_obj) {
                if let Object::Array(arr) = resolved {
                    for (i, obj) in arr.iter().enumerate() {
                        let w = obj.as_float().ok()
                            .or(obj.as_i64().ok().map(|v| v as f32))
                            .unwrap_or(default_width);
                        widths.insert(first_char + i as u32, w);
                    }
                }
            }
        }
        (widths, default_width)
    }
}

fn parse_cid_width_array(arr: &[Object], widths: &mut HashMap<u32, f32>) {
    let mut i = 0;
    while i < arr.len() {
        if let Some(start) = arr[i].as_i64().ok() {
            i += 1;
            if i >= arr.len() { break; }
            if let Object::Array(sub_arr) = &arr[i] {
                for (j, obj) in sub_arr.iter().enumerate() {
                    let w = obj.as_float().ok()
                        .or(obj.as_i64().ok().map(|v| v as f32))
                        .unwrap_or(600.0);
                    widths.insert(start as u32 + j as u32, w);
                }
                i += 1;
            } else if let Some(end) = arr[i].as_i64().ok() {
                i += 1;
                if i >= arr.len() { break; }
                let w = arr[i].as_float().ok()
                    .or(arr[i].as_i64().ok().map(|v| v as f32))
                    .unwrap_or(600.0);
                for cid in start..=end {
                    widths.insert(cid as u32, w);
                }
                i += 1;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
}

// --- String extraction utilities (reused from reader.rs) ---

/// Extract a PDF string literal: (Hello World) -> "Hello World"
fn extract_pdf_string(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len()-1];
        let mut result = String::new();
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('r') => result.push('\r'),
                    Some('t') => result.push('\t'),
                    Some('b') => result.push('\u{08}'),
                    Some('f') => result.push('\u{0C}'),
                    Some('(') => result.push('('),
                    Some(')') => result.push(')'),
                    Some('\\') => result.push('\\'),
                    Some(d) if d.is_ascii_digit() => {
                        let mut octal = String::from(d);
                        for _ in 0..2 {
                            if let Some(&next) = chars.peek() {
                                if next.is_ascii_digit() { octal.push(chars.next().unwrap()); }
                                else { break; }
                            }
                        }
                        if let Ok(code) = u8::from_str_radix(&octal, 8) { result.push(code as char); }
                    }
                    Some(other) => result.push(other),
                    None => result.push('\\'),
                }
            } else { result.push(c); }
        }
        Some(result)
    } else if s.starts_with('<') && s.ends_with('>') {
        Some(decode_hex_string(&s[1..s.len()-1]))
    } else {
        None
    }
}

/// Extract text from a PDF array: [(Hello)-5(World)] TJ
fn extract_pdf_array_text(s: &str) -> Option<String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') { return None; }

    let inner = &s[1..s.len()-1];
    let bytes = inner.as_bytes();
    let mut result = String::new();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '(' {
            let mut depth = 1;
            let start = i + 1;
            i += 1;
            while i < bytes.len() && depth > 0 {
                let bc = bytes[i] as char;
                if bc == '(' && (i == 0 || bytes[i-1] as char != '\\') { depth += 1; }
                else if bc == ')' && (i == 0 || bytes[i-1] as char != '\\') { depth -= 1; }
                i += 1;
            }
            let string_content = std::str::from_utf8(&bytes[start..i-1]).unwrap_or("");
            if let Some(extracted) = extract_pdf_string(&format!("({})", string_content)) {
                result.push_str(&extracted);
            }
        } else if c == '<' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] as char != '>' { i += 1; }
            let hex_content = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            result.push_str(&decode_hex_string(hex_content));
            i += 1;
        } else if c.is_ascii_digit() || c == '-' || c == '.' {
            i += 1;
            while i < bytes.len() {
                let bc = bytes[i] as char;
                if bc.is_ascii_digit() || bc == '.' || bc == '-' { i += 1; }
                else { break; }
            }
        } else { i += 1; }
    }
    Some(result)
}

fn decode_hex_string(hex: &str) -> String {
    let hex = hex.trim();
    let mut result = String::new();
    let mut i = 0;
    while i + 2 <= hex.len() {
        if let Ok(byte) = u8::from_str_radix(&hex[i..i+2], 16) { result.push(byte as char); }
        i += 2;
    }
    result
}

/// Encode a text string as a PDF literal string.
pub fn encode_pdf_string(text: &str) -> String {
    let mut escaped = String::new();
    escaped.push('(');
    for c in text.chars() {
        match c {
            '(' => escaped.push_str("\\("),
            ')' => escaped.push_str("\\)"),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    escaped.push(')');
    escaped
}

/// Parse a numeric value from a PDF content stream operand string.
fn parse_float(s: &str) -> f32 {
    s.trim().parse::<f32>().unwrap_or(0.0)
}

/// Extract the operand portion before the operator from a content stream line.
/// For example: "72 0 0 72 100 200 Tm" -> operands ["72","0","0","72","100","200"], operator "Tm"
fn split_line_into_operands_and_operator(line: &str) -> (Vec<String>, String) {
    let line = line.trim();
    // Operators are alphabetic tokens (possibly with * or ')
    // Find the last token that looks like an operator
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.is_empty() { return (Vec::new(), String::new()); }

    // The operator is the last token that is purely alphabetic (+ * ')
    // Find the rightmost operator token
    let mut op_idx = None;
    for (i, token) in tokens.iter().enumerate().rev() {
        if token.chars().all(|c| c.is_alphabetic() || c == '*' || c == '\'') && !token.is_empty() {
            op_idx = Some(i);
            break;
        }
    }

    if let Some(idx) = op_idx {
        let operands = tokens[..idx].iter().map(|s| s.to_string()).collect();
        let operator = tokens[idx].to_string();
        (operands, operator)
    } else {
        // No operator found — treat entire line as operands
        (tokens.iter().map(|s| s.to_string()).collect(), String::new())
    }
}

/// Text state machine for tracking position, font, and style during content stream parsing.
/// line_x/line_y track the text line start position (Td offsets are relative to this).
/// cursor_x tracks the rendering cursor position (advances after Tj/TJ by text width).
/// bbox uses line_x/line_y since text blocks start at the line origin.
struct TextState {
    line_x: f32,
    line_y: f32,
    cursor_x: f32,
    font_name: Option<String>,
    font_size: f32,
    char_spacing: f32,
    word_spacing: f32,
    fill_color: Option<PdfColor>,
    in_bt: bool,
    bt_start_line: usize,
    tm_set: bool,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            line_x: 0.0,
            line_y: 0.0,
            cursor_x: 0.0,
            font_name: None,
            font_size: 12.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            fill_color: None,
            in_bt: false,
            bt_start_line: 0,
            tm_set: false,
        }
    }
}

/// Parse a page's content stream bytes into a ParsedContentStream.
/// Uses line-by-line parsing (since lopdf::parser::content is private).
pub fn parse_page_content_stream(
    content_bytes: &[u8],
    page_id: ObjectId,
    doc: &LopdfDocument,
) -> Result<ParsedContentStream, HandlerError> {
    // Step 1: Split content into lines
    let content_str = String::from_utf8_lossy(content_bytes);
    let lines: Vec<String> = content_str.lines().map(|l| l.to_string()).collect();

    // Step 2: Extract font info
    let font_map = extract_page_fonts(doc, page_id);

    // Step 3: Walk lines, build text blocks
    let mut state = TextState::default();
    let mut text_blocks = Vec::new();
    let mut block_counter = 0usize;

    // Track BT/ET pairs to fill in bt_end_line
    let mut bt_stack: Vec<usize> = Vec::new();

    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let (operands, operator) = split_line_into_operands_and_operator(trimmed);

        match operator.as_str() {
            "BT" => {
                state.in_bt = true;
                state.bt_start_line = line_idx;
                state.tm_set = false;
                state.line_x = 0.0;
                state.line_y = 0.0;
                state.cursor_x = 0.0;
                bt_stack.push(line_idx);
            }
            "ET" => {
                state.in_bt = false;
            }
            "Tm" => {
                // Text matrix: a b c d e f — operands[4]=e(x), operands[5]=f(y)
                if operands.len() == 6 {
                    state.line_x = parse_float(&operands[4]);
                    state.line_y = parse_float(&operands[5]);
                    state.cursor_x = state.line_x;
                    state.tm_set = true;
                }
            }
            "Td" | "TD" => {
                // Td moves relative to line start, NOT cursor position
                if operands.len() == 2 {
                    let dx = parse_float(&operands[0]);
                    let dy = parse_float(&operands[1]);
                    if state.tm_set {
                        state.line_x += dx;
                        state.line_y += dy;
                    } else {
                        state.line_x = dx;
                        state.line_y = dy;
                        state.tm_set = true;
                    }
                    state.cursor_x = state.line_x;
                }
            }
            "T*" => {
                // Move to start of next line (offset by -font_size in y)
                state.line_y -= state.font_size;
                state.cursor_x = state.line_x;
            }
            "Tf" => {
                if operands.len() >= 2 {
                    // Font name operand may be /Name format
                    let font_name_raw = operands[0].trim();
                    let font_name = if font_name_raw.starts_with('/') {
                        font_name_raw[1..].to_string()
                    } else {
                        font_name_raw.to_string()
                    };
                    state.font_name = Some(font_name);
                    state.font_size = parse_float(&operands[1]);
                }
            }
            "Tc" => {
                if !operands.is_empty() { state.char_spacing = parse_float(&operands[0]); }
            }
            "Tw" => {
                if !operands.is_empty() { state.word_spacing = parse_float(&operands[0]); }
            }
            "rg" => {
                if operands.len() == 3 {
                    state.fill_color = Some(PdfColor::Rgb(
                        parse_float(&operands[0]),
                        parse_float(&operands[1]),
                        parse_float(&operands[2]),
                    ));
                }
            }
            "g" => {
                if !operands.is_empty() {
                    state.fill_color = Some(PdfColor::Gray(parse_float(&operands[0])));
                }
            }
            "k" => {
                if operands.len() == 4 {
                    state.fill_color = Some(PdfColor::Cmyk(
                        parse_float(&operands[0]),
                        parse_float(&operands[1]),
                        parse_float(&operands[2]),
                        parse_float(&operands[3]),
                    ));
                }
            }
            "Tj" => {
                if state.in_bt && !trimmed.is_empty() {
                    let string_part = trimmed.trim_end_matches("Tj").trim();
                    if let Some(text) = extract_pdf_string(string_part) {
                        if !text.is_empty() {
                            block_counter += 1;
                            let (width, height) = compute_block_dimensions(
                                &text, &font_map, &state
                            );
                            text_blocks.push(PdfTextBlock {
                                index: block_counter,
                                text,
                                bbox: BBox { x: state.cursor_x, y: state.line_y, width, height },
                                style: TextStyle {
                                    font_name: state.font_name.clone(),
                                    font_size: Some(state.font_size),
                                    fill_color: state.fill_color.clone(),
                                    char_spacing: state.char_spacing,
                                    word_spacing: state.word_spacing,
                                },
                                bt_start_line: state.bt_start_line,
                                bt_end_line: line_idx,
                                text_line_index: line_idx,
                                is_array_text: false,
                            });
                            state.cursor_x += width;
                        }
                    }
                }
            }
            "TJ" => {
                if state.in_bt && !trimmed.is_empty() {
                    let array_part = trimmed.trim_end_matches("TJ").trim();
                    if let Some(text) = extract_pdf_array_text(array_part) {
                        if !text.is_empty() {
                            block_counter += 1;
                            let (width, height) = compute_block_dimensions(
                                &text, &font_map, &state
                            );
                            text_blocks.push(PdfTextBlock {
                                index: block_counter,
                                text,
                                bbox: BBox { x: state.cursor_x, y: state.line_y, width, height },
                                style: TextStyle {
                                    font_name: state.font_name.clone(),
                                    font_size: Some(state.font_size),
                                    fill_color: state.fill_color.clone(),
                                    char_spacing: state.char_spacing,
                                    word_spacing: state.word_spacing,
                                },
                                bt_start_line: state.bt_start_line,
                                bt_end_line: line_idx,
                                text_line_index: line_idx,
                                is_array_text: true,
                            });
                            state.cursor_x += width;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Update bt_end_line — find the ET line for each BT section
    let mut bt_et_pairs: Vec<(usize, usize)> = Vec::new();
    let mut bt_stack: Vec<usize> = Vec::new();
    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "BT" {
            bt_stack.push(line_idx);
        } else if trimmed == "ET" {
            if let Some(bt_start) = bt_stack.pop() {
                bt_et_pairs.push((bt_start, line_idx));
            }
        }
    }

    for block in &mut text_blocks {
        for (bt_start, bt_end) in &bt_et_pairs {
            if block.bt_start_line == *bt_start {
                block.bt_end_line = *bt_end;
                break;
            }
        }
    }

    Ok(ParsedContentStream {
        lines,
        text_blocks,
        font_map,
    })
}

fn compute_block_dimensions(
    text: &str,
    font_map: &HashMap<String, FontInfo>,
    state: &TextState,
) -> (f32, f32) {
    let height = state.font_size;
    let width = if let Some(ref font_name) = state.font_name {
        let font_info = font_map.get(font_name)
            .cloned()
            .unwrap_or_else(|| FontInfo {
                pdf_name: font_name.clone(),
                base_font: None,
                is_cid_font: false,
                char_widths: HashMap::new(),
                default_width: standard_font_avg_width(font_name),
            });
        estimate_text_width(text, &font_info, state.font_size, state.char_spacing, state.word_spacing)
    } else {
        text.chars().count() as f32 * state.font_size * 0.5
    };
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_pdf_string() {
        assert_eq!(extract_pdf_string("(Hello World)"), Some("Hello World".to_string()));
        assert_eq!(extract_pdf_string("(Hello\\nWorld)"), Some("Hello\nWorld".to_string()));
        assert_eq!(extract_pdf_string("(Hello\\(World\\))"), Some("Hello(World)".to_string()));
    }

    #[test]
    fn test_extract_pdf_array_text() {
        assert_eq!(
            extract_pdf_array_text("[(Hello)5(World)]"),
            Some("HelloWorld".to_string())
        );
    }

    #[test]
    fn test_encode_pdf_string() {
        let encoded = encode_pdf_string("Hello (World)");
        assert_eq!(encoded, "(Hello \\(World\\))");
    }

    #[test]
    fn test_estimate_text_width() {
        let font_info = FontInfo {
            pdf_name: "F1".to_string(),
            base_font: Some("Helvetica".to_string()),
            is_cid_font: false,
            char_widths: HashMap::new(),
            default_width: 580.0,
        };
        let width = estimate_text_width("Hello", &font_info, 12.0, 0.0, 0.0);
        assert!(width > 30.0 && width < 40.0);
    }

    #[test]
    fn test_split_line() {
        let (operands, op) = split_line_into_operands_and_operator("72 0 0 72 100 200 Tm");
        assert_eq!(op, "Tm");
        assert_eq!(operands.len(), 6);

        let (operands, op) = split_line_into_operands_and_operator("/F1 12 Tf");
        assert_eq!(op, "Tf");
        assert_eq!(operands, vec!["/F1", "12"]);

        let (operands, op) = split_line_into_operands_and_operator("(Hello) Tj");
        assert_eq!(op, "Tj");
        assert_eq!(operands, vec!["(Hello)"]);
    }
}