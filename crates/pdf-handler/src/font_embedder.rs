use std::collections::{HashMap, HashSet};
use std::path::Path;

use handler_common::HandlerError;
use lopdf::{dictionary, Dictionary, Document as LopdfDocument, Object, ObjectId, Stream};
use ttf_parser::{Face, GlyphId};

/// Bundled CJK fallback font (Noto Sans SC, variable TTF).
const BUNDLED_NOTO: &[u8] = include_bytes!("../assets/NotoSansSC-Regular.ttf");

/// Return whether the first face in a font file covers every requested character.
///
/// Semantic exporters use this to select one optional system font that covers
/// mixed CJK, Latin and phonetic text before asking the PDF writer to subset it.
pub fn font_file_covers_chars(
    path: &Path,
    chars_needed: &HashSet<char>,
) -> Result<bool, HandlerError> {
    Ok(font_file_missing_chars(path, chars_needed)?.is_empty())
}

/// Return the requested characters that the first face in a font file cannot render.
pub fn font_file_missing_chars(
    path: &Path,
    chars_needed: &HashSet<char>,
) -> Result<HashSet<char>, HandlerError> {
    let font_bytes = std::fs::read(path).map_err(|error| {
        HandlerError::OperationFailed(format!(
            "failed to read font file '{}': {error}",
            path.display()
        ))
    })?;
    let face = Face::parse(&font_bytes, 0).map_err(|error| {
        HandlerError::OperationFailed(format!(
            "failed to parse font file '{}': {error:?}",
            path.display()
        ))
    })?;
    Ok(chars_needed
        .iter()
        .copied()
        .filter(|character| face.glyph_index(*character).is_none())
        .collect())
}

/// Ensure a CJK-capable font is embedded on the given page so the requested
/// characters can be rendered.
///
/// * If `user_font_file` is set (`--prop fontFile=...`), that TTF is embedded.
/// * Otherwise, characters that no existing page font can render trigger
///   embedding of the bundled NotoSansSC.
///
/// Returns the PDF resource name of the newly embedded font, or `None` if no
/// embedding was needed.
pub fn ensure_cjk_font_for_chars(
    doc: &mut LopdfDocument,
    page_num: usize,
    chars_needed: &HashSet<char>,
    preferred_name: Option<&str>,
    user_font_file: Option<&str>,
    force_embed: bool,
) -> Result<Option<String>, HandlerError> {
    let pages = doc.get_pages();
    let page_id = *pages
        .get(&(page_num as u32))
        .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

    // A semantic export supplies the same complete character set for the first
    // text block on every page. Reuse an already embedded Type0 font object
    // instead of subsetting and embedding the same font once per page.
    if force_embed {
        if let Some(preferred_name) = preferred_name {
            if let Some(font_id) = find_embedded_font_covering_chars(doc, chars_needed) {
                let page_font_name = choose_pdf_font_name(doc, page_id, Some(preferred_name));
                register_font_on_page(doc, page_id, &page_font_name, font_id)?;
                return Ok(Some(page_font_name));
            }
        }
    }

    // Determine which chars actually need embedding.
    let chars_to_embed: HashSet<char> = if user_font_file.is_some() || force_embed {
        chars_needed.clone()
    } else {
        let already_supported = collect_supported_chars(doc, page_id, chars_needed);
        chars_needed
            .iter()
            .copied()
            .filter(|ch| !already_supported.contains(ch))
            .collect()
    };

    if chars_to_embed.is_empty() {
        return Ok(None);
    }

    let font_bytes: Vec<u8> = if let Some(path) = user_font_file {
        std::fs::read(Path::new(path)).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to read font file '{}': {}", path, e))
        })?
    } else {
        BUNDLED_NOTO.to_vec()
    };

    let face = Face::parse(&font_bytes, 0)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to parse font: {:?}", e)))?;

    let mut char_to_gid: Vec<(char, u16)> = Vec::new();
    let mut missing_in_font: Vec<char> = Vec::new();
    for &ch in &chars_to_embed {
        if let Some(GlyphId(gid)) = face.glyph_index(ch) {
            char_to_gid.push((ch, gid));
        } else {
            missing_in_font.push(ch);
        }
    }
    if char_to_gid.is_empty() {
        return Err(HandlerError::OperationFailed(format!(
            "font does not contain glyphs for: {}",
            missing_in_font.iter().collect::<String>()
        )));
    }

    let unique_gids: Vec<u16> = {
        let mut set: HashSet<u16> = HashSet::new();
        let mut out = Vec::new();
        for (_, g) in &char_to_gid {
            if set.insert(*g) {
                out.push(*g);
            }
        }
        out
    };

    let remapper = subsetter::GlyphRemapper::new_from_glyphs_sorted(&unique_gids);
    let subset_bytes = subsetter::subset(&font_bytes, 0, &remapper)
        .map_err(|e| HandlerError::OperationFailed(format!("failed to subset font: {:?}", e)))?;

    let units_per_em = face.units_per_em() as f32;
    if units_per_em <= 0.0 {
        return Err(HandlerError::OperationFailed(
            "font has invalid units_per_em".into(),
        ));
    }
    let scale = 1000.0 / units_per_em;

    let mut new_gid_to_unicodes: HashMap<u16, Vec<char>> = HashMap::new();
    for (ch, old_gid) in &char_to_gid {
        if let Some(new_gid) = remapper.get(*old_gid) {
            new_gid_to_unicodes.entry(new_gid).or_default().push(*ch);
        }
    }

    // Old GIDs in new-order (new_gid 0 → first item, 1 → second, ...).
    let old_gids_in_new_order: Vec<u16> = remapper.remapped_gids().collect();
    let mut widths: Vec<i64> = Vec::with_capacity(old_gids_in_new_order.len());
    for old_gid in &old_gids_in_new_order {
        let w = face.glyph_hor_advance(GlyphId(*old_gid)).unwrap_or(0) as f32;
        widths.push((w * scale).round() as i64);
    }

    let bbox = face.global_bounding_box();
    let ascender = (face.ascender() as f32 * scale).round() as i64;
    let descender = (face.descender() as f32 * scale).round() as i64;
    let cap_height = face
        .capital_height()
        .map(|h| (h as f32 * scale).round() as i64)
        .unwrap_or(ascender);
    let italic_angle: f32 = face.italic_angle().unwrap_or(0.0);

    let pdf_font_name = choose_pdf_font_name(doc, page_id, preferred_name);
    let postscript_name = format!("{}+NotoSansSC", subset_prefix());

    let length1 = subset_bytes.len() as i64;
    let mut fontfile_stream = Stream::new(
        dictionary! { "Length1" => Object::Integer(length1) },
        subset_bytes,
    );
    let _ = fontfile_stream.compress();
    let fontfile_id = doc.add_object(Object::Stream(fontfile_stream));

    let fd_id = doc.add_object(dictionary! {
        "Type" => Object::Name(b"FontDescriptor".to_vec()),
        "FontName" => Object::Name(postscript_name.clone().into_bytes()),
        "Flags" => Object::Integer(4),
        "FontBBox" => Object::Array(vec![
            Object::Integer((bbox.x_min as f32 * scale) as i64),
            Object::Integer((bbox.y_min as f32 * scale) as i64),
            Object::Integer((bbox.x_max as f32 * scale) as i64),
            Object::Integer((bbox.y_max as f32 * scale) as i64),
        ]),
        "ItalicAngle" => Object::Real(italic_angle),
        "Ascent" => Object::Integer(ascender),
        "Descent" => Object::Integer(descender),
        "CapHeight" => Object::Integer(cap_height),
        "StemV" => Object::Integer(80),
        "FontFile2" => Object::Reference(fontfile_id),
    });

    // W array: single contiguous run starting at CID 0.
    let inner: Vec<Object> = widths.iter().map(|&w| Object::Integer(w)).collect();
    let w_array = vec![Object::Integer(0), Object::Array(inner)];

    let cidfont_id = doc.add_object(dictionary! {
        "Type" => Object::Name(b"Font".to_vec()),
        "Subtype" => Object::Name(b"CIDFontType2".to_vec()),
        "BaseFont" => Object::Name(postscript_name.clone().into_bytes()),
        "CIDSystemInfo" => Object::Dictionary(dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => Object::Integer(0),
        }),
        "FontDescriptor" => Object::Reference(fd_id),
        "CIDToGIDMap" => Object::Name(b"Identity".to_vec()),
        "W" => Object::Array(w_array),
    });

    let tounicode_bytes = build_tounicode_cmap(&new_gid_to_unicodes);
    let mut tu_stream = Stream::new(Dictionary::new(), tounicode_bytes);
    let _ = tu_stream.compress();
    let tu_id = doc.add_object(Object::Stream(tu_stream));

    let type0_id = doc.add_object(dictionary! {
        "Type" => Object::Name(b"Font".to_vec()),
        "Subtype" => Object::Name(b"Type0".to_vec()),
        "BaseFont" => Object::Name(postscript_name.into_bytes()),
        "Encoding" => Object::Name(b"Identity-H".to_vec()),
        "DescendantFonts" => Object::Array(vec![Object::Reference(cidfont_id)]),
        "ToUnicode" => Object::Reference(tu_id),
    });

    register_font_on_page(doc, page_id, &pdf_font_name, type0_id)?;

    Ok(Some(pdf_font_name))
}

fn find_embedded_font_covering_chars(
    doc: &LopdfDocument,
    chars_needed: &HashSet<char>,
) -> Option<ObjectId> {
    doc.objects.iter().find_map(|(object_id, object)| {
        let dictionary = object.as_dict().ok()?;
        if dictionary
            .get(b"Subtype")
            .ok()
            .and_then(|value| value.as_name().ok())
            != Some(b"Type0".as_slice())
        {
            return None;
        }
        let to_unicode_id = dictionary.get(b"ToUnicode").ok()?.as_reference().ok()?;
        let stream = doc.get_object(to_unicode_id).ok()?.as_stream().ok()?;
        let bytes = stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone());
        let content = String::from_utf8_lossy(&bytes);
        let cmap = crate::content_stream::parse_to_unicode_cmap(&content);
        let covered: HashSet<char> = cmap.values().flat_map(|value| value.chars()).collect();
        chars_needed
            .iter()
            .all(|character| covered.contains(character))
            .then_some(*object_id)
    })
}

/// Inspect each page font and collect which of `chars_needed` it can already render.
fn collect_supported_chars(
    doc: &LopdfDocument,
    page_id: ObjectId,
    chars_needed: &HashSet<char>,
) -> HashSet<char> {
    let mut supported = HashSet::new();
    let Ok(fonts) = doc.get_page_fonts(page_id) else {
        return supported;
    };

    // Gather original text characters for each font
    let mut font_original_chars: HashMap<String, HashSet<char>> = HashMap::new();
    if let Ok(content_bytes) = doc.get_page_content(page_id) {
        if let Ok(parsed) =
            crate::content_stream::parse_page_content_stream(&content_bytes, page_id, doc)
        {
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

    for (name, font) in fonts {
        let font_name = String::from_utf8_lossy(&name).to_string();
        let is_subsetted = font
            .get(b"BaseFont")
            .ok()
            .and_then(|v| v.as_name_str().ok())
            .map(|s| s.contains('+'))
            .unwrap_or(false);

        // Try custom ToUnicode CMap first
        let mut custom_cmap: Option<HashMap<u32, String>> = None;
        if let Ok(to_unicode) = font.get(b"ToUnicode") {
            if let Ok(ref_id) = to_unicode.as_reference() {
                if let Ok(Object::Stream(stream)) = doc.get_object(ref_id) {
                    let bytes = stream
                        .decompressed_content()
                        .unwrap_or_else(|_| stream.content.clone());
                    let content = String::from_utf8_lossy(&bytes);
                    let cmap = crate::content_stream::parse_to_unicode_cmap(&content);
                    if !cmap.is_empty() {
                        custom_cmap = Some(cmap);
                    }
                }
            }
        }

        let orig_chars = font_original_chars.get(&font_name);

        for &ch in chars_needed {
            if supported.contains(&ch) {
                continue;
            }

            // 1. Check custom ToUnicode CMap
            if let Some(ref cmap) = custom_cmap {
                if cmap.values().any(|s| s.contains(ch)) {
                    supported.insert(ch);
                    continue;
                }
            }

            // 2. If subsetted, check if the character was originally used on the page by this font
            if is_subsetted {
                if let Some(set) = orig_chars {
                    if set.contains(&ch) {
                        supported.insert(ch);
                    }
                }
            } else {
                // 3. If not subsetted, check standard encoding
                if let Ok(encoding) = font.get_font_encoding(doc) {
                    if char_renders_via_encoding(&encoding, ch) {
                        supported.insert(ch);
                    }
                }
            }
        }
    }
    supported
}

fn char_renders_via_encoding(encoding: &lopdf::Encoding, ch: char) -> bool {
    if let lopdf::Encoding::UnicodeMapEncoding(cmap) = encoding {
        for cid in 0u16..=65535 {
            if let Some(uni) = cmap.get(cid) {
                if uni.len() == 1 && uni[0] as u32 == ch as u32 {
                    return true;
                }
                if !uni.is_empty() {
                    let s = String::from_utf16_lossy(&uni);
                    if s.chars().any(|c| c == ch) {
                        return true;
                    }
                }
            }
        }
        return false;
    }
    let s = ch.to_string();
    let bytes = LopdfDocument::encode_text(encoding, &s);
    if bytes.is_empty() {
        return false;
    }
    encoding
        .bytes_to_string(&bytes)
        .map(|d| d == s)
        .unwrap_or(false)
}

/// Pick a unique PDF font name on the page (e.g. CJK1, CJK2).
/// Honors `preferred` if it isn't taken.
fn choose_pdf_font_name(doc: &LopdfDocument, page_id: ObjectId, preferred: Option<&str>) -> String {
    let existing: HashSet<String> = doc
        .get_page_fonts(page_id)
        .map(|fonts| {
            fonts
                .into_keys()
                .map(|n| String::from_utf8_lossy(&n).to_string())
                .collect()
        })
        .unwrap_or_default();

    if let Some(name) = preferred {
        if !existing.contains(name) {
            return name.to_string();
        }
    }

    for i in 1..1000 {
        let candidate = format!("CJK{}", i);
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    "CJK_FALLBACK".to_string()
}

/// PDF Subset Prefix — 6 uppercase ASCII letters preceding `+`.
fn subset_prefix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xC0FFEE);
    let mut out = [b'A'; 6];
    for slot in out.iter_mut() {
        *slot = b'A' + (seed % 26) as u8;
        seed /= 26;
        if seed == 0 {
            seed = 0xDEADBEEF;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Build a ToUnicode CMap stream body that maps each new CID to its Unicode code point(s).
/// Standard Adobe Identity-UCS format.
fn build_tounicode_cmap(map: &HashMap<u16, Vec<char>>) -> Vec<u8> {
    let mut entries: Vec<(u16, Vec<char>)> = map.iter().map(|(k, v)| (*k, v.clone())).collect();
    entries.sort_by_key(|(k, _)| *k);

    let mut s = String::new();
    s.push_str("/CIDInit /ProcSet findresource begin\n");
    s.push_str("12 dict begin\n");
    s.push_str("begincmap\n");
    s.push_str(
        "/CIDSystemInfo <<\n  /Registry (Adobe)\n  /Ordering (UCS)\n  /Supplement 0\n>> def\n",
    );
    s.push_str("/CMapName /Adobe-Identity-UCS def\n");
    s.push_str("/CMapType 2 def\n");
    s.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");

    // Each chunk allows max 100 entries per beginbfchar.
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (cid, chars) in chunk {
            let mut hex_unicode = String::new();
            for ch in chars {
                let cp = *ch as u32;
                if cp <= 0xFFFF {
                    hex_unicode.push_str(&format!("{:04X}", cp));
                } else {
                    // UTF-16 surrogate pair
                    let cp_adj = cp - 0x10000;
                    let hi = 0xD800 + (cp_adj >> 10);
                    let lo = 0xDC00 + (cp_adj & 0x3FF);
                    hex_unicode.push_str(&format!("{:04X}{:04X}", hi, lo));
                }
            }
            s.push_str(&format!("<{:04X}> <{}>\n", cid, hex_unicode));
        }
        s.push_str("endbfchar\n");
    }

    s.push_str("endcmap\n");
    s.push_str("CMapName currentdict /CMap defineresource pop\n");
    s.push_str("end\nend\n");

    s.into_bytes()
}

/// Add the new font reference under `Resources/Font/<name>` on the page.
fn register_font_on_page(
    doc: &mut LopdfDocument,
    page_id: ObjectId,
    font_name: &str,
    font_obj_id: ObjectId,
) -> Result<(), HandlerError> {
    let resources = doc.get_or_create_resources(page_id).map_err(|e| {
        HandlerError::OperationFailed(format!("failed to get/create page resources: {:?}", e))
    })?;

    // `resources` may be a direct dict or a reference to a shared resources object.
    // We resolve to the actual mutable Dictionary.
    let resources_id_opt: Option<ObjectId> = match resources {
        Object::Reference(id) => Some(*id),
        _ => None,
    };

    if let Some(res_id) = resources_id_opt {
        let res_obj = doc.get_object_mut(res_id).map_err(|e| {
            HandlerError::OperationFailed(format!("resources obj missing: {:?}", e))
        })?;
        ensure_font_in_resources(res_obj, font_name, font_obj_id)?;
    } else {
        ensure_font_in_resources(resources, font_name, font_obj_id)?;
    }

    Ok(())
}

fn ensure_font_in_resources(
    resources: &mut Object,
    font_name: &str,
    font_obj_id: ObjectId,
) -> Result<(), HandlerError> {
    let dict = resources
        .as_dict_mut()
        .map_err(|e| HandlerError::OperationFailed(format!("resources not a dict: {:?}", e)))?;
    if !dict.has(b"Font") {
        dict.set("Font", Dictionary::new());
    }
    let font_dict = dict
        .get_mut(b"Font")
        .and_then(Object::as_dict_mut)
        .map_err(|e| HandlerError::OperationFailed(format!("/Font not a dict: {:?}", e)))?;
    font_dict.set(
        font_name.as_bytes().to_vec(),
        Object::Reference(font_obj_id),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_cjk_font_coverage_is_reported_exactly() {
        let font_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("NotoSansSC-Regular.ttf");
        assert!(font_file_covers_chars(&font_path, &HashSet::from(['中', 'A'])).unwrap());
        assert!(!font_file_covers_chars(&font_path, &HashSet::from(['ə', 'ʊ'])).unwrap());
    }
}
