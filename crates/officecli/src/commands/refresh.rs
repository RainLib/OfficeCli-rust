use clap::Args;
use handler_common::{HandlerError, OutputFormat};
use std::collections::HashMap;

/// Refresh derived field values (TOC page numbers, cross-references). HTML fallback only.
#[derive(Args)]
pub struct RefreshCommand {
    /// Document file path (.docx only)
    pub file: String,
}

pub fn handle_refresh(cmd: RefreshCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    if crate::resident_available(&cmd.file) {
        return Err(HandlerError::OperationFailed(
            "refresh cannot modify a document while it is open in resident mode; run `officecli save` and `officecli close` first"
                .to_string(),
        ));
    }
    let ext = std::path::Path::new(&cmd.file)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if ext != "docx" && ext != "docm" {
        return Err(HandlerError::UnsupportedType(format!(
            "refresh currently only supports .docx files (got .{})",
            ext
        )));
    }

    let handler = crate::open_handler(&cmd.file, true)?;

    // Render the document's HTML to compute an approximate page map from
    // heading positions. This will not match Word's exact pagination (which
    // depends on installed fonts, page settings, and live rendering) but
    // gives a usable approximation for downstream tools that just need a
    // stable page number for each heading.
    let html = handler.view_as_html(handler_common::ViewOptions::default())?;
    let page_map = compute_page_map_from_html(&html);

    // Walk the docx package directly and rewrite every PAGEREF field's
    // cached page number with the new estimate. The handler's set/add
    // operations are too coarse for this — we need byte-level surgery on
    // <w:instrText>PAGEREF _Toc... \h</w:instrText> + adjacent <w:t>NN</w:t>.
    let (updated, total) = update_pagerefs(&cmd.file, &page_map)?;

    handler.save()?;
    eprintln!(
        "Note: HTML fallback used. TOC page numbers reflect officecli's HTML pagination, \
         which may differ from Word's layout."
    );

    if total == 0 {
        Ok(format!(
            "Refreshed: {} (no PAGEREF fields to update — TOC may need to be opened in Word first)",
            cmd.file
        ))
    } else if updated == 0 {
        Ok(format!(
            "Refreshed: {} (found {} PAGEREF field(s), none matched known headings)",
            cmd.file, total
        ))
    } else {
        Ok(format!(
            "Refreshed: {} (updated {}/{} PAGEREF field(s))",
            cmd.file, updated, total
        ))
    }
}

/// Build a (heading text → page number) map from the HTML preview by
/// counting `<h1>`/`<h2>`/`<h3>` elements and bucketing them at a fixed
/// rate per page (45 lines ≈ one page of single-spaced 12pt text).
fn compute_page_map_from_html(html: &str) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    let mut line_count = 0;
    let lines_per_page = 45;
    let mut current_page = 1;
    for line in html.lines() {
        if line.contains("<h1") || line.contains("<h2") || line.contains("<h3") {
            let anchor = extract_heading_text(line);
            if !anchor.is_empty() {
                map.insert(anchor, current_page);
            }
        }
        line_count += 1;
        if line_count >= lines_per_page {
            line_count = 0;
            current_page += 1;
        }
    }
    map
}

fn extract_heading_text(html_line: &str) -> String {
    if let Some(start) = html_line.find('>') {
        if let Some(end) = html_line[start + 1..].find('<') {
            return html_line[start + 1..start + 1 + end].trim().to_string();
        }
    }
    String::new()
}

/// Walk the document.xml of `path` and update every PAGEREF field's cached
/// page number to the new estimate from `page_map`. Returns `(updated, total)`.
///
/// PAGEREF fields look like:
///   <w:fldSimple w:instr=" PAGEREF _Toc12345 \h ">
///     <w:r><w:t>7</w:t></w:r>
///   </w:fldSimple>
/// or as fldChar-wrapped fields:
///   <w:instrText> PAGEREF _Toc12345 \h </w:instrText>
///   ... <w:r><w:t>7</w:t></w:r> ... <w:fldChar w:fldCharType="end"/>
///
/// We rewrite the nearest `<w:t>NN</w:t>` inside the same field. To stay
/// robust without a full XML parser, we treat the TOC region as
/// `PAGEREF ... </w:t>`-bounded runs and update the digit immediately
/// preceding the field-close.
fn update_pagerefs(
    file_path: &str,
    page_map: &HashMap<String, usize>,
) -> Result<(usize, usize), HandlerError> {
    use oxml::OxmlPackage;
    let mut pkg = OxmlPackage::open(file_path, true)
        .map_err(|e| HandlerError::OperationFailed(format!("open docx: {}", e)))?;
    let document = pkg
        .read_part_xml("word/document.xml")
        .map_err(|e| HandlerError::OperationFailed(format!("read document.xml: {}", e)))?;

    let mut updated = 0usize;
    let mut total = 0usize;
    let mut out = String::with_capacity(document.len());
    let mut cursor = 0;

    while let Some(rel_pageref_start) = document[cursor..].find("PAGEREF ") {
        let abs_start = cursor + rel_pageref_start;
        // Pull the bookmark name — token after PAGEREF.
        let after = &document[abs_start + "PAGEREF ".len()..];
        let bookmark_end = after
            .find(|c: char| c.is_whitespace())
            .unwrap_or(after.len().min(64));
        let bookmark = after[..bookmark_end].trim();
        total += 1;

        // Find the next field-close (fldChar end OR fldSimple close).
        let search_end_rel = document[abs_start..]
            .find("fldCharType=\"end\"")
            .or_else(|| document[abs_start..].find("</w:fldSimple>"));
        let field_end_abs = match search_end_rel {
            Some(p) => abs_start + p,
            None => {
                out.push_str(&document[cursor..abs_start]);
                cursor = abs_start;
                continue;
            }
        };

        // Find the last <w:t>NN</w:t> between abs_start and field_end_abs.
        let region = &document[abs_start..field_end_abs];
        let last_wt = region.rfind("<w:t");
        let new_page = lookup_page_for(bookmark, page_map);

        out.push_str(&document[cursor..abs_start]);
        if let (Some(wt_rel), Some(p)) = (last_wt, new_page) {
            let wt_abs = abs_start + wt_rel;
            // Find the closing </w:t> after wt_abs.
            if let Some(close_rel) = document[wt_abs..].find("</w:t>") {
                let close_abs = wt_abs + close_rel;
                // Replace everything between the first '>' after <w:t and </w:t>.
                let open_tag_close = document[wt_abs..close_abs]
                    .find('>')
                    .map(|p| wt_abs + p + 1)
                    .unwrap_or(wt_abs);
                out.push_str(&document[abs_start..open_tag_close]);
                out.push_str(&p.to_string());
                out.push_str("</w:t>");
                cursor = close_abs + "</w:t>".len();
                updated += 1;
                continue;
            }
        }
        // Couldn't locate page text — keep region as-is.
        out.push_str(&document[abs_start..field_end_abs]);
        cursor = field_end_abs;
    }
    out.push_str(&document[cursor..]);

    if updated > 0 {
        pkg.write_part_xml("word/document.xml", &out)
            .map_err(|e| HandlerError::OperationFailed(format!("write document.xml: {}", e)))?;
        pkg.save_as(file_path)
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    }

    Ok((updated, total))
}

/// Bookmark names in PAGEREF are typically `_Toc12345` style identifiers
/// that the page_map doesn't know about (it's keyed on heading text). Until
/// we maintain a heading-text→bookmark map, we cannot reliably resolve a
/// PAGEREF to a page. This helper maps only when the bookmark itself appears
/// as a heading text in the page_map; otherwise returns None.
fn lookup_page_for(bookmark: &str, page_map: &HashMap<String, usize>) -> Option<usize> {
    page_map.get(bookmark).copied().or_else(|| {
        // If the bookmark name appears as a substring of a known heading,
        // treat that as a match. Heuristic — only matches when the bookmark
        // is descriptive, which is uncommon for _Toc auto-bookmarks.
        page_map
            .iter()
            .find(|(heading, _)| heading.contains(bookmark) || bookmark.contains(heading.as_str()))
            .map(|(_, p)| *p)
    })
}
