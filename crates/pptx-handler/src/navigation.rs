use crate::dom_types::{Paragraph, Presentation, Run, Shape, Slide, NS_A, NS_P, NS_R};
use handler_common::PathSegment;

/// Parse a pptx path string into segments.
/// Paths:
///   /slide[N] — Nth slide
///   /slide[N]/shape[M] — Mth shape on Nth slide
///   /slide[N]/shape[M]/paragraph[K] — Kth paragraph
pub fn parse_path(path: &str) -> Vec<PathSegment> {
    if path.is_empty() || path == "/" {
        return Vec::new();
    }
    let path = path.strip_prefix('/').unwrap_or(path);
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|seg| {
            // Parse "slide[1]", "shape[3]", "paragraph[2]", etc.
            if let Some(bracket_start) = seg.find('[') {
                let name = &seg[..bracket_start];
                let bracket_content = &seg[bracket_start + 1..];
                if let Some(bracket_end) = bracket_content.find(']') {
                    let content = &bracket_content[..bracket_end];
                    if let Ok(idx) = content.parse::<usize>() {
                        PathSegment::new(name).with_index(idx)
                    } else {
                        PathSegment::new(seg)
                    }
                } else {
                    PathSegment::new(seg)
                }
            } else {
                PathSegment::new(seg)
            }
        })
        .collect()
}

/// Build the Presentation model by parsing presentation.xml and each slide.
pub fn build_presentation(
    package: &oxml::OxmlPackage,
) -> Result<Presentation, handler_common::HandlerError> {
    // 1. Read presentation.xml
    let pres_xml = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|e| handler_common::HandlerError::OperationFailed(e.to_string()))?;

    // 2. Parse <p:sldIdLst> to get slide IDs and their rId targets
    let slide_id_entries = parse_slide_id_list(&pres_xml)?;

    // 3. Read presentation.xml.rels to resolve rId -> part path
    let pres_rels = package
        .part_rels("ppt/presentation.xml")
        .map_err(|e| handler_common::HandlerError::OperationFailed(e.to_string()))?;

    // 4. For each slide ID entry, resolve the part path and parse the slide
    let mut slides = Vec::new();
    for (idx, entry) in slide_id_entries.iter().enumerate() {
        let slide_index = idx + 1; // 1-based

        // Resolve rId to target path
        let rel = pres_rels.get(&entry.r_id);
        if rel.is_none() {
            continue;
        }
        let rel = rel.unwrap();
        let target = package.resolve_rel_target("ppt/presentation.xml", &rel.target);

        // Read and parse the slide XML
        let slide_xml = match package.read_part_xml(&target) {
            Ok(xml) => xml,
            Err(_) => continue, // Skip missing slides
        };

        let shapes = parse_slide_shapes(&slide_xml);
        let (has_morph, morph_candidates) = detect_morph_transition(&slide_xml);
        slides.push(Slide {
            index: slide_index,
            part_path: target,
            slide_id: entry.id.clone(),
            shapes,
            has_morph,
            morph_candidates,
        });
    }

    Ok(Presentation { slides })
}

/// Resolve a 1-based logical slide index to its actual OOXML part path.
///
/// Physical part names are not required to be contiguous. After deleting or
/// reordering slides, logical `/slide[2]` may point to `slide3.xml`, so callers
/// must resolve through presentation relationships instead of constructing a
/// filename from the logical index.
pub fn resolve_slide_part_path(
    package: &oxml::OxmlPackage,
    slide_index: usize,
) -> Result<String, handler_common::HandlerError> {
    if slide_index == 0 {
        return Err(handler_common::HandlerError::InvalidPath(
            "slide indices are 1-based".to_string(),
        ));
    }
    build_presentation(package)?
        .slides
        .into_iter()
        .find(|slide| slide.index == slide_index)
        .map(|slide| slide.part_path)
        .ok_or_else(|| handler_common::HandlerError::PathNotFound(format!("slide {}", slide_index)))
}

/// Return the package relationship-part path for an OOXML part.
pub fn relationships_part_path(part_path: &str) -> String {
    match part_path.rsplit_once('/') {
        Some((directory, file_name)) => {
            format!("{}/_rels/{}.rels", directory, file_name)
        }
        None => format!("_rels/{}.rels", part_path),
    }
}

/// Parse <p:sldIdLst> from presentation.xml.
/// Uses roxmltree for namespace-aware attribute parsing (r:id requires namespace resolution).
fn parse_slide_id_list(
    xml: &str,
) -> Result<Vec<crate::dom_types::SlideIdEntry>, handler_common::HandlerError> {
    // Directly use roxmltree — it handles namespace-qualified attributes correctly
    parse_slide_id_list_roxml(xml)
}

/// Fallback: use roxmltree for namespace-aware parsing of sldIdLst.
fn parse_slide_id_list_roxml(
    xml: &str,
) -> Result<Vec<crate::dom_types::SlideIdEntry>, handler_common::HandlerError> {
    let mut entries = Vec::new();

    let doc = roxmltree::Document::parse(xml).map_err(|e| {
        handler_common::HandlerError::OperationFailed(format!("roxmltree parse error: {}", e))
    })?;

    // Find <p:sldIdLst> element
    let sld_id_lst = doc
        .descendants()
        .find(|n| n.has_tag_name((NS_P, "sldIdLst")));

    // Also try without namespace
    let sld_id_lst = sld_id_lst.or_else(|| doc.descendants().find(|n| n.has_tag_name("sldIdLst")));

    if let Some(lst) = sld_id_lst {
        for child in lst.children() {
            if child.has_tag_name((NS_P, "sldId")) || child.has_tag_name("sldId") {
                let id = child.attribute("id").unwrap_or("").to_string();
                // The r:id attribute in OOXML is namespaced
                let r_id = child
                    .attribute((NS_R, "id"))
                    .or_else(|| child.attribute("r:id"))
                    .unwrap_or("")
                    .to_string();
                if !id.is_empty() && !r_id.is_empty() {
                    entries.push(crate::dom_types::SlideIdEntry { id, r_id });
                }
            }
        }
    }

    Ok(entries)
}

/// Parse shapes from a slide XML document.
pub fn parse_slide_shapes(xml: &str) -> Vec<Shape> {
    let mut shapes = Vec::new();

    // Use roxmltree for namespace-aware parsing
    let doc = match roxmltree::Document::parse(xml) {
        Ok(d) => d,
        Err(_) => return shapes,
    };

    // Find the <p:spTree> element (shape tree)
    let sp_tree = doc
        .descendants()
        .find(|n| n.has_tag_name((NS_P, "spTree")))
        .or_else(|| doc.descendants().find(|n| n.has_tag_name("spTree")));

    if let Some(tree) = sp_tree {
        for child in tree.children() {
            // Look for <p:sp> (shape) elements
            if child.has_tag_name((NS_P, "sp")) || child.has_tag_name("sp") {
                if let Some(shape) = parse_shape_node(&child) {
                    shapes.push(shape);
                }
            }
            // Also look for <p:graphicFrame> (tables, charts, etc.)
            // and <p:pic> (pictures) — skip those for now, focus on text shapes
        }
    }

    shapes
}

/// Parse a single <p:sp> element into a Shape.
fn parse_shape_node(sp: &roxmltree::Node) -> Option<Shape> {
    let mut name = String::new();
    let mut id = String::new();
    let mut placeholder_type = None;
    let mut paragraphs = Vec::new();
    let mut full_text = String::new();

    // Find <p:nvSpPr> for name, id, and placeholder info
    for child in sp.children() {
        if child.has_tag_name((NS_P, "nvSpPr")) || child.has_tag_name("nvSpPr") {
            for nv_child in child.children() {
                // <p:cNvPr id="2" name="Title 1">
                if nv_child.has_tag_name((NS_P, "cNvPr")) || nv_child.has_tag_name("cNvPr") {
                    id = nv_child.attribute("id").unwrap_or("").to_string();
                    name = nv_child.attribute("name").unwrap_or("").to_string();
                }
                // <p:nvPr> — check for <p:ph> placeholder
                if nv_child.has_tag_name((NS_P, "nvPr")) || nv_child.has_tag_name("nvPr") {
                    for ph_child in nv_child.children() {
                        if ph_child.has_tag_name((NS_P, "ph")) || ph_child.has_tag_name("ph") {
                            placeholder_type = ph_child.attribute("type").map(|t| t.to_string());
                        }
                    }
                }
            }
        }

        // Find <p:txBody> (text body) — contains <a:p> paragraphs
        if child.has_tag_name((NS_P, "txBody")) || child.has_tag_name("txBody") {
            for p_node in child.children() {
                // <a:p> paragraphs
                if p_node.has_tag_name((NS_A, "p")) || p_node.has_tag_name("p") {
                    let para = parse_paragraph_node(&p_node);
                    if !para.text.is_empty() || !para.runs.is_empty() {
                        if !full_text.is_empty() {
                            full_text.push('\n');
                        }
                        full_text.push_str(&para.text);
                        paragraphs.push(para);
                    }
                }
            }
        }
    }

    Some(Shape {
        name,
        id,
        placeholder_type,
        text: full_text,
        paragraphs,
    })
}

/// Parse an <a:p> paragraph node.
fn parse_paragraph_node(p: &roxmltree::Node) -> Paragraph {
    let mut runs = Vec::new();
    let mut text = String::new();

    for child in p.children() {
        // <a:r> runs
        if child.has_tag_name((NS_A, "r")) || child.has_tag_name("r") {
            let run = parse_run_node(&child);
            text.push_str(&run.text);
            runs.push(run);
        }
        // <a:fld> fields (e.g. slide number) — extract text if present
        if child.has_tag_name((NS_A, "fld")) || child.has_tag_name("fld") {
            let run = parse_field_node(&child);
            text.push_str(&run.text);
            runs.push(run);
        }
    }

    Paragraph { text, runs }
}

/// Parse an <a:r> run node.
fn parse_run_node(r: &roxmltree::Node) -> Run {
    let mut text = String::new();

    for child in r.children() {
        // <a:t> text element
        if child.has_tag_name((NS_A, "t")) || child.has_tag_name("t") {
            text.push_str(child.text().unwrap_or(""));
        }
    }

    Run { text }
}

/// Parse an <a:fld> field node.
fn parse_field_node(fld: &roxmltree::Node) -> Run {
    let mut text = String::new();

    for child in fld.children() {
        // <a:t> text element within field
        if child.has_tag_name((NS_A, "t")) || child.has_tag_name("t") {
            text.push_str(child.text().unwrap_or(""));
        }
    }

    Run { text }
}

/// Find a slide by 1-based index.
pub fn find_slide(pres: &Presentation, index: usize) -> Option<&Slide> {
    pres.slides.iter().find(|s| s.index == index)
}

/// Find a shape by 1-based index within a slide.
pub fn find_shape(slide: &Slide, index: usize) -> Option<&Shape> {
    if index > 0 && index <= slide.shapes.len() {
        Some(&slide.shapes[index - 1])
    } else {
        None
    }
}

/// Find a paragraph by 1-based index within a shape.
pub fn find_paragraph(shape: &Shape, index: usize) -> Option<&Paragraph> {
    if index > 0 && index <= shape.paragraphs.len() {
        Some(&shape.paragraphs[index - 1])
    } else {
        None
    }
}

/// Detect morph transition on a slide.
/// Looks for <p14:morphPr> element in the slide XML transition section.
/// Returns (has_morph, morph_candidates_count).
fn detect_morph_transition(slide_xml: &str) -> (bool, usize) {
    // Check if the slide XML contains a morph transition element.
    // Morph transitions use the p14 namespace (http://schemas.microsoft.com/office/powerpoint/2010/main)
    // and appear as <p14:morphPr> inside <p:transition>.
    // Also check for <mc:AlternateContent> with morphPr fallback.

    let has_morph = slide_xml.contains("morphPr")
        && (slide_xml.contains("<p:transition") || slide_xml.contains("<p14:transition"));

    if !has_morph {
        return (false, 0);
    }

    // Count morph candidates: shapes that have matching names on adjacent slides.
    // In a morph transition, shapes with the same name on consecutive slides
    // are morph candidates. We count shapes with non-empty names as candidates.
    let doc = match roxmltree::Document::parse(slide_xml) {
        Ok(d) => d,
        Err(_) => return (true, 0),
    };

    // Count shapes that could participate in morph (those with name attributes)
    let mut candidates = 0;
    if let Some(sp_tree) = doc
        .descendants()
        .find(|n| n.has_tag_name((NS_P, "spTree")))
        .or_else(|| doc.descendants().find(|n| n.has_tag_name("spTree")))
    {
        for child in sp_tree.children() {
            let is_sp = child.has_tag_name((NS_P, "sp")) || child.has_tag_name("sp");
            let is_pic = child.has_tag_name((NS_P, "pic")) || child.has_tag_name("pic");
            let is_grp = child.has_tag_name((NS_P, "grpSp")) || child.has_tag_name("grpSp");
            if is_sp || is_pic || is_grp {
                // Check for a cNvPr with a name (morph matches by name)
                let has_name = child.descendants().any(|n| {
                    (n.has_tag_name((NS_P, "cNvPr")) || n.has_tag_name("cNvPr"))
                        && n.attribute("name").is_some()
                });
                if has_name {
                    candidates += 1;
                }
            }
        }
    }

    (true, candidates)
}
