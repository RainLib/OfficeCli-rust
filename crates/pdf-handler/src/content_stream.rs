use std::collections::{HashMap, HashSet};
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

/// Filled rectangle drawn in the graphics state (background highlight).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilledRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: PdfColor,
}

/// Style properties extracted from PDF operators for a text block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TextStyle {
    pub font_name: Option<String>,
    /// Effective rendered font size = raw Tf operand * |tm_d|. Used for bbox /
    /// `get` output and same-size merging.
    pub font_size: Option<f32>,
    /// Raw Tf operand (without Tm matrix scaling). Must be used when re-emitting
    /// Tf operators in modified content so Tm scaling is not applied twice.
    pub raw_font_size: Option<f32>,
    pub fill_color: Option<PdfColor>,
    pub char_spacing: f32,
    pub word_spacing: f32,
    pub bg_color: Option<PdfColor>,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            font_name: None,
            font_size: None,
            raw_font_size: None,
            fill_color: None,
            char_spacing: 0.0,
            word_spacing: 0.0,
            bg_color: None,
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
    /// Bounding box computed in raw user space before CTM is applied
    pub user_bbox: BBox,
    /// Style properties active at the time of text rendering
    pub style: TextStyle,
    /// Starting line index of the BT section in the raw content stream
    pub bt_start_line: usize,
    /// Ending line index of the ET section
    pub bt_end_line: usize,
    /// Line index that contains the Tj/TJ string
    pub text_line_index: usize,
    /// The index of the string/array operand token in the line's tokens list
    pub line_token_index: usize,
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
    pub unicode_to_cid: HashMap<u32, u32>,
}

/// A structured image block extracted from a page's content stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PdfImageBlock {
    /// 1-based index within the page, corresponding to /page[N]/image[M]
    pub index: usize,
    /// Bounding box computed from active CTM matrix at Do operator time
    pub bbox: BBox,
    /// The name of the XObject resource (e.g. "Im1")
    pub xobject_name: String,
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
    /// Image blocks extracted from page content stream
    pub image_blocks: Vec<PdfImageBlock>,
    /// Maps XObject name -> Base64 Data URI string
    pub image_map: HashMap<String, String>,
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

    for c in text.chars() {
        let mut code = c as u32;
        if font_info.is_cid_font {
            if let Some(&cid) = font_info.unicode_to_cid.get(&code) {
                code = cid;
            }
        }
        let w = font_info.char_widths
            .get(&code)
            .copied()
            .unwrap_or_else(|| {
                if font_info.is_cid_font && c.is_ascii() {
                    // Standard ASCII characters rendered in CJK CID fonts default to 1000.0 in PDF,
                    // but render at a standard Roman width (~500.0 units) in browsers.
                    500.0
                } else {
                    font_info.default_width
                }
            });
        total_width_units += w;
        if c == ' ' { space_count += 1; }
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

    let mut unicode_to_cid = HashMap::new();
    if let Ok(to_unicode) = font_dict.get(b"ToUnicode") {
        if let Ok(ref_id) = to_unicode.as_reference() {
            if let Ok(Object::Stream(stream)) = doc.get_object(ref_id) {
                let content = String::from_utf8_lossy(&stream.content);
                let cmap = parse_to_unicode_cmap(&content);
                for (cid, unicode_str) in cmap {
                    if let Some(ch) = unicode_str.chars().next() {
                        unicode_to_cid.insert(ch as u32, cid);
                    }
                }
            }
        }
    }

    FontInfo {
        pdf_name: pdf_name.to_string(),
        base_font,
        is_cid_font: is_cid,
        char_widths,
        default_width,
        unicode_to_cid,
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

/// Extract raw bytes from a PDF string literal or hex string.
fn extract_pdf_string_bytes(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len()-1];
        let mut result = Vec::new();
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => result.push(b'\n'),
                    Some('r') => result.push(b'\r'),
                    Some('t') => result.push(b'\t'),
                    Some('b') => result.push(0x08),
                    Some('f') => result.push(0x0C),
                    Some('(') => result.push(b'('),
                    Some(')') => result.push(b')'),
                    Some('\\') => result.push(b'\\'),
                    Some(d) if d.is_ascii_digit() => {
                        let mut octal = String::from(d);
                        for _ in 0..2 {
                            if let Some(&next) = chars.peek() {
                                if next.is_ascii_digit() { octal.push(chars.next().unwrap()); }
                                else { break; }
                            }
                        }
                        if let Ok(code) = u8::from_str_radix(&octal, 8) { result.push(code); }
                    }
                    Some(other) => {
                        let mut buf = [0; 4];
                        for &byte in other.encode_utf8(&mut buf).as_bytes() {
                            result.push(byte);
                        }
                    }
                    None => result.push(b'\\'),
                }
            } else {
                let mut buf = [0; 4];
                for &byte in c.encode_utf8(&mut buf).as_bytes() {
                    result.push(byte);
                }
            }
        }
        Some(result)
    } else if s.starts_with('<') && s.ends_with('>') {
        Some(decode_hex_string_bytes(&s[1..s.len()-1]))
    } else {
        None
    }
}

fn decode_hex_string_bytes(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    let mut result = Vec::new();
    let mut i = 0;
    while i + 2 <= hex.len() {
        if let Ok(byte) = u8::from_str_radix(&hex[i..i+2], 16) {
            result.push(byte);
        }
        i += 2;
    }
    result
}

pub fn parse_to_unicode_cmap(cmap_str: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let mut in_bfchar = false;
    let mut in_bfrange = false;

    for line in cmap_str.lines() {
        let trimmed = line.trim();
        if trimmed.contains("beginbfchar") {
            in_bfchar = true;
            continue;
        }
        if trimmed.contains("endbfchar") {
            in_bfchar = false;
            continue;
        }
        if trimmed.contains("beginbfrange") {
            in_bfrange = true;
            continue;
        }
        if trimmed.contains("endbfrange") {
            in_bfrange = false;
            continue;
        }

        if in_bfchar {
            let parts: Vec<&str> = trimmed.split('<')
                .filter_map(|p| p.split('>').next())
                .filter(|p| !p.is_empty())
                .collect();
            if parts.len() >= 2 {
                if let Ok(cid) = u32::from_str_radix(parts[0], 16) {
                    if let Some(unicode_str) = hex_to_string(parts[1]) {
                        map.insert(cid, unicode_str);
                    }
                }
            }
        }

        if in_bfrange {
            let has_array = trimmed.contains('[');
            let parts: Vec<&str> = trimmed.split('<')
                .filter_map(|p| p.split('>').next())
                .filter(|p| !p.is_empty())
                .collect();
            if parts.len() >= 3 {
                if let (Ok(start), Ok(end)) = (u32::from_str_radix(parts[0], 16), u32::from_str_radix(parts[1], 16)) {
                    if has_array {
                        for (idx, hex_val) in parts[2..].iter().enumerate() {
                            let cid = start + idx as u32;
                            if cid <= end {
                                if let Some(unicode_str) = hex_to_string(hex_val) {
                                    map.insert(cid, unicode_str);
                                }
                            }
                        }
                    } else {
                        if let Ok(start_unicode) = u32::from_str_radix(parts[2], 16) {
                            for offset in 0..=(end - start) {
                                let cid = start + offset;
                                let unicode_val = start_unicode + offset;
                                if let Some(ch) = std::char::from_u32(unicode_val) {
                                    map.insert(cid, ch.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    map
}

fn hex_to_string(hex: &str) -> Option<String> {
    let mut u16_chars = Vec::new();
    let mut i = 0;
    while i + 4 <= hex.len() {
        if let Ok(val) = u16::from_str_radix(&hex[i..i+4], 16) {
            u16_chars.push(val);
        } else {
            return None;
        }
        i += 4;
    }
    if u16_chars.is_empty() && hex.len() == 2 {
        if let Ok(val) = u16::from_str_radix(hex, 16) {
            u16_chars.push(val);
        }
    }
    Some(String::from_utf16_lossy(&u16_chars))
}

fn decode_bytes(
    bytes: &[u8],
    encoding: Option<&lopdf::Encoding>,
    custom_to_unicode: Option<&HashMap<u32, String>>,
    is_cid: bool,
) -> String {
    if is_cid {
        let mut decoded = String::new();
        let mut i = 0;
        while i + 2 <= bytes.len() {
            let cid = ((bytes[i] as u32) << 8) | (bytes[i+1] as u32);
            if let Some(custom_map) = custom_to_unicode {
                if let Some(mapped_str) = custom_map.get(&cid) {
                    decoded.push_str(mapped_str);
                    i += 2;
                    continue;
                }
            }
            if let Some(lopdf::Encoding::UnicodeMapEncoding(cmap)) = encoding {
                if let Some(unicode_vec) = cmap.get(cid as u16) {
                    decoded.push_str(&String::from_utf16_lossy(&unicode_vec));
                    i += 2;
                    continue;
                }
            }
            if let Some(ch) = std::char::from_u32(cid) {
                decoded.push(ch);
            }
            i += 2;
        }
        decoded
    } else {
        if let Some(custom_map) = custom_to_unicode {
            let mut decoded = String::new();
            for &b in bytes {
                if let Some(mapped_str) = custom_map.get(&(b as u32)) {
                    decoded.push_str(mapped_str);
                } else {
                    decoded.push(b as char);
                }
            }
            decoded
        } else if let Some(enc) = encoding {
            lopdf::Document::decode_text(enc, bytes).unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string())
        } else {
            String::from_utf8_lossy(bytes).to_string()
        }
    }
}

/// Decode a single PDF string or hex string using the specified encoding.
fn decode_pdf_string(
    s: &str,
    encoding: Option<&lopdf::Encoding>,
    custom_to_unicode: Option<&HashMap<u32, String>>,
    is_cid: bool,
) -> Option<String> {
    let extracted_bytes = extract_pdf_string_bytes(s)?;
    Some(decode_bytes(&extracted_bytes, encoding, custom_to_unicode, is_cid))
}

/// Decode text from a PDF array TJ operator, applying font encoding to each string segment.
fn decode_pdf_array_text(
    s: &str,
    encoding: Option<&lopdf::Encoding>,
    custom_to_unicode: Option<&HashMap<u32, String>>,
    is_cid: bool,
) -> Option<String> {
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
            if let Some(extracted_bytes) = extract_pdf_string_bytes(&format!("({})", string_content)) {
                let segment_text = decode_bytes(&extracted_bytes, encoding, custom_to_unicode, is_cid);
                result.push_str(&segment_text);
            }
        } else if c == '<' {
            let start = i + 1;
            i += 1;
            while i < bytes.len() && bytes[i] as char != '>' { i += 1; }
            let hex_content = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            let extracted_bytes = decode_hex_string_bytes(hex_content);
            let segment_text = decode_bytes(&extracted_bytes, encoding, custom_to_unicode, is_cid);
            result.push_str(&segment_text);
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

/// Format raw bytes as a PDF hex string operand.
fn format_pdf_hex_bytes(bytes: &[u8]) -> String {
    let mut hex_str = String::with_capacity(bytes.len() * 2 + 2);
    hex_str.push('<');
    for byte in bytes {
        hex_str.push_str(&format!("{:02X}", byte));
    }
    hex_str.push('>');
    hex_str
}

/// Build a unicode→CID map from a CID font encoding (ToUnicode CMap based).
/// Returns None if the encoding is not a CID font.
fn build_unicode_to_cid_map(encoding: &lopdf::Encoding) -> Option<HashMap<u32, u16>> {
    let lopdf::Encoding::UnicodeMapEncoding(cmap) = encoding else {
        return None;
    };
    let mut unicode_to_cid = HashMap::new();
    for cid in 0u16..=65535 {
        if let Some(unicode_vec) = cmap.get(cid) {
            if unicode_vec.len() == 1 {
                unicode_to_cid.entry(unicode_vec[0] as u32).or_insert(cid);
            } else if !unicode_vec.is_empty() {
                for ch in String::from_utf16_lossy(&unicode_vec).chars() {
                    unicode_to_cid.entry(ch as u32).or_insert(cid);
                }
            }
        }
    }
    Some(unicode_to_cid)
}

fn font_supports_char_via_encoding(encoding: &lopdf::Encoding, ch: char) -> bool {
    if let Some(map) = build_unicode_to_cid_map(encoding) {
        return map.contains_key(&(ch as u32));
    }
    let s = ch.to_string();
    let bytes = LopdfDocument::encode_text(encoding, &s);
    if bytes.is_empty() {
        return false;
    }
    match encoding.bytes_to_string(&bytes) {
        Ok(decoded) => decoded == s,
        Err(_) => false,
    }
}

/// A planned encoding segment: text bytes for a chosen font + the font's PDF name.
#[derive(Debug, Clone)]
pub struct FontSegment {
    pub font_name: String,
    pub text: String,
    pub encoded_operand: String,
}

/// Encode a single text chunk using a specific font and return the proper PDF operand
/// (hex string for CID fonts, literal/hex for one-byte fonts).
pub fn encode_chunk_with_font(
    doc: &LopdfDocument,
    page_id: ObjectId,
    font_name: &str,
    text: &str,
) -> Result<String, HandlerError> {
    if text.is_empty() {
        return Ok("<>".to_string());
    }

    let Ok(fonts) = doc.get_page_fonts(page_id) else {
        return Err(HandlerError::OperationFailed(format!(
            "page has no font resources for '{}'",
            font_name
        )));
    };

    for (name, font) in fonts {
        if String::from_utf8_lossy(&name) != font_name {
            continue;
        }

        // Try our custom ToUnicode CMap mapping first!
        let mut custom_cmap: Option<HashMap<u32, String>> = None;
        if let Ok(to_unicode) = font.get(b"ToUnicode") {
            if let Ok(ref_id) = to_unicode.as_reference() {
                if let Ok(Object::Stream(stream)) = doc.get_object(ref_id) {
                    let content = String::from_utf8_lossy(&stream.content);
                    let cmap = parse_to_unicode_cmap(&content);
                    if !cmap.is_empty() {
                        custom_cmap = Some(cmap);
                    }
                }
            }
        }

        if let Some(ref cmap) = custom_cmap {
            let mut bytes = Vec::with_capacity(text.len() * 2);
            let mut missing = String::new();
            for ch in text.chars() {
                // Find CID that maps to ch
                let found_cid = cmap.iter()
                    .find(|(_, val)| val.contains(ch))
                    .map(|(cid, _)| *cid);
                if let Some(cid) = found_cid {
                    bytes.push((cid >> 8) as u8);
                    bytes.push((cid & 0xFF) as u8);
                } else {
                    missing.push(ch);
                }
            }
            if !missing.is_empty() {
                return Err(HandlerError::OperationFailed(format!(
                    "characters not encodable in font '{}': {}",
                    font_name, missing
                )));
            }
            return Ok(format_pdf_hex_bytes(&bytes));
        }

        let encoding = font.get_font_encoding(doc).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to resolve encoding for '{}': {:?}", font_name, e))
        })?;

        if let Some(map) = build_unicode_to_cid_map(&encoding) {
            let mut bytes = Vec::with_capacity(text.len() * 2);
            let mut missing = String::new();
            for ch in text.chars() {
                if let Some(&cid) = map.get(&(ch as u32)) {
                    bytes.push((cid >> 8) as u8);
                    bytes.push((cid & 0xFF) as u8);
                } else {
                    missing.push(ch);
                }
            }
            if !missing.is_empty() {
                return Err(HandlerError::OperationFailed(format!(
                    "characters not encodable in font '{}': {}",
                    font_name, missing
                )));
            }
            return Ok(format_pdf_hex_bytes(&bytes));
        }

        let bytes = LopdfDocument::encode_text(&encoding, text);
        let decoded = encoding.bytes_to_string(&bytes).unwrap_or_default();
        if decoded != text {
            return Err(HandlerError::OperationFailed(format!(
                "characters not encodable in font '{}': {}",
                font_name, text
            )));
        }
        if bytes
            .iter()
            .all(|&b| b.is_ascii() && b >= 0x20 && b != b'(' && b != b')' && b != b'\\')
        {
            return Ok(encode_pdf_string(text));
        }
        return Ok(format_pdf_hex_bytes(&bytes));
    }

    Err(HandlerError::OperationFailed(format!(
        "font '{}' not found on page",
        font_name
    )))
}

/// Pick fonts for each character of `text` from the fonts available on the page.
/// Priority order: `preferred_font` (if set) → other CID fonts on page → one-byte fonts on page.
/// Returns a list of segments (font_name, chunk_text, encoded_operand).
/// Characters that no page font can render are collected and returned via `missing_chars`.
pub fn pick_fonts_for_text(
    doc: &LopdfDocument,
    page_id: ObjectId,
    preferred_font: Option<&str>,
    text: &str,
    missing_chars: &mut Vec<char>,
) -> Result<Vec<FontSegment>, HandlerError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }

    // Collect page fonts and their properties
    let mut custom_to_unicode = HashMap::new();
    let mut font_original_chars: HashMap<String, HashSet<char>> = HashMap::new();
    let mut is_subset_font: HashMap<String, bool> = HashMap::new();
    let mut page_fonts: HashMap<String, &Dictionary> = HashMap::new();

    // Parse content stream to gather original text characters for each font
    if let Ok(content_bytes) = doc.get_page_content(page_id) {
        if let Ok(parsed) = parse_page_content_stream(&content_bytes, page_id, doc) {
            for block in &parsed.text_blocks {
                if let Some(ref f_name) = block.style.font_name {
                    let set = font_original_chars.entry(f_name.clone()).or_default();
                    for ch in block.text.chars() {
                        set.insert(ch);
                    }
                }
            }
        }
    }

    if let Ok(fonts) = doc.get_page_fonts(page_id) {
        for (name, font) in fonts {
            let font_name = String::from_utf8_lossy(&name).to_string();
            let is_sub = font.get(b"BaseFont")
                .ok()
                .and_then(|v| v.as_name_str().ok())
                .map(|s| s.contains('+'))
                .unwrap_or(false);
            is_subset_font.insert(font_name.clone(), is_sub);
            page_fonts.insert(font_name.clone(), font);

            if let Ok(to_unicode) = font.get(b"ToUnicode") {
                if let Ok(ref_id) = to_unicode.as_reference() {
                    if let Ok(Object::Stream(stream)) = doc.get_object(ref_id) {
                        let content = String::from_utf8_lossy(&stream.content);
                        let cmap = parse_to_unicode_cmap(&content);
                        if !cmap.is_empty() {
                            custom_to_unicode.insert(font_name, cmap);
                        }
                    }
                }
            }
        }
    }

    let font_supports_char = |font_name: &str, ch: char| -> bool {
        // 1. Check custom ToUnicode CMap if present
        if let Some(cmap) = custom_to_unicode.get(font_name) {
            let found_cid = cmap.iter().find(|(_, val)| val.contains(ch));
            if found_cid.is_some() {
                return true;
            }
        }

        // 2. If the font is subsetted, it only supports the character if it was originally in the original characters
        let is_sub = is_subset_font.get(font_name).copied().unwrap_or(false);
        if is_sub {
            if let Some(set) = font_original_chars.get(font_name) {
                if set.contains(&ch) {
                    return true;
                }
            }
            return false;
        }

        // 3. If not subsetted, check via standard encoding
        if let Some(font_dict) = page_fonts.get(font_name) {
            if let Ok(encoding) = font_dict.get_font_encoding(doc) {
                if font_supports_char_via_encoding(&encoding, ch) {
                    return true;
                }
            }
        }

        false
    };

    // Determine the font that can render a single character, honoring preferred order.
    let pick_font_for = |ch: char| -> Option<String> {
        if let Some(pref) = preferred_font {
            if page_fonts.contains_key(pref) && font_supports_char(pref, ch) {
                return Some(pref.to_string());
            }
        }
        
        // Check other fonts on the page
        for font_name in page_fonts.keys() {
            if Some(font_name.as_str()) == preferred_font {
                continue;
            }
            if font_supports_char(font_name, ch) {
                return Some(font_name.clone());
            }
        }
        None
    };

    // Build segments: coalesce consecutive same-font characters.
    let mut segments: Vec<FontSegment> = Vec::new();
    let mut current_font: Option<String> = None;
    let mut current_text = String::new();

    for ch in text.chars() {
        let font_for_ch = pick_font_for(ch);
        match font_for_ch {
            Some(font_name) => {
                if current_font.as_deref() == Some(font_name.as_str()) {
                    current_text.push(ch);
                } else {
                    if let Some(prev) = current_font.take() {
                        let encoded = encode_chunk_with_font(doc, page_id, &prev, &current_text)?;
                        segments.push(FontSegment {
                            font_name: prev,
                            text: std::mem::take(&mut current_text),
                            encoded_operand: encoded,
                        });
                    }
                    current_font = Some(font_name);
                    current_text.push(ch);
                }
            }
            None => {
                missing_chars.push(ch);
            }
        }
    }

    if let Some(prev) = current_font {
        let encoded = encode_chunk_with_font(doc, page_id, &prev, &current_text)?;
        segments.push(FontSegment {
            font_name: prev,
            text: current_text,
            encoded_operand: encoded,
        });
    }

    Ok(segments)
}

/// Convenience wrapper: encode text for a specific font on a page.
/// Used when only one font is involved and the caller wants a single hex string back.
pub fn encode_pdf_text_with_font(
    doc: &LopdfDocument,
    page_id: ObjectId,
    font_name: Option<&str>,
    text: &str,
) -> Result<String, HandlerError> {
    if text.is_empty() {
        return Ok("<>".to_string());
    }
    if let Some(name) = font_name {
        return encode_chunk_with_font(doc, page_id, name, text);
    }
    Ok(encode_pdf_string(text))
}

/// Parse a numeric value from a PDF content stream operand string.
fn parse_float(s: &str) -> f32 {
    s.trim().parse::<f32>().unwrap_or(0.0)
}

/// Tokenize a PDF content stream line, respecting self-delimiting tokens like strings (parentheses),
/// hex strings (angle brackets), and arrays (square brackets) so they are parsed correctly even if
/// there is no space between them and their following operators (e.g. `<xxx>Tj`, `(xxx)Tj`, `[xxx]TJ`).
pub fn tokenize_pdf_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Skip whitespace
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        if chars[i] == '(' {
            // Parse string literal
            let start = i;
            i += 1;
            let mut depth = 1;
            let mut escaped = false;
            while i < chars.len() && depth > 0 {
                if escaped {
                    escaped = false;
                } else if chars[i] == '\\' {
                    escaped = true;
                } else if chars[i] == '(' {
                    depth += 1;
                } else if chars[i] == ')' {
                    depth -= 1;
                }
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            tokens.push(token);
        } else if chars[i] == '<' {
            // Parse hex string or dictionary start
            let start = i;
            i += 1;
            if i < chars.len() && chars[i] == '<' {
                // Dictionary start <<
                i += 1;
                let token: String = chars[start..i].iter().collect();
                tokens.push(token);
            } else {
                // Hex string
                while i < chars.len() && chars[i] != '>' {
                    i += 1;
                }
                if i < chars.len() {
                    i += 1; // include '>'
                }
                let token: String = chars[start..i].iter().collect();
                tokens.push(token);
            }
        } else if chars[i] == '>' {
            let start = i;
            i += 1;
            if i < chars.len() && chars[i] == '>' {
                // Dictionary end >>
                i += 1;
                let token: String = chars[start..i].iter().collect();
                tokens.push(token);
            } else {
                tokens.push(">".to_string());
            }
        } else if chars[i] == '[' {
            // Parse array (can contain strings, numbers, etc. but we can just parse until matching ']')
            let start = i;
            i += 1;
            let mut depth = 1;
            while i < chars.len() && depth > 0 {
                if chars[i] == '[' {
                    depth += 1;
                } else if chars[i] == ']' {
                    depth -= 1;
                }
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            tokens.push(token);
        } else if chars[i] == '/' {
            // PDF Name: read until next whitespace or any delimiter (including '/').
            let start = i;
            i += 1;
            while i < chars.len() {
                let c = chars[i];
                if c.is_whitespace()
                    || matches!(c, '(' | ')' | '<' | '>' | '[' | ']' | '{' | '}' | '/' | '%')
                {
                    break;
                }
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            tokens.push(token);
        } else {
            // Regular tokens (numbers and operators). PowerPoint/Pages PDFs frequently
            // omit whitespace between adjacent tokens (e.g. `Tf0.05`, `q1`, `Tr10`,
            // `j0.6006`, `RGq`). To split correctly we classify by the first character:
            //
            //   * digit / sign / '.'  -> number, continue while [0-9.] (sign only at start)
            //   * letter              -> operator name, continue while [A-Za-z*'"]
            //                          (PDF operators include `T*`, `f*`, `B*`, `'`, `"`)
            //   * anything else       -> single character
            let start = i;
            let first = chars[i];
            let starts_number = first.is_ascii_digit() || first == '+' || first == '-' || first == '.';
            let starts_word = first.is_ascii_alphabetic();

            if starts_number {
                i += 1;
                while i < chars.len() {
                    let c = chars[i];
                    if c.is_ascii_digit() || c == '.' {
                        i += 1;
                    } else {
                        break;
                    }
                }
            } else if starts_word {
                i += 1;
                while i < chars.len() {
                    let c = chars[i];
                    if c.is_ascii_alphabetic() || c == '*' || c == '\'' || c == '"' {
                        i += 1;
                    } else {
                        break;
                    }
                }
            } else {
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            tokens.push(token);
        }
    }
    tokens
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
    tm_a: f32,
    tm_b: f32,
    tm_c: f32,
    tm_d: f32,
    // Graphics State Transformation Matrix (CTM)
    ctm_a: f32,
    ctm_b: f32,
    ctm_c: f32,
    ctm_d: f32,
    ctm_e: f32,
    ctm_f: f32,
    ctm_stack: Vec<[f32; 6]>,
    filled_rects: Vec<FilledRect>,
    last_rect: Option<(f32, f32, f32, f32)>,
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
            tm_a: 1.0,
            tm_b: 0.0,
            tm_c: 0.0,
            tm_d: 1.0,
            ctm_a: 1.0,
            ctm_b: 0.0,
            ctm_c: 0.0,
            ctm_d: 1.0,
            ctm_e: 0.0,
            ctm_f: 0.0,
            ctm_stack: Vec::new(),
            filled_rects: Vec::new(),
            last_rect: None,
        }
    }
}

/// Parse a page's content stream bytes into a ParsedContentStream.
/// Uses line-by-line parsing (since lopdf::parser::content is private).
fn is_pdf_operator(token: &str) -> bool {
    matches!(
        token,
        "BT" | "ET" | "Tm" | "Td" | "TD" | "T*" | "Tf" | "Tc" | "Tw" | "Tj" | "TJ" | "rg" | "g" | "k" | "q" | "Q" | "cm" | "Do" | "re" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*"
    )
}

fn colors_equal(c1: Option<&PdfColor>, c2: Option<&PdfColor>) -> bool {
    match (c1, c2) {
        (None, None) => true,
        (Some(PdfColor::Gray(g1)), Some(PdfColor::Gray(g2))) => (g1 - g2).abs() < 0.001,
        (Some(PdfColor::Rgb(r1, g1, b1)), Some(PdfColor::Rgb(r2, g2, b2))) => {
            (r1 - r2).abs() < 0.001 && (g1 - g2).abs() < 0.001 && (b1 - b2).abs() < 0.001
        }
        (Some(PdfColor::Cmyk(c1, m1, y1, k1)), Some(PdfColor::Cmyk(c2, m2, y2, k2))) => {
            (c1 - c2).abs() < 0.001
                && (m1 - m2).abs() < 0.001
                && (y1 - y2).abs() < 0.001
                && (k1 - k2).abs() < 0.001
        }
        _ => false,
    }
}

fn add_or_merge_text_block(
    text_blocks: &mut Vec<PdfTextBlock>,
    text: String,
    state: &mut TextState,
    font_map: &HashMap<String, FontInfo>,
    line_idx: usize,
    block_counter: &mut usize,
    is_array_text: bool,
    line_token_index: usize,
) {
    let (width, height) = compute_block_dimensions(&text, font_map, state);
    let effective_font_size = state.font_size * state.tm_d.abs();

    let user_bbox = BBox {
        x: state.cursor_x,
        y: state.line_y,
        width,
        height,
    };

    let transform_bbox = |bb: &BBox| -> BBox {
        let c1x = bb.x;
        let text_scale_y = state.tm_d.signum() * bb.height;
        let c1y = bb.y - 0.2 * text_scale_y;
        let c2x = bb.x + bb.width;
        let c2y = bb.y - 0.2 * text_scale_y;
        let c3x = bb.x;
        let c3y = bb.y + 0.8 * text_scale_y;
        let c4x = bb.x + bb.width;
        let c4y = bb.y + 0.8 * text_scale_y;

        let tx1 = c1x * state.ctm_a + c1y * state.ctm_c + state.ctm_e;
        let ty1 = c1x * state.ctm_b + c1y * state.ctm_d + state.ctm_f;
        let tx2 = c2x * state.ctm_a + c2y * state.ctm_c + state.ctm_e;
        let ty2 = c2x * state.ctm_b + c2y * state.ctm_d + state.ctm_f;
        let tx3 = c3x * state.ctm_a + c3y * state.ctm_c + state.ctm_e;
        let ty3 = c3x * state.ctm_b + c3y * state.ctm_d + state.ctm_f;
        let tx4 = c4x * state.ctm_a + c4y * state.ctm_c + state.ctm_e;
        let ty4 = c4x * state.ctm_b + c4y * state.ctm_d + state.ctm_f;

        let min_x = tx1.min(tx2).min(tx3).min(tx4);
        let max_x = tx1.max(tx2).max(tx3).max(tx4);
        let min_y = ty1.min(ty2).min(ty3).min(ty4);
        let max_y = ty1.max(ty2).max(ty3).max(ty4);

        BBox {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        }
    };

    let find_bg_color = |bb: &BBox, rects: &[FilledRect]| -> Option<PdfColor> {
        for rect in rects {
            let x_ok = rect.x <= bb.x + 2.0 && rect.x >= bb.x - 20.0;
            let y_ok = rect.y <= bb.y + 2.0 && rect.y >= bb.y - 10.0;
            let w_ok = rect.width >= bb.width - 2.0 && rect.width <= bb.width + 100.0;
            let h_ok = rect.height >= bb.height - 2.0 && rect.height <= bb.height + 20.0;
            if x_ok && y_ok && w_ok && h_ok {
                return Some(rect.color.clone());
            }
        }
        None
    };

    let merged = if let Some(last) = text_blocks.last_mut() {
        // Must be on the exact same vertical line in user space
        let same_y = (last.user_bbox.y - state.line_y).abs() < 0.1;

        // And horizontally adjacent in user space (within 2.0 characters of width)
        let gap = state.cursor_x - (last.user_bbox.x + last.user_bbox.width);
        let close_x = gap.abs() < 2.0 * effective_font_size;

        // And must have the exact same style properties to prevent style loss (e.g. key words in different color/weight)
        let same_font = last.style.font_name == state.font_name;
        let same_size = match last.style.font_size {
            Some(last_sz) => (last_sz - effective_font_size).abs() < 0.01,
            None => false,
        };
        let same_color = colors_equal(last.style.fill_color.as_ref(), state.fill_color.as_ref());

        if same_y && close_x && same_font && same_size && same_color {
            // If there's a small gap, inject a space character between segments
            if gap > 0.15 * effective_font_size && !last.text.ends_with(' ') && !text.starts_with(' ') {
                last.text.push(' ');
            }
            last.text.push_str(&text);
            last.user_bbox.width = state.cursor_x + width - last.user_bbox.x;
            last.bbox = transform_bbox(&last.user_bbox);
            last.bt_end_line = line_idx;
            last.style.bg_color = find_bg_color(&last.user_bbox, &state.filled_rects);
            true
        } else {
            false
        }
    } else {
        false
    };

    if !merged {
        *block_counter += 1;
        let bbox = transform_bbox(&user_bbox);
        let bg_color = find_bg_color(&user_bbox, &state.filled_rects);
        text_blocks.push(PdfTextBlock {
            index: *block_counter,
            text,
            bbox,
            user_bbox,
            style: TextStyle {
                font_name: state.font_name.clone(),
                font_size: Some(effective_font_size),
                raw_font_size: Some(state.font_size),
                fill_color: state.fill_color.clone(),
                char_spacing: state.char_spacing,
                word_spacing: state.word_spacing,
                bg_color,
            },
            bt_start_line: state.bt_start_line,
            bt_end_line: line_idx,
            text_line_index: line_idx,
            line_token_index,
            is_array_text,
        });
    }

    state.cursor_x += width;
}

fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as usize;
        let b1 = if i + 1 < data.len() { data[i + 1] as usize } else { 0 };
        let b2 = if i + 2 < data.len() { data[i + 2] as usize } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[(triple >> 18) & 63] as char);
        result.push(ALPHABET[(triple >> 12) & 63] as char);

        if i + 1 < data.len() { result.push(ALPHABET[(triple >> 6) & 63] as char); }
        else { result.push('='); }

        if i + 2 < data.len() { result.push(ALPHABET[triple & 63] as char); }
        else { result.push('='); }

        i += 3;
    }
    result
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            if c & 1 != 0 {
                c = (c >> 1) ^ 0xedb8_8320;
            } else {
                c >>= 1;
            }
        }
    }
    !c
}

fn write_png_chunk(buf: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start_idx = buf.len();
    buf.extend_from_slice(chunk_type);
    buf.extend_from_slice(data);
    let crc_val = crc32(&buf[start_idx..]);
    buf.extend_from_slice(&crc_val.to_be_bytes());
}

fn encode_png(width: u32, height: u32, rgb_data: &[u8]) -> Option<Vec<u8>> {
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    let mut png = Vec::new();
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // color type (RGB)
    ihdr.push(0); // compression method
    ihdr.push(0); // filter method
    ihdr.push(0); // interlace method
    write_png_chunk(&mut png, b"IHDR", &ihdr);

    let row_size = (3 * width) as usize;
    let mut filtered_data = Vec::with_capacity((1 + row_size) * height as usize);
    for row in rgb_data.chunks_exact(row_size) {
        filtered_data.push(0); // Filter type 0
        filtered_data.extend_from_slice(row);
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&filtered_data).ok()?;
    let compressed = encoder.finish().ok()?;
    write_png_chunk(&mut png, b"IDAT", &compressed);

    write_png_chunk(&mut png, b"IEND", &[]);

    Some(png)
}

fn convert_to_rgb_png(data: &[u8]) -> Option<Vec<u8>> {
    use jpeg_decoder::{Decoder, PixelFormat};

    let mut decoder = Decoder::new(data);
    let pixels = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (info.width as u32, info.height as u32);

    match info.pixel_format {
        PixelFormat::RGB24 => {
            encode_png(width, height, &pixels)
        }
        PixelFormat::CMYK32 => {
            let mut rgb = Vec::with_capacity((width * height * 3) as usize);
            for chunk in pixels.chunks_exact(4) {
                // PDF/Photoshop CMYK JPEGs have inverted channels (C_raw = 255 - C_actual, etc.)
                let c_raw = chunk[0] as f32;
                let m_raw = chunk[1] as f32;
                let y_raw = chunk[2] as f32;
                let k_raw = chunk[3] as f32;
                
                let r = ((c_raw * k_raw) / 255.0) as u8;
                let g = ((m_raw * k_raw) / 255.0) as u8;
                let b = ((y_raw * k_raw) / 255.0) as u8;
                
                rgb.push(r);
                rgb.push(g);
                rgb.push(b);
            }
            encode_png(width, height, &rgb)
        }
        PixelFormat::L8 => {
            let mut rgb = Vec::with_capacity((width * height * 3) as usize);
            for &g in &pixels {
                rgb.push(g);
                rgb.push(g);
                rgb.push(g);
            }
            encode_png(width, height, &rgb)
        }
        _ => None,
    }
}

fn decode_png_predictor(data: &[u8], bytes_per_pixel: usize, columns: usize) -> Option<Vec<u8>> {
    let row_data_size = columns * bytes_per_pixel;
    let row_stride = 1 + row_data_size;
    if data.len() % row_stride != 0 {
        return None;
    }
    let num_rows = data.len() / row_stride;
    let mut decompressed = vec![0u8; num_rows * row_data_size];

    for r in 0..num_rows {
        let row_start = r * row_stride;
        let filter = data[row_start];
        let raw_row = &data[row_start + 1 .. row_start + row_stride];
        
        let out_row_start = r * row_data_size;
        
        match filter {
            0 => {
                decompressed[out_row_start .. out_row_start + row_data_size].copy_from_slice(raw_row);
            }
            1 => {
                for i in 0..row_data_size {
                    let left = if i >= bytes_per_pixel { decompressed[out_row_start + i - bytes_per_pixel] } else { 0 };
                    decompressed[out_row_start + i] = raw_row[i].wrapping_add(left);
                }
            }
            2 => {
                for i in 0..row_data_size {
                    let up = if r > 0 { decompressed[(r - 1) * row_data_size + i] } else { 0 };
                    decompressed[out_row_start + i] = raw_row[i].wrapping_add(up);
                }
            }
            3 => {
                for i in 0..row_data_size {
                    let left = if i >= bytes_per_pixel { decompressed[out_row_start + i - bytes_per_pixel] as u32 } else { 0 };
                    let up = if r > 0 { decompressed[(r - 1) * row_data_size + i] as u32 } else { 0 };
                    let avg = ((left + up) / 2) as u8;
                    decompressed[out_row_start + i] = raw_row[i].wrapping_add(avg);
                }
            }
            4 => {
                for i in 0..row_data_size {
                    let left = if i >= bytes_per_pixel { decompressed[out_row_start + i - bytes_per_pixel] as i32 } else { 0 };
                    let up = if r > 0 { decompressed[(r - 1) * row_data_size + i] as i32 } else { 0 };
                    let corner = if i >= bytes_per_pixel && r > 0 { decompressed[(r - 1) * row_data_size + i - bytes_per_pixel] as i32 } else { 0 };
                    
                    let p = left + up - corner;
                    let pa = (p - left).abs();
                    let pb = (p - up).abs();
                    let pc = (p - corner).abs();
                    
                    let paeth = if pa <= pb && pa <= pc {
                        left
                    } else if pb <= pc {
                        up
                    } else {
                        corner
                    } as u8;
                    
                    decompressed[out_row_start + i] = raw_row[i].wrapping_add(paeth);
                }
            }
            _ => return None,
        }
    }
    Some(decompressed)
}

fn decode_flate_to_png(stream: &lopdf::Stream, doc: &LopdfDocument) -> Option<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    let mut decompressed = Vec::new();
    let mut decoder = ZlibDecoder::new(stream.content.as_slice());
    decoder.read_to_end(&mut decompressed).ok()?;

    let width = stream.dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let height = stream.dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    if width == 0 || height == 0 { return None; }

    let mut pixels = decompressed;
    if let Ok(decode_parms) = stream.dict.get(b"DecodeParms") {
        if let Some(params) = decode_parms.as_dict().ok() {
            let predictor = params.get(b"Predictor").ok().and_then(|o| o.as_i64().ok()).unwrap_or(1);
            if (10..=15).contains(&predictor) {
                let columns = params.get(b"Columns").ok().and_then(|o| o.as_i64().ok()).unwrap_or(width as i64) as usize;
                let colors = params.get(b"Colors").ok().and_then(|o| o.as_i64().ok()).unwrap_or(1) as usize;
                let bits = params.get(b"BitsPerComponent").ok().and_then(|o| o.as_i64().ok()).unwrap_or(8) as usize;
                let bytes_per_pixel = (colors * bits + 7) / 8;
                pixels = decode_png_predictor(&pixels, bytes_per_pixel, columns)?;
            }
        }
    }

    let colorspace_obj = stream.dict.get(b"ColorSpace").ok()?;
    let colorspace_resolved = doc.dereference(colorspace_obj).map(|(_, o)| o).unwrap_or(colorspace_obj);

    let mut rgb_pixels = Vec::with_capacity((width * height * 3) as usize);

    match colorspace_resolved {
        Object::Name(ref name) => {
            let name_str = String::from_utf8_lossy(name);
            match name_str.as_ref() {
                "DeviceRGB" | "RGB" => {
                    rgb_pixels = pixels;
                }
                "DeviceCMYK" | "CMYK" => {
                    // Normal CMYK to RGB
                    for chunk in pixels.chunks_exact(4) {
                        let c = chunk[0] as f32 / 255.0;
                        let m = chunk[1] as f32 / 255.0;
                        let y = chunk[2] as f32 / 255.0;
                        let k = chunk[3] as f32 / 255.0;
                        let r = (255.0 * (1.0 - c) * (1.0 - k)) as u8;
                        let g = (255.0 * (1.0 - m) * (1.0 - k)) as u8;
                        let b = (255.0 * (1.0 - y) * (1.0 - k)) as u8;
                        rgb_pixels.push(r);
                        rgb_pixels.push(g);
                        rgb_pixels.push(b);
                    }
                }
                _ => {
                    for &g in &pixels {
                        rgb_pixels.push(g);
                        rgb_pixels.push(g);
                        rgb_pixels.push(g);
                    }
                }
            }
        }
        Object::Array(ref arr) => {
            if arr.len() >= 4 && arr[0].as_name_str().ok() == Some("Indexed") {
                let base_space = arr[1].as_name_str().unwrap_or("DeviceRGB");
                let lookup_obj = &arr[3];
                let lookup_resolved = doc.dereference(lookup_obj).map(|(_, o)| o).unwrap_or(lookup_obj);

                let mut lookup_bytes = Vec::new();
                match lookup_resolved {
                    Object::String(ref bytes, _) => {
                        lookup_bytes = bytes.clone();
                    }
                    Object::Stream(ref stream) => {
                        let mut dec = ZlibDecoder::new(stream.content.as_slice());
                        let mut dec_bytes = Vec::new();
                        if dec.read_to_end(&mut dec_bytes).is_ok() {
                            lookup_bytes = dec_bytes;
                        } else {
                            lookup_bytes = stream.content.clone();
                        }
                    }
                    _ => {}
                }

                let num_components = match base_space {
                    "DeviceRGB" | "RGB" => 3,
                    "DeviceCMYK" | "CMYK" => 4,
                    _ => 1,
                };

                for &idx in &pixels {
                    let idx = idx as usize;
                    let offset = idx * num_components;
                    if offset + num_components <= lookup_bytes.len() {
                        let color_slice = &lookup_bytes[offset .. offset + num_components];
                        match num_components {
                            3 => {
                                rgb_pixels.push(color_slice[0]);
                                rgb_pixels.push(color_slice[1]);
                                rgb_pixels.push(color_slice[2]);
                            }
                            4 => {
                                let c = color_slice[0] as f32 / 255.0;
                                let m = color_slice[1] as f32 / 255.0;
                                let y = color_slice[2] as f32 / 255.0;
                                let k = color_slice[3] as f32 / 255.0;
                                let r = (255.0 * (1.0 - c) * (1.0 - k)) as u8;
                                let g = (255.0 * (1.0 - m) * (1.0 - k)) as u8;
                                let b = (255.0 * (1.0 - y) * (1.0 - k)) as u8;
                                rgb_pixels.push(r);
                                rgb_pixels.push(g);
                                rgb_pixels.push(b);
                            }
                            _ => {
                                rgb_pixels.push(color_slice[0]);
                                rgb_pixels.push(color_slice[0]);
                                rgb_pixels.push(color_slice[0]);
                            }
                        }
                    } else {
                        rgb_pixels.push(0);
                        rgb_pixels.push(0);
                        rgb_pixels.push(0);
                    }
                }
            } else {
                let num_channels = if width * height > 0 { pixels.len() / (width as usize * height as usize) } else { 1 };
                if num_channels == 3 {
                    rgb_pixels = pixels;
                } else if num_channels == 4 {
                    for chunk in pixels.chunks_exact(4) {
                        let c = chunk[0] as f32 / 255.0;
                        let m = chunk[1] as f32 / 255.0;
                        let y = chunk[2] as f32 / 255.0;
                        let k = chunk[3] as f32 / 255.0;
                        let r = (255.0 * (1.0 - c) * (1.0 - k)) as u8;
                        let g = (255.0 * (1.0 - m) * (1.0 - k)) as u8;
                        let b = (255.0 * (1.0 - y) * (1.0 - k)) as u8;
                        rgb_pixels.push(r);
                        rgb_pixels.push(g);
                        rgb_pixels.push(b);
                    }
                } else {
                    for &g in &pixels {
                        rgb_pixels.push(g);
                        rgb_pixels.push(g);
                        rgb_pixels.push(g);
                    }
                }
            }
        }
        _ => {
            let num_channels = if width * height > 0 { pixels.len() / (width as usize * height as usize) } else { 1 };
            if num_channels == 3 {
                rgb_pixels = pixels;
            } else {
                for &g in &pixels {
                    rgb_pixels.push(g);
                    rgb_pixels.push(g);
                    rgb_pixels.push(g);
                }
            }
        }
    }

    encode_png(width, height, &rgb_pixels)
}

/// Extract image dictionaries from a page's /Resources and convert them to Base64 Data URIs.
fn extract_page_images(doc: &LopdfDocument, page_id: ObjectId) -> HashMap<String, String> {
    let mut image_map = HashMap::new();

    if let Ok((resources_dict, _parent_chain)) = doc.get_page_resources(page_id) {
        if let Some(resources) = resources_dict {
            if let Ok(xobject_dict) = resources.get(b"XObject") {
                if let Object::Dictionary(dict) = xobject_dict {
                    for (name, value) in dict.iter() {
                        let pdf_name = String::from_utf8_lossy(name).to_string();
                        if let Ok((_, xobject_obj)) = doc.dereference(value) {
                            if let Object::Stream(stream) = xobject_obj {
                                if let Ok(subtype_obj) = stream.dict.get(b"Subtype") {
                                    if let Ok(name_str) = subtype_obj.as_name_str() {
                                        if name_str == "Image" {
                                            let filter = stream.dict.get(b"Filter")
                                                .ok()
                                                .and_then(|f| f.as_name_str().ok().or_else(|| f.as_array().ok().and_then(|arr| arr.first().and_then(|first| first.as_name_str().ok()))))
                                                .unwrap_or("");
                                            
                                            // Extract the raw JPEG/PNG data
                                            if filter == "DCTDecode" || filter == "JPXDecode" {
                                                if let Some(rgb_png_bytes) = convert_to_rgb_png(&stream.content) {
                                                    let b64 = base64_encode(&rgb_png_bytes);
                                                    image_map.insert(pdf_name, format!("data:image/png;base64,{}", b64));
                                                } else {
                                                    // Fallback to original bytes
                                                    let b64 = base64_encode(&stream.content);
                                                    image_map.insert(pdf_name, format!("data:image/jpeg;base64,{}", b64));
                                                }
                                            } else if filter == "FlateDecode" {
                                                if let Some(rgb_png_bytes) = decode_flate_to_png(stream, doc) {
                                                    let b64 = base64_encode(&rgb_png_bytes);
                                                    image_map.insert(pdf_name.clone(), format!("data:image/png;base64,{}", b64));
                                                } else {
                                                    let b64 = base64_encode(&stream.content);
                                                    image_map.insert(pdf_name.clone(), format!("data:image/png;base64,{}", b64));
                                                }
                                            } else {
                                                let b64 = base64_encode(&stream.content);
                                                image_map.insert(pdf_name.clone(), format!("data:image/png;base64,{}", b64));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    image_map
}

/// Parse a page's content stream bytes into a ParsedContentStream.
/// Uses sequential token parsing to support multiple operators per line.
pub fn parse_page_content_stream(
    content_bytes: &[u8],
    page_id: ObjectId,
    doc: &LopdfDocument,
) -> Result<ParsedContentStream, HandlerError> {
    // Step 1: Split content into lines
    let content_str = String::from_utf8_lossy(content_bytes);
    let lines: Vec<String> = content_str.lines().map(|l| l.to_string()).collect();

    // Step 2: Extract font and image info
    let font_map = extract_page_fonts(doc, page_id);
    let image_map = extract_page_images(doc, page_id);

    // Also load actual lopdf encodings for ToUnicode mapping
    let mut encodings = std::collections::HashMap::new();
    let mut custom_to_unicode = std::collections::HashMap::new();
    if let Ok(fonts) = doc.get_page_fonts(page_id) {
        for (name, font) in fonts {
            let font_name = String::from_utf8_lossy(&name).to_string();
            if let Ok(encoding) = font.get_font_encoding(doc) {
                encodings.insert(font_name.clone(), encoding);
            }
            if let Ok(to_unicode) = font.get(b"ToUnicode") {
                if let Ok(ref_id) = to_unicode.as_reference() {
                    if let Ok(lopdf::Object::Stream(stream)) = doc.get_object(ref_id) {
                        let content = String::from_utf8_lossy(&stream.content);
                        let cmap = parse_to_unicode_cmap(&content);
                        if !cmap.is_empty() {
                            custom_to_unicode.insert(font_name, cmap);
                        }
                    }
                }
            }
        }
    }

    // Step 3: Walk lines, process tokens sequentially
    let mut state = TextState::default();
    let mut text_blocks = Vec::new();
    let mut image_blocks = Vec::new();
    let mut block_counter = 0usize;
    let mut image_counter = 0usize;

    // Track BT/ET pairs to fill in bt_end_line
    let mut bt_stack: Vec<usize> = Vec::new();

    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let tokens = tokenize_pdf_line(trimmed);
        let mut operands: Vec<String> = Vec::new();

        for (token_idx, token) in tokens.iter().enumerate() {
            if is_pdf_operator(token) {
                match token.as_str() {
                    "BT" => {
                        state.in_bt = true;
                        state.bt_start_line = line_idx;
                        state.tm_set = false;
                        state.line_x = 0.0;
                        state.line_y = 0.0;
                        state.cursor_x = 0.0;
                        state.tm_a = 1.0;
                        state.tm_b = 0.0;
                        state.tm_c = 0.0;
                        state.tm_d = 1.0;
                        bt_stack.push(line_idx);
                        state.last_rect = None;
                    }
                    "ET" => {
                        state.in_bt = false;
                    }
                    "Tm" => {
                        let len = operands.len();
                        if len >= 6 {
                            state.tm_a = parse_float(&operands[len - 6]);
                            state.tm_b = parse_float(&operands[len - 5]);
                            state.tm_c = parse_float(&operands[len - 4]);
                            state.tm_d = parse_float(&operands[len - 3]);
                            state.line_x = parse_float(&operands[len - 2]);
                            state.line_y = parse_float(&operands[len - 1]);
                            state.cursor_x = state.line_x;
                            state.tm_set = true;
                        }
                    }
                    "Td" | "TD" => {
                        let len = operands.len();
                        if len >= 2 {
                            let dx = parse_float(&operands[len - 2]);
                            let dy = parse_float(&operands[len - 1]);
                            // Displacements are transformed by the active text matrix!
                            state.line_x = dx * state.tm_a + dy * state.tm_c + state.line_x;
                            state.line_y = dx * state.tm_b + dy * state.tm_d + state.line_y;
                            state.cursor_x = state.line_x;
                            state.tm_set = true;
                        }
                    }
                    "T*" => {
                        state.line_y -= state.font_size * state.tm_d.abs();
                        state.cursor_x = state.line_x;
                    }
                    "Tf" => {
                        let len = operands.len();
                        if len >= 2 {
                            let font_name_raw = operands[len - 2].trim();
                            let font_name = if font_name_raw.starts_with('/') {
                                font_name_raw[1..].to_string()
                            } else {
                                font_name_raw.to_string()
                            };
                            state.font_name = Some(font_name);
                            state.font_size = parse_float(&operands[len - 1]);
                        }
                    }
                    "Tc" => {
                        let len = operands.len();
                        if len >= 1 {
                            state.char_spacing = parse_float(&operands[len - 1]);
                        }
                    }
                    "Tw" => {
                        let len = operands.len();
                        if len >= 1 {
                            state.word_spacing = parse_float(&operands[len - 1]);
                        }
                    }
                    "rg" => {
                        let len = operands.len();
                        if len >= 3 {
                            state.fill_color = Some(PdfColor::Rgb(
                                parse_float(&operands[len - 3]),
                                parse_float(&operands[len - 2]),
                                parse_float(&operands[len - 1]),
                            ));
                        }
                    }
                    "g" => {
                        let len = operands.len();
                        if len >= 1 {
                            state.fill_color = Some(PdfColor::Gray(parse_float(&operands[len - 1])));
                        }
                    }
                    "k" => {
                        let len = operands.len();
                        if len >= 4 {
                            state.fill_color = Some(PdfColor::Cmyk(
                                parse_float(&operands[len - 4]),
                                parse_float(&operands[len - 3]),
                                parse_float(&operands[len - 2]),
                                parse_float(&operands[len - 1]),
                            ));
                        }
                    }
                    "q" => {
                        state.ctm_stack.push([
                            state.ctm_a,
                            state.ctm_b,
                            state.ctm_c,
                            state.ctm_d,
                            state.ctm_e,
                            state.ctm_f,
                        ]);
                        state.last_rect = None;
                    }
                    "Q" => {
                        if let Some(restored) = state.ctm_stack.pop() {
                            state.ctm_a = restored[0];
                            state.ctm_b = restored[1];
                            state.ctm_c = restored[2];
                            state.ctm_d = restored[3];
                            state.ctm_e = restored[4];
                            state.ctm_f = restored[5];
                        }
                        state.last_rect = None;
                    }
                    "cm" => {
                        let len = operands.len();
                        if len >= 6 {
                            let ma = parse_float(&operands[len - 6]);
                            let mb = parse_float(&operands[len - 5]);
                            let mc = parse_float(&operands[len - 4]);
                            let md = parse_float(&operands[len - 3]);
                            let me = parse_float(&operands[len - 2]);
                            let mf = parse_float(&operands[len - 1]);
                            
                            let new_a = ma * state.ctm_a + mb * state.ctm_c;
                            let new_b = ma * state.ctm_b + mb * state.ctm_d;
                            let new_c = mc * state.ctm_a + md * state.ctm_c;
                            let new_d = mc * state.ctm_b + md * state.ctm_d;
                            let new_e = me * state.ctm_a + mf * state.ctm_c + state.ctm_e;
                            let new_f = me * state.ctm_b + mf * state.ctm_d + state.ctm_f;
                            
                            state.ctm_a = new_a;
                            state.ctm_b = new_b;
                            state.ctm_c = new_c;
                            state.ctm_d = new_d;
                            state.ctm_e = new_e;
                            state.ctm_f = new_f;
                        }
                        state.last_rect = None;
                    }
                    "re" => {
                        let len = operands.len();
                        if len >= 4 {
                            let rx = parse_float(&operands[len - 4]);
                            let ry = parse_float(&operands[len - 3]);
                            let rw = parse_float(&operands[len - 2]);
                            let rh = parse_float(&operands[len - 1]);
                            state.last_rect = Some((rx, ry, rw, rh));
                        }
                    }
                    "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                        if let Some((rx, ry, rw, rh)) = state.last_rect.take() {
                            state.filled_rects.push(FilledRect {
                                x: rx,
                                y: ry,
                                width: rw,
                                height: rh,
                                color: state.fill_color.clone().unwrap_or(PdfColor::Gray(0.0)),
                            });
                        }
                    }
                    "Do" => {
                        if let Some(operand) = operands.last() {
                            let xobject_name = if operand.starts_with('/') {
                                operand[1..].to_string()
                            } else {
                                operand.to_string()
                            };
                            
                            if image_map.contains_key(&xobject_name) {
                                image_counter += 1;
                                image_blocks.push(PdfImageBlock {
                                    index: image_counter,
                                    bbox: BBox {
                                        x: state.ctm_e,
                                        y: state.ctm_f,
                                        width: state.ctm_a.abs(),
                                        height: state.ctm_d.abs(),
                                    },
                                    xobject_name,
                                });
                            }
                        }
                    }
                    "Tj" => {
                        if state.in_bt {
                            if let Some(operand) = operands.last() {
                                let active_encoding = state.font_name.as_ref().and_then(|name| encodings.get(name));
                                let is_cid = state.font_name.as_ref()
                                    .and_then(|name| font_map.get(name))
                                    .map(|info| info.is_cid_font)
                                    .unwrap_or(false);
                                let custom_cmap = state.font_name.as_ref()
                                    .and_then(|name| custom_to_unicode.get(name));
                                if let Some(text) = decode_pdf_string(operand, active_encoding, custom_cmap, is_cid) {
                                    if !text.is_empty() {
                                        add_or_merge_text_block(
                                            &mut text_blocks,
                                            text,
                                            &mut state,
                                            &font_map,
                                            line_idx,
                                            &mut block_counter,
                                            false,
                                            token_idx.saturating_sub(1),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    "TJ" => {
                        if state.in_bt {
                            if let Some(operand) = operands.last() {
                                let active_encoding = state.font_name.as_ref().and_then(|name| encodings.get(name));
                                let is_cid = state.font_name.as_ref()
                                    .and_then(|name| font_map.get(name))
                                    .map(|info| info.is_cid_font)
                                    .unwrap_or(false);
                                let custom_cmap = state.font_name.as_ref()
                                    .and_then(|name| custom_to_unicode.get(name));
                                if let Some(text) = decode_pdf_array_text(operand, active_encoding, custom_cmap, is_cid) {
                                    if !text.is_empty() {
                                        add_or_merge_text_block(
                                            &mut text_blocks,
                                            text,
                                            &mut state,
                                            &font_map,
                                            line_idx,
                                            &mut block_counter,
                                            true,
                                            token_idx.saturating_sub(1),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                operands.clear();
            } else {
                operands.push(token.clone());
            }
        }
    }

    // Update bt_end_line — find the ET line for each BT section
    let mut bt_et_pairs: Vec<(usize, usize)> = Vec::new();
    let mut bt_stack: Vec<usize> = Vec::new();
    for (line_idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.contains("BT") {
            bt_stack.push(line_idx);
        } else if trimmed.contains("ET") {
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
        image_blocks,
        image_map,
    })
}

fn compute_block_dimensions(
    text: &str,
    font_map: &HashMap<String, FontInfo>,
    state: &TextState,
) -> (f32, f32) {
    let effective_height = state.font_size * state.tm_d.abs();
    let effective_width_scale = state.font_size * state.tm_a.abs();

    let width = if let Some(ref font_name) = state.font_name {
        let font_info = font_map.get(font_name)
            .cloned()
            .unwrap_or_else(|| FontInfo {
                pdf_name: font_name.clone(),
                base_font: None,
                is_cid_font: false,
                char_widths: HashMap::new(),
                default_width: standard_font_avg_width(font_name),
                unicode_to_cid: HashMap::new(),
            });
        estimate_text_width(text, &font_info, effective_width_scale, state.char_spacing, state.word_spacing)
    } else {
        text.chars().count() as f32 * effective_width_scale * 0.5
    };
    (width, effective_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_pdf_string(s: &str) -> Option<String> {
        decode_pdf_string(s, None, None, false)
    }

    fn extract_pdf_array_text(s: &str) -> Option<String> {
        decode_pdf_array_text(s, None, None, false)
    }

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
    fn test_encode_cid_font_text_demo3() {
        let pdf_path = "../../examples/pdf/demo3.pdf";
        if let Ok(doc) = lopdf::Document::load(pdf_path) {
            let page_id = *doc.get_pages().get(&1).unwrap();
            let encoded = encode_pdf_text_with_font(&doc, page_id, Some("C2_0"), "刑事技术").unwrap();
            assert_eq!(encoded, "<04AB03AB086109A2>");
        }
    }

    #[test]
    fn test_tokenize_line() {
        let tokens = tokenize_pdf_line("72 0 0 72 100 200 Tm");
        assert_eq!(tokens, vec!["72", "0", "0", "72", "100", "200", "Tm"]);

        let tokens = tokenize_pdf_line("/F1 12 Tf");
        assert_eq!(tokens, vec!["/F1", "12", "Tf"]);

        let tokens = tokenize_pdf_line("(Hello) Tj");
        assert_eq!(tokens, vec!["(Hello)", "Tj"]);
    }

    /// Cover the PowerPoint/Pages style PDFs where operators and operands are
    /// packed without whitespace (the parser previously could not handle this).
    #[test]
    fn test_tokenize_compact_ppt_style() {
        // Tf followed by next operand without space
        let tokens = tokenize_pdf_line("/FT47 360 Tf0.05 0 0 -0.05 64 63.675 Tm");
        assert_eq!(
            tokens,
            vec!["/FT47", "360", "Tf", "0.05", "0", "0", "-0.05", "64", "63.675", "Tm"]
        );

        // Single-letter operators packed with following numbers
        let tokens = tokenize_pdf_line("q1 0 0 -1 0 405 cm");
        assert_eq!(tokens, vec!["q", "1", "0", "0", "-1", "0", "405", "cm"]);

        let tokens = tokenize_pdf_line("0.12 w2 M2 J2 j0.5148 w");
        assert_eq!(
            tokens,
            vec!["0.12", "w", "2", "M", "2", "J", "2", "j", "0.5148", "w"]
        );

        // Color + state save run on
        let tokens = tokenize_pdf_line("0 0 0 RG/GS8 gs");
        assert_eq!(tokens, vec!["0", "0", "0", "RG", "/GS8", "gs"]);

        // Tr with digit (text rendering mode)
        let tokens = tokenize_pdf_line("2 Tr10 M0 J");
        assert_eq!(tokens, vec!["2", "Tr", "10", "M", "0", "J"]);

        // Hex string + Tj + TD chain
        let tokens = tokenize_pdf_line("<0CE7>Tj 361 -0 TD<1029>Tj");
        assert_eq!(
            tokens,
            vec!["<0CE7>", "Tj", "361", "-0", "TD", "<1029>", "Tj"]
        );

        // Operator suffix characters: T*, f*, B*
        let tokens = tokenize_pdf_line("T* f* B*");
        assert_eq!(tokens, vec!["T*", "f*", "B*"]);

        // Name immediately followed by operator (no space)
        let tokens = tokenize_pdf_line("/FT86 420 Tf");
        assert_eq!(tokens, vec!["/FT86", "420", "Tf"]);

        // Negative numbers stay grouped
        let tokens = tokenize_pdf_line("-0.5 1.0 -.25 +3");
        assert_eq!(tokens, vec!["-0.5", "1.0", "-.25", "+3"]);
    }

    #[test]
    fn test_print_pdf_images() {
        let pdf_path = "../../examples/pdf/demo.pdf";
        if let Ok(doc) = lopdf::Document::load(pdf_path) {
            println!("--- PDF Images Diagnostic ---");
            let mut cmyk_printed = false;
            for page_num in 1..=doc.get_pages().len() {
                let page_id = doc.get_pages().get(&(page_num as u32)).copied().unwrap();
                if let Ok((resources_dict, _)) = doc.get_page_resources(page_id) {
                    if let Some(resources) = resources_dict {
                        if let Ok(xobject_dict) = resources.get(b"XObject") {
                            if let lopdf::Object::Dictionary(dict) = xobject_dict {
                                for (name, value) in dict.iter() {
                                    let pdf_name = String::from_utf8_lossy(name).to_string();
                                    if let Ok((_, xobject_obj)) = doc.dereference(value) {
                                        if let lopdf::Object::Stream(stream) = xobject_obj {
                                            if let Ok(subtype_obj) = stream.dict.get(b"Subtype") {
                                                if let Ok("Image") = subtype_obj.as_name_str() {
                                                    let filter = stream.dict.get(b"Filter")
                                                        .ok()
                                                        .and_then(|f| f.as_name_str().ok().or_else(|| f.as_array().ok().and_then(|arr| arr.first().and_then(|first| first.as_name_str().ok()))))
                                                        .unwrap_or("");
                                                    let width = stream.dict.get(b"Width")
                                                        .ok()
                                                        .and_then(|o| o.as_i64().ok())
                                                        .unwrap_or(0);
                                                    let height = stream.dict.get(b"Height")
                                                        .ok()
                                                        .and_then(|o| o.as_i64().ok())
                                                        .unwrap_or(0);
                                                    if filter == "DCTDecode" || filter == "JPXDecode" {
                                                        use jpeg_decoder::Decoder;
                                                        let mut decoder = Decoder::new(stream.content.as_slice());
                                                        if let Ok(pixels) = decoder.decode() {
                                                            if let Some(info) = decoder.info() {
                                                                if info.pixel_format == jpeg_decoder::PixelFormat::CMYK32 && !cmyk_printed {
                                                                    cmyk_printed = true;
                                                                    println!("Page {}, Image /{}, Filter={}, Size={}x{}", page_num, pdf_name, filter, info.width, info.height);
                                                                    println!("Stream Dictionary:");
                                                                    for (k, v) in stream.dict.iter() {
                                                                        println!("  /{}: {:?}", String::from_utf8_lossy(k), v);
                                                                    }
                                                                    println!("First 20 CMYK pixels:");
                                                                    for (idx, chunk) in pixels.chunks_exact(4).take(20).enumerate() {
                                                                        let c_raw = chunk[0];
                                                                        let m_raw = chunk[1];
                                                                        let y_raw = chunk[2];
                                                                        let k_raw = chunk[3];
                                                                        
                                                                        // Case A (Inverted: R = c_raw * k_raw / 255)
                                                                        let r_a = ((c_raw as f32 * k_raw as f32) / 255.0) as u8;
                                                                        let g_a = ((m_raw as f32 * k_raw as f32) / 255.0) as u8;
                                                                        let b_a = ((y_raw as f32 * k_raw as f32) / 255.0) as u8;
                                                                        
                                                                        // Case B (Normal: R = (255 - c_raw) * (255 - k_raw) / 255)
                                                                        let c_norm = c_raw as f32 / 255.0;
                                                                        let m_norm = m_raw as f32 / 255.0;
                                                                        let y_norm = y_raw as f32 / 255.0;
                                                                        let k_norm = k_raw as f32 / 255.0;
                                                                        let r_b = (255.0 * (1.0 - c_norm) * (1.0 - k_norm)) as u8;
                                                                        let g_b = (255.0 * (1.0 - m_norm) * (1.0 - k_norm)) as u8;
                                                                        let b_b = (255.0 * (1.0 - y_norm) * (1.0 - k_norm)) as u8;
                                                                        
                                                                        println!("Pixel {}: Raw=[C:{}, M:{}, Y:{}, K:{}], Case A (Inverted RGB)=[{}, {}, {}], Case B (Normal RGB)=[{}, {}, {}]",
                                                                            idx, c_raw, m_raw, y_raw, k_raw, r_a, g_a, b_a, r_b, g_b, b_b
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    } else if filter == "FlateDecode" {
                                                        println!("Page {}, Image /{}, Filter=FlateDecode, Size={}x{}", page_num, pdf_name, width, height);
                                                        let cs_resolved = if let Ok(cs_obj) = stream.dict.get(b"ColorSpace") {
                                                            if let lopdf::Object::Reference(id) = cs_obj {
                                                                doc.get_object(*id).ok().map(|o| format!("{:?}", o)).unwrap_or_else(|| format!("{:?}", cs_obj))
                                                            } else {
                                                                format!("{:?}", cs_obj)
                                                            }
                                                        } else {
                                                            "None".to_string()
                                                        };
                                                        println!("  -> ColorSpace: {}", cs_resolved);
                                                        use flate2::read::ZlibDecoder;
                                                        use std::io::Read;
                                                        let mut decoder = ZlibDecoder::new(stream.content.as_slice());
                                                        let mut decompressed = Vec::new();
                                                        if decoder.read_to_end(&mut decompressed).is_ok() {
                                                            let num_channels = if width * height > 0 { decompressed.len() as i64 / (width * height) } else { 0 };
                                                            println!("  -> Decompressed size: {} bytes, computed channels: {}", decompressed.len(), num_channels);
                                                            if num_channels == 4 && pdf_name == "Im0" && page_num == 2 {
                                                                println!("First 20 FlateDecode CMYK pixels:");
                                                                for (idx, chunk) in decompressed.chunks_exact(4).take(20).enumerate() {
                                                                    let c_raw = chunk[0];
                                                                    let m_raw = chunk[1];
                                                                    let y_raw = chunk[2];
                                                                    let k_raw = chunk[3];
                                                                    
                                                                    // Case A (Inverted: R = c_raw * k_raw / 255)
                                                                    let r_a = ((c_raw as f32 * k_raw as f32) / 255.0) as u8;
                                                                    let g_a = ((m_raw as f32 * k_raw as f32) / 255.0) as u8;
                                                                    let b_a = ((y_raw as f32 * k_raw as f32) / 255.0) as u8;
                                                                    
                                                                    // Case B (Normal: R = (255 - c_raw) * (255 - k_raw) / 255)
                                                                    let c_norm = c_raw as f32 / 255.0;
                                                                    let m_norm = m_raw as f32 / 255.0;
                                                                    let y_norm = y_raw as f32 / 255.0;
                                                                    let k_norm = k_raw as f32 / 255.0;
                                                                    let r_b = (255.0 * (1.0 - c_norm) * (1.0 - k_norm)) as u8;
                                                                    let g_b = (255.0 * (1.0 - m_norm) * (1.0 - k_norm)) as u8;
                                                                    let b_b = (255.0 * (1.0 - y_norm) * (1.0 - k_norm)) as u8;
                                                                    
                                                                    println!("Pixel {}: Raw=[C:{}, M:{}, Y:{}, K:{}], Case A (Inverted RGB)=[{}, {}, {}], Case B (Normal RGB)=[{}, {}, {}]",
                                                                        idx, c_raw, m_raw, y_raw, k_raw, r_a, g_a, b_a, r_b, g_b, b_b
                                                                    );
                                                                }
                                                            }
                                                        } else {
                                                            println!("  -> Decompression failed");
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } else {
            println!("Could not load demo.pdf at {}", pdf_path);
        }
    }

    #[test]
    fn test_debug_pkulaw() {
        let pdf_path = "../../examples/pdf/pkulaw_v6_test.pdf";
        if let Ok(mut doc) = lopdf::Document::load(pdf_path) {
            let _ = doc.decompress();
            println!("Successfully loaded and decompressed pkulaw_v6_test.pdf");
            let pages = doc.get_pages();
            if let Some(&page_id) = pages.get(&2) {
                let content = doc.get_page_content(page_id).unwrap();
                let parsed = parse_page_content_stream(&content, page_id, &doc).unwrap();
                println!("Page 2 text blocks:");
                for block in parsed.text_blocks.iter().take(10) {
                    println!("Text: '{}'", block.text);
                    println!("  Style: {:?}", block.style);
                }
            }
        } else {
            println!("Could not load pkulaw_v6_test.pdf at {}", pdf_path);
        }
    }
}