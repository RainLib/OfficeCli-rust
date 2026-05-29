use handler_common::HandlerError;
use handler_common::InsertPosition;
use oxml::OxmlPackage;

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
    let new_slide_num = pres.slides.len() + 1;
    let new_slide_path = format!("ppt/slides/slide{}.xml", new_slide_num);

    // Write the copied slide content
    package
        .write_part_xml(&new_slide_path, &source_xml)
        .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

    // Update presentation.xml to add the new slide reference
    crate::add::update_presentation_slides(package, new_slide_num)?;

    Ok(format!("/slide[{}]", new_slide_num))
}

/// Reorder the sldIdLst in presentation.xml by moving an entry from source to target position.
fn reorder_sld_id_list(xml: &str, source: usize, target: usize) -> Result<String, HandlerError> {
    // Find all <p:sldId .../> entries
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
    let slide_path = format!("ppt/slides/slide{}.xml", slide_num);

    // Remove the slide part
    if package.has_part(&slide_path) {
        package
            .write_part(&slide_path, Vec::<u8>::new())
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
    }

    Ok(())
}

fn remove_shape(
    package: &mut OxmlPackage,
    slide_num: usize,
    shape_idx: usize,
) -> Result<(), HandlerError> {
    let slide_path = format!("ppt/slides/slide{}.xml", slide_num);

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

fn parse_slide_num(path: &str) -> Result<usize, HandlerError> {
    path.strip_prefix("/slide[")
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|s| s.parse::<usize>().ok())
        .ok_or_else(|| HandlerError::InvalidPath(format!("expected /slide[N], got: {}", path)))
}

fn parse_slide_num_from_full_path(path: &str) -> Result<usize, HandlerError> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .next()
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
