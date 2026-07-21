use handler_common::HandlerError;
use handler_common::InsertPosition;
use oxml::OxmlPackage;
use std::ops::Range;

/// Remove an element from the PPTX presentation.
pub fn remove_element(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<Option<String>, HandlerError> {
    if path.starts_with("/slide[") && !path.contains("/shape") {
        // Remove entire slide
        let slide_num = parse_slide_num(path)?;
        remove_slide(package, slide_num)?;
        Ok(Some(format!("removed slide {}", slide_num)))
    } else if path.contains("/shape") {
        // Remove a shape from a slide
        let slide_num = parse_slide_num_from_full_path(path)?;
        let shape_idx = parse_shape_idx(path)?;
        remove_shape(package, slide_num, shape_idx)?;
        Ok(Some(format!(
            "removed shape {} from slide {}",
            shape_idx, slide_num
        )))
    } else {
        Err(HandlerError::InvalidPath(format!(
            "PPTX remove path must be /slide[N] or /slide[N]/shape[M]: {}",
            path
        )))
    }
}

/// Move a slide to a different position in the presentation.
/// Reorders the <p:sldIdLst> in presentation.xml.
pub fn move_slide(
    package: &mut OxmlPackage,
    source: &str,
    _target_parent: Option<&str>,
    position: InsertPosition,
) -> Result<String, HandlerError> {
    // Parse source slide number
    let source_num = parse_slide_num(source)?;

    // Determine target position
    let target_num = match position {
        InsertPosition::AfterElement(anchor) => parse_slide_num(&anchor)? + 1,
        InsertPosition::BeforeElement(anchor) => parse_slide_num(&anchor)?,
        InsertPosition::AtIndex(idx) => idx,
        InsertPosition::Append => {
            let pres = crate::navigation::build_presentation(package)?;
            pres.slides.len() + 1
        }
    };

    // Reorder the slide ID list in presentation.xml
    let pres_xml = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let modified = reorder_sld_id_list(&pres_xml, source_num, target_num)?;

    package
        .write_part_xml("ppt/presentation.xml", &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(format!("/slide[{}]", target_num))
}

/// Copy a slide from source to a new position.
/// Creates a duplicate slide part and adds it to the presentation.
pub fn copy_slide(
    package: &mut OxmlPackage,
    source: &str,
    _target_parent: &str,
    _position: InsertPosition,
) -> Result<String, HandlerError> {
    let source_num = parse_slide_num(source)?;

    let pres = crate::navigation::build_presentation(package)?;
    let source_slide = pres
        .slides
        .iter()
        .find(|s| s.index == source_num)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", source_num)))?;

    // Read the source slide XML
    let source_xml = package
        .read_part_xml(&source_slide.part_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Create a new slide at the end
    let new_slide_index = pres.slides.len() + 1;
    let new_slide_part_number = crate::add::next_slide_part_number(package);
    let new_slide_path = format!("ppt/slides/slide{}.xml", new_slide_part_number);

    // Write the copied slide content
    package
        .write_part_xml(&new_slide_path, &source_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Update presentation.xml to add the new slide reference
    crate::add::update_presentation_slides(package, new_slide_part_number)?;
    crate::add::register_slide_content_type(package, &new_slide_path)?;

    Ok(format!("/slide[{}]", new_slide_index))
}

/// Reorder the sldIdLst in presentation.xml by moving an entry from source to target position.
fn reorder_sld_id_list(xml: &str, source: usize, target: usize) -> Result<String, HandlerError> {
    let mut entries: Vec<(usize, String)> = Vec::new(); // (position, entry_xml)
    let mut search_from = 0;

    while let Some(start) = xml[search_from..].find("<p:sldId") {
        let abs_start = search_from + start;
        // Find end of the element (self-closing /> or regular </p:sldId>)
        let end = if let Some(pos) = xml[abs_start..].find("/>") {
            abs_start + pos + 2
        } else if let Some(pos) = xml[abs_start..].find("</p:sldId>") {
            abs_start + pos + "</p:sldId>".len()
        } else if let Some(pos) = xml[abs_start..].find(">") {
            abs_start + pos + 1
        } else {
            xml.len()
        };

        entries.push((abs_start, xml[abs_start..end].to_string()));
        search_from = end;
    }

    if entries.len() < source {
        return Err(HandlerError::InvalidPath(format!(
            "slide {} not found in sldIdLst",
            source
        )));
    }

    // Remove the source entry (1-based → 0-based)
    let removed_entry = entries.remove(source - 1).1;

    // Insert at target position (target can be > len, meaning append)
    let _ = if target > entries.len() {
        entries.len()
    } else {
        target - 1 // 1-based → 0-based, but clamp to at least 0
    };
    // The insert_pos should be based on the NEW list length (after removal)
    let adjusted_pos = (target - 1).min(entries.len());
    entries.insert(adjusted_pos, (0, removed_entry));

    // Rebuild the XML by replacing the sldIdLst content
    // Find </p:sldIdLst> and reconstruct entries before it
    let sld_id_lst_end = xml
        .find("</p:sldIdLst>")
        .ok_or_else(|| HandlerError::OperationFailed("no </p:sldIdLst> found".to_string()))?;

    // Find <p:sldIdLst> start
    let sld_id_lst_start = xml
        .find("<p:sldIdLst")
        .ok_or_else(|| HandlerError::OperationFailed("no <p:sldIdLst> found".to_string()))?;

    // Find the end of the opening tag of <p:sldIdLst>
    let sld_id_lst_tag_end = xml[sld_id_lst_start..]
        .find('>')
        .map(|pos| sld_id_lst_start + pos + 1)
        .ok_or_else(|| HandlerError::OperationFailed("malformed <p:sldIdLst>".to_string()))?;

    // Build the new entries section
    let new_entries = entries
        .iter()
        .map(|(_, entry)| entry.clone())
        .collect::<Vec<String>>()
        .join("\n    ");

    let mut result = xml[..sld_id_lst_tag_end].to_string();
    result.push_str("\n    ");
    result.push_str(&new_entries);
    result.push_str("\n  ");
    result.push_str(&xml[sld_id_lst_end..]);

    Ok(result)
}

fn remove_slide(package: &mut OxmlPackage, slide_num: usize) -> Result<(), HandlerError> {
    let presentation_path = "ppt/presentation.xml";
    let presentation_rels_path = "ppt/_rels/presentation.xml.rels";
    let content_types_path = "[Content_Types].xml";

    let presentation_xml = package
        .read_part_xml(presentation_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let (updated_presentation, relationship_id) =
        remove_slide_references(&presentation_xml, slide_num)?;

    let presentation_rels = package
        .part_rels(presentation_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let relationship = presentation_rels.get(&relationship_id).ok_or_else(|| {
        HandlerError::OperationFailed(format!(
            "slide {} references missing relationship {}",
            slide_num, relationship_id
        ))
    })?;
    let slide_path = package.resolve_rel_target(presentation_path, &relationship.target);
    if !package.has_part(&slide_path) {
        return Err(HandlerError::OperationFailed(format!(
            "slide {} part not found: {}",
            slide_num, slide_path
        )));
    }

    let presentation_rels_xml = package
        .read_part_xml(presentation_rels_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let updated_presentation_rels = remove_relationship(&presentation_rels_xml, &relationship_id)?;

    let content_types_xml = package
        .read_part_xml(content_types_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    let updated_content_types = remove_content_type_override(&content_types_xml, &slide_path)?;

    package
        .write_part_xml(presentation_path, &updated_presentation)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    package
        .write_part_xml(presentation_rels_path, &updated_presentation_rels)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    package
        .write_part_xml(content_types_path, &updated_content_types)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    package
        .remove_part(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    package
        .remove_part(&crate::navigation::relationships_part_path(&slide_path))
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(())
}

/// Remove the selected slide ID and any custom-show entries that reference it.
///
/// A custom show with no remaining slides is removed, and an empty custom-show
/// list is removed as well. Leaving these references dangling causes PowerPoint
/// to reject the presentation after the slide relationship is deleted.
fn remove_slide_references(xml: &str, slide_num: usize) -> Result<(String, String), HandlerError> {
    if slide_num == 0 {
        return Err(HandlerError::InvalidPath(
            "slide indices are 1-based".to_string(),
        ));
    }

    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid presentation.xml: {}", e)))?;
    let slide_id_list = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "sldIdLst")
        .ok_or_else(|| {
            HandlerError::OperationFailed("presentation has no slide ID list".to_string())
        })?;
    let slide_id = slide_id_list
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "sldId")
        .nth(slide_num - 1)
        .ok_or_else(|| HandlerError::PathNotFound(format!("slide {}", slide_num)))?;
    let relationship_id = relationship_id_of(&slide_id).ok_or_else(|| {
        HandlerError::OperationFailed(format!("slide {} has no relationship ID", slide_num))
    })?;

    let mut ranges = vec![slide_id.range()];
    if let Some(custom_show_list) = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "custShowLst")
    {
        let shows: Vec<_> = custom_show_list
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "custShow")
            .collect();
        let mut removed_shows = 0;
        let mut show_ranges = Vec::new();
        let mut entry_ranges = Vec::new();

        for show in &shows {
            let entries: Vec<_> = show
                .descendants()
                .filter(|node| node.is_element() && node.tag_name().name() == "sld")
                .collect();
            let matching: Vec<_> = entries
                .iter()
                .filter(|node| {
                    relationship_id_of(node).as_deref() == Some(relationship_id.as_str())
                })
                .collect();

            if entries.len() == matching.len() {
                show_ranges.push(show.range());
                removed_shows += 1;
            } else {
                entry_ranges.extend(matching.into_iter().map(|node| node.range()));
            }
        }

        if removed_shows == shows.len() {
            ranges.push(custom_show_list.range());
        } else {
            ranges.extend(show_ranges);
            ranges.extend(entry_ranges);
        }
    }

    Ok((remove_xml_ranges(xml, ranges), relationship_id))
}

fn remove_relationship(xml: &str, relationship_id: &str) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| {
        HandlerError::OperationFailed(format!("invalid presentation relationships: {}", e))
    })?;
    let relationship = doc
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Relationship"
                && node.attribute("Id") == Some(relationship_id)
        })
        .ok_or_else(|| {
            HandlerError::OperationFailed(format!(
                "presentation relationship {} not found",
                relationship_id
            ))
        })?;
    Ok(remove_xml_ranges(xml, vec![relationship.range()]))
}

fn remove_content_type_override(xml: &str, slide_path: &str) -> Result<String, HandlerError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("invalid content types: {}", e)))?;
    let part_name = format!("/{}", slide_path.trim_start_matches('/'));
    let ranges = doc
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == "Override"
                && node.attribute("PartName") == Some(part_name.as_str())
        })
        .map(|node| node.range())
        .collect();
    Ok(remove_xml_ranges(xml, ranges))
}

fn relationship_id_of(node: &roxmltree::Node<'_, '_>) -> Option<String> {
    const RELATIONSHIPS_NS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    node.attribute((RELATIONSHIPS_NS, "id"))
        .or_else(|| node.attribute("r:id"))
        .map(str::to_string)
}

fn remove_xml_ranges(xml: &str, mut ranges: Vec<Range<usize>>) -> String {
    ranges.sort_by(|left, right| right.start.cmp(&left.start));
    let mut result = xml.to_string();
    let mut last_start = xml.len();
    for range in ranges {
        if range.end <= last_start {
            result.replace_range(range.clone(), "");
            last_start = range.start;
        }
    }
    result
}

fn remove_shape(
    package: &mut OxmlPackage,
    slide_num: usize,
    shape_idx: usize,
) -> Result<(), HandlerError> {
    let slide_path = crate::navigation::resolve_slide_part_path(package, slide_num)?;

    let slide_xml = package
        .read_part_xml(&slide_path)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Find and remove the Nth <p:sp> element
    let modified = remove_nth_sp(&slide_xml, shape_idx);

    package
        .write_part_xml(&slide_path, &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok(())
}

fn remove_nth_sp(xml: &str, n: usize) -> String {
    let mut result = xml.to_string();
    let mut count = 0;
    let mut search_start = 0;

    while let Some(start) = result[search_start..].find("<p:sp>") {
        let abs_start = search_start + start;
        if let Some(end) = result[abs_start..].find("</p:sp>") {
            let abs_end = abs_start + end + 6; // length of "</p:sp>"
            count += 1;
            if count == n {
                result.replace_range(abs_start..abs_end, "");
                break;
            }
            search_start = abs_end;
        } else {
            break;
        }
    }

    result
}

/// Swap two slides in the presentation.
/// Reorders the <p:sldIdLst> in presentation.xml by exchanging entries at positions a and b.
pub fn swap_slides(
    package: &mut OxmlPackage,
    path1: &str,
    path2: &str,
) -> Result<(String, String), HandlerError> {
    let a = parse_slide_num(path1)?;
    let b = parse_slide_num(path2)?;

    if a == b {
        return Err(HandlerError::InvalidArgument(format!(
            "swap requires two different slides, both were {}",
            a
        )));
    }

    let pres_xml = package
        .read_part_xml("ppt/presentation.xml")
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    let modified = swap_sld_id_list_entries(&pres_xml, a, b)?;

    package
        .write_part_xml("ppt/presentation.xml", &modified)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    Ok((format!("/slide[{}]", a), format!("/slide[{}]", b)))
}

/// Swap two entries in the sldIdLst (1-based indices).
fn swap_sld_id_list_entries(xml: &str, a: usize, b: usize) -> Result<String, HandlerError> {
    // Collect all <p:sldId .../> entries
    let mut entries: Vec<String> = Vec::new();
    let mut search_from = 0;

    while let Some(start) = xml[search_from..].find("<p:sldId") {
        let abs_start = search_from + start;
        let end = if let Some(pos) = xml[abs_start..].find("/>") {
            abs_start + pos + 2
        } else if let Some(pos) = xml[abs_start..].find("</p:sldId>") {
            abs_start + pos + "</p:sldId>".len()
        } else if let Some(pos) = xml[abs_start..].find('>') {
            abs_start + pos + 1
        } else {
            xml.len()
        };

        entries.push(xml[abs_start..end].to_string());
        search_from = end;
    }

    if a < 1 || a > entries.len() {
        return Err(HandlerError::InvalidPath(format!(
            "slide {} not found in sldIdLst",
            a
        )));
    }
    if b < 1 || b > entries.len() {
        return Err(HandlerError::InvalidPath(format!(
            "slide {} not found in sldIdLst",
            b
        )));
    }

    // Swap (1-based → 0-based)
    entries.swap(a - 1, b - 1);

    // Rebuild the XML
    let sld_id_lst_start = xml
        .find("<p:sldIdLst")
        .ok_or_else(|| HandlerError::OperationFailed("no <p:sldIdLst> found".to_string()))?;
    let sld_id_lst_tag_end = xml[sld_id_lst_start..]
        .find('>')
        .map(|pos| sld_id_lst_start + pos + 1)
        .ok_or_else(|| HandlerError::OperationFailed("malformed <p:sldIdLst>".to_string()))?;
    let sld_id_lst_end = xml
        .find("</p:sldIdLst>")
        .ok_or_else(|| HandlerError::OperationFailed("no </p:sldIdLst> found".to_string()))?;

    let new_entries = entries.join("\n    ");
    let mut result = xml[..sld_id_lst_tag_end].to_string();
    result.push_str("\n    ");
    result.push_str(&new_entries);
    result.push_str("\n  ");
    result.push_str(&xml[sld_id_lst_end..]);

    Ok(result)
}

fn parse_slide_num(path: &str) -> Result<usize, HandlerError> {
    path.strip_prefix("/slide[")
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| HandlerError::InvalidPath(format!("expected /slide[N], got: {}", path)))
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

#[cfg(test)]
mod tests {
    use super::*;

    const PRESENTATION_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
 xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst>
    <p:sldId id="256" r:id="rId2"/>
    <p:sldId id="257" r:id="rId5"/>
  </p:sldIdLst>
  <p:custShowLst>
    <p:custShow name="keep" id="1"><p:sldLst><p:sld r:id="rId2"/><p:sld r:id="rId5"/></p:sldLst></p:custShow>
    <p:custShow name="drop" id="2"><p:sldLst><p:sld r:id="rId5"/></p:sldLst></p:custShow>
  </p:custShowLst>
</p:presentation>"#;

    const PRESENTATION_RELS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId2" Type="slide" Target="slides/slide1.xml"/>
  <Relationship Id="rId5" Type="slide" Target="slides/slide7.xml"/>
</Relationships>"#;

    const CONTENT_TYPES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Override PartName="/ppt/slides/slide1.xml" ContentType="slide"/>
  <Override PartName="/ppt/slides/slide7.xml" ContentType="slide"/>
</Types>"#;

    #[test]
    fn remove_slide_references_prunes_custom_shows() {
        let (updated, relationship_id) = remove_slide_references(PRESENTATION_XML, 2).unwrap();

        assert_eq!(relationship_id, "rId5");
        assert!(!updated.contains(r#"id="257""#));
        assert!(!updated.contains(r#"r:id="rId5""#));
        assert!(!updated.contains(r#"name="drop""#));
        assert!(updated.contains(r#"name="keep""#));
        assert!(updated.contains(r#"r:id="rId2""#));
        roxmltree::Document::parse(&updated).unwrap();
    }

    #[test]
    fn remove_slide_uses_relationship_target_and_removes_package_metadata() {
        let mut package = OxmlPackage::create("unused.pptx");
        package.add_part("ppt/presentation.xml", PRESENTATION_XML.as_bytes());
        package.add_part(
            "ppt/_rels/presentation.xml.rels",
            PRESENTATION_RELS_XML.as_bytes(),
        );
        package.add_part("[Content_Types].xml", CONTENT_TYPES_XML.as_bytes());
        package.add_part("ppt/slides/slide1.xml", b"<p:sld/>");
        package.add_part("ppt/slides/slide7.xml", b"<p:sld/>");
        package.add_part("ppt/slides/_rels/slide7.xml.rels", b"<Relationships/>");

        remove_slide(&mut package, 2).unwrap();

        assert!(package.has_part("ppt/slides/slide1.xml"));
        assert!(!package.has_part("ppt/slides/slide7.xml"));
        assert!(!package.has_part("ppt/slides/_rels/slide7.xml.rels"));
        assert!(!package
            .read_part_xml("ppt/presentation.xml")
            .unwrap()
            .contains("rId5"));
        assert!(!package
            .read_part_xml("ppt/_rels/presentation.xml.rels")
            .unwrap()
            .contains("rId5"));
        assert!(!package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains("slide7.xml"));
    }

    #[test]
    fn add_after_removal_uses_new_part_and_unique_ids() {
        let mut package = OxmlPackage::create("unused.pptx");
        package.add_part("ppt/presentation.xml", PRESENTATION_XML.as_bytes());
        package.add_part(
            "ppt/_rels/presentation.xml.rels",
            PRESENTATION_RELS_XML.as_bytes(),
        );
        package.add_part("[Content_Types].xml", CONTENT_TYPES_XML.as_bytes());
        package.add_part("ppt/slides/slide1.xml", b"<p:sld/>");
        package.add_part("ppt/slides/slide7.xml", b"<p:sld/>");

        remove_slide(&mut package, 1).unwrap();
        assert_eq!(
            crate::navigation::resolve_slide_part_path(&package, 1).unwrap(),
            "ppt/slides/slide7.xml"
        );

        let created = crate::add::add_element(
            &mut package,
            "/presentation",
            "slide",
            InsertPosition::Append,
            &std::collections::HashMap::new(),
        )
        .unwrap();

        assert_eq!(created, "/slide[2]");
        assert!(package.has_part("ppt/slides/slide7.xml"));
        assert!(package.has_part("ppt/slides/slide8.xml"));
        let presentation = package.read_part_xml("ppt/presentation.xml").unwrap();
        assert!(presentation.contains(r#"id="258""#));
        assert!(presentation.contains(r#"r:id="rId6""#));
        let relationships = package
            .read_part_xml("ppt/_rels/presentation.xml.rels")
            .unwrap();
        assert!(relationships.contains(r#"Id="rId6""#));
        assert!(relationships.contains(r#"Target="slides/slide8.xml""#));
        assert!(package
            .read_part_xml("[Content_Types].xml")
            .unwrap()
            .contains(r#"PartName="/ppt/slides/slide8.xml""#));
    }
}
