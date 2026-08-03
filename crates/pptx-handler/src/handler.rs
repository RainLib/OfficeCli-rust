use handler_common::output_format::{BinaryInfo, RawOptions};
use handler_common::*;
use oxml::OxmlPackage;
use std::cell::RefCell;
use std::collections::HashMap;

pub struct PptxHandler {
    package: RefCell<OxmlPackage>,
    editable: bool,
}

impl PptxHandler {
    pub fn open(path: &str, editable: bool) -> Result<Self, HandlerError> {
        let package = OxmlPackage::open(path, editable)
            .map_err(|e| HandlerError::OpenError(e.to_string()))?;
        Ok(Self {
            package: RefCell::new(package),
            editable,
        })
    }
}

impl DocumentHandler for PptxHandler {
    fn format_name(&self) -> &str {
        "pptx"
    }

    fn view_as_text(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        crate::view::view_as_text(&self.package.borrow(), &opts)
    }

    fn view_as_annotated(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        crate::view::view_as_annotated(&self.package.borrow(), &opts)
    }

    fn view_as_outline(&self) -> Result<String, HandlerError> {
        crate::view::view_as_outline(&self.package.borrow())
    }

    fn view_as_stats(&self) -> Result<String, HandlerError> {
        crate::view::view_as_stats(&self.package.borrow())
    }

    fn view_as_issues(
        &self,
        issue_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentIssue>, HandlerError> {
        crate::view::view_as_issues(&self.package.borrow(), issue_type, limit)
    }

    fn view_as_html(&self, _opts: ViewOptions) -> Result<String, HandlerError> {
        crate::html_preview::view_as_html(&self.package.borrow())
    }

    fn view_as_svg(&self) -> Result<String, HandlerError> {
        crate::svg_preview::view_as_svg(&self.package.borrow())
    }

    fn view_as_text_json(&self, opts: ViewOptions) -> Result<serde_json::Value, HandlerError> {
        crate::view::view_as_text_json(&self.package.borrow(), &opts)
    }

    fn view_as_outline_json(&self) -> Result<serde_json::Value, HandlerError> {
        crate::view::view_as_outline_json(&self.package.borrow())
    }

    fn view_as_stats_json(&self) -> Result<serde_json::Value, HandlerError> {
        let mut stats = crate::view::view_as_stats_json(&self.package.borrow())?;
        // Merge docProps/app.xml extended properties — see docx_handler.
        if let Ok(app_xml) = self.package.borrow().read_part_bytes("docProps/app.xml") {
            let mut node = DocumentNode::new("/", "root");
            handler_common::extended_properties::populate_extended_properties(
                Some(app_xml.as_slice()),
                &mut node,
            );
            if let serde_json::Value::Object(ref mut map) = stats {
                if !node.format.is_empty() {
                    let mut extended = serde_json::Map::new();
                    for (k, v) in node.format.iter() {
                        if let Some(val) = v {
                            extended.insert(k.clone(), val.clone());
                        }
                    }
                    map.insert("extended".into(), serde_json::Value::Object(extended));
                }
            }
        }
        Ok(stats)
    }

    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
        if path.eq_ignore_ascii_case("/theme") {
            return crate::presentation::get_theme(&self.package.borrow());
        }
        if matches!(path, "/presentation") {
            return crate::presentation::get(&self.package.borrow(), depth);
        }
        if path == "/" {
            let mut root = crate::view::get_node(&self.package.borrow(), path, depth)?;
            let settings = crate::presentation::get(&self.package.borrow(), 0)?;
            root.format.extend(settings.format);
            crate::presentation::populate_root_theme(&self.package.borrow(), &mut root)?;
            return Ok(root);
        }
        if path.contains("/br[") || path.contains("/linebreak[") {
            return crate::linebreak::get_linebreak(&self.package.borrow(), path);
        }
        let normalized_path = path.to_ascii_lowercase();
        if normalized_path.contains("/moderncomment[") {
            return crate::add::get_modern_comment_node(&self.package.borrow(), path);
        }
        if normalized_path.contains("/comment[") {
            return crate::add::get_comment_node(&self.package.borrow(), path);
        }
        if path.ends_with("/notes") {
            return crate::add::get_notes_node(&self.package.borrow(), path);
        }
        crate::view::get_node(&self.package.borrow(), path, depth)
    }

    fn query(&self, selector: &str) -> Result<Vec<DocumentNode>, HandlerError> {
        crate::query::query_elements(&self.package.borrow(), selector)
    }

    fn set(
        &self,
        path: &str,
        properties: &HashMap<String, String>,
    ) -> Result<Vec<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        // Find/replace and range edits carry their target in the property map.
        if !properties.contains_key("find")
            && !properties.contains_key("range_paths")
            && !matches!(path, "/" | "" | "/presentation")
            && !path.eq_ignore_ascii_case("/theme")
        {
            handler_common::ensure_scoped(path, "set")?;
        }
        if let Some(range_paths_str) = properties.get("range_paths") {
            let segments = handler_common::parse_range_paths(range_paths_str).map_err(|e| {
                HandlerError::InvalidArgument(format!("invalid range paths: {}", e))
            })?;
            crate::view::apply_pptx_range_highlights(
                &mut self.package.borrow_mut(),
                properties,
                &segments,
            )
        } else if path.eq_ignore_ascii_case("/theme") {
            crate::presentation::set_theme(&mut self.package.borrow_mut(), properties)
        } else if matches!(path, "/" | "" | "/presentation") {
            crate::presentation::set(&mut self.package.borrow_mut(), properties)
        } else if path.to_ascii_lowercase().contains("/moderncomment[") {
            crate::add::set_modern_comment(&mut self.package.borrow_mut(), path, properties)
        } else if path.to_ascii_lowercase().contains("/comment[") {
            crate::add::set_comment(&mut self.package.borrow_mut(), path, properties)
        } else if path.ends_with("/notes") {
            crate::add::set_notes(&mut self.package.borrow_mut(), path, properties)
        } else {
            crate::view::set_shape_text(&mut self.package.borrow_mut(), path, properties)
        }
    }

    fn add(
        &self,
        parent: &str,
        element_type: &str,
        position: InsertPosition,
        properties: &HashMap<String, String>,
        _wrap: Option<&str>,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        crate::add::add_element(
            &mut self.package.borrow_mut(),
            parent,
            element_type,
            position,
            properties,
        )
    }

    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        handler_common::ensure_scoped(path, "remove")?;
        if path.contains("/br[") || path.contains("/linebreak[") {
            return crate::linebreak::remove_linebreak(&mut self.package.borrow_mut(), path);
        }
        let normalized_path = path.to_ascii_lowercase();
        if normalized_path.contains("/moderncomment[") {
            return crate::add::remove_modern_comment(&mut self.package.borrow_mut(), path);
        }
        if normalized_path.contains("/comment[") {
            return crate::add::remove_comment(&mut self.package.borrow_mut(), path);
        }
        if path.ends_with("/notes") {
            return crate::add::remove_notes(&mut self.package.borrow_mut(), path);
        }
        crate::mutations::remove_element(&mut self.package.borrow_mut(), path)
    }

    fn move_element(
        &self,
        source: &str,
        target_parent: Option<&str>,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        crate::mutations::move_slide(&mut pkg, source, target_parent, position)
    }

    fn copy_from(
        &self,
        source: &str,
        target_parent: &str,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        crate::mutations::copy_slide(&mut pkg, source, target_parent, position)
    }

    fn swap(&self, path1: &str, path2: &str) -> Result<(String, String), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        crate::mutations::swap_slides(&mut pkg, path1, path2)
    }

    fn merge(&self, data: &HashMap<String, String>) -> Result<MergeResult, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        let parts = template_merger::pptx_merge_parts(&pkg);
        template_merger::merge_ooxml_parts(&mut pkg, &parts, "a:t", data)
    }

    fn raw(&self, part_path: &str, _opts: RawOptions) -> Result<String, HandlerError> {
        let package = self.package.borrow();
        let resolved = resolve_raw_part_path(&package, part_path)?;
        package
            .read_part_xml(&resolved)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))
    }

    fn raw_set(
        &self,
        part_path: &str,
        xpath: &str,
        action: &str,
        xml: Option<&str>,
    ) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut package = self.package.borrow_mut();
        let resolved = resolve_raw_part_path(&package, part_path)?;
        crate::raw::apply_raw_set(&mut package, &resolved, xpath, action, xml)
    }

    fn add_part(
        &self,
        parent: &str,
        part_type: &str,
        properties: Option<&HashMap<String, String>>,
    ) -> Result<(String, String), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        crate::raw::add_part(
            &mut self.package.borrow_mut(),
            parent,
            part_type,
            properties,
        )
    }

    fn validate(&self) -> Result<Vec<ValidationError>, HandlerError> {
        crate::view::validate(&self.package.borrow())
    }

    fn try_extract_binary(
        &self,
        path: &str,
        dest: &str,
    ) -> Result<Option<BinaryInfo>, HandlerError> {
        let pkg = self.package.borrow();
        let content_types = pkg.content_types();

        // Search for media parts (images, etc.)
        let media_path = if path.starts_with("/image") {
            let parts = pkg.list_parts();
            if let Some(idx_str) = path
                .strip_prefix("/image[")
                .and_then(|s| s.strip_suffix(']'))
            {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let image_parts: Vec<&String> = parts
                        .into_iter()
                        .filter(|p| p.starts_with("ppt/media/"))
                        .collect();
                    if idx > 0 && idx <= image_parts.len() {
                        Some(image_parts[idx - 1].clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else if pkg.has_part(path) {
            Some(path.to_string())
        } else {
            None
        };

        let part_path = media_path.ok_or_else(|| {
            HandlerError::PathNotFound(format!("binary part for path '{}'", path))
        })?;

        let bytes = pkg
            .read_part_bytes(&part_path)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

        std::fs::write(dest, bytes).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to write to '{}': {}", dest, e))
        })?;

        let content_type = content_types
            .content_type_for(&part_path)
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string());

        Ok(Some(BinaryInfo {
            content_type,
            byte_count: bytes.len(),
        }))
    }

    fn save(&self) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "package opened in read-only mode".to_string(),
            ));
        }
        self.package
            .borrow_mut()
            .save()
            .map_err(|e| HandlerError::SaveError(e.to_string()))
    }

    fn extract_text_with_offsets(&self) -> Result<TextOffsetMap, HandlerError> {
        crate::text_offset::extract_text_with_offsets(&self.package.borrow())
    }
}

fn resolve_raw_part_path(package: &OxmlPackage, part_path: &str) -> Result<String, HandlerError> {
    let trimmed = part_path.trim_start_matches('/');
    if trimmed == "presentation" {
        return Ok("ppt/presentation.xml".to_string());
    }
    if trimmed == "theme" {
        return crate::presentation::theme_path(package)?.ok_or_else(|| {
            HandlerError::PathNotFound("theme relationship for presentation".to_string())
        });
    }
    if let Some(index) = trimmed
        .strip_prefix("slide[")
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<usize>().ok())
    {
        return crate::navigation::resolve_slide_part_path(package, index);
    }
    if let Some(index) = semantic_index(trimmed, "slideMaster") {
        return indexed_relationship_target(
            package,
            "ppt/presentation.xml",
            SLIDE_MASTER_REL,
            index,
        );
    }
    if let Some(index) = semantic_index(trimmed, "slideLayout") {
        let masters = relationship_targets(package, "ppt/presentation.xml", SLIDE_MASTER_REL)?;
        let layouts = masters
            .iter()
            .map(|master| relationship_targets(package, master, SLIDE_LAYOUT_REL))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        return layouts
            .into_iter()
            .nth(index - 1)
            .ok_or_else(|| HandlerError::PathNotFound(format!("slideLayout[{}]", index)));
    }
    if let Some(index) = semantic_index(trimmed, "noteSlide") {
        let slide = crate::navigation::resolve_slide_part_path(package, index)?;
        return indexed_relationship_target(package, &slide, NOTES_SLIDE_REL, 1);
    }
    Ok(trimmed.to_string())
}

const SLIDE_MASTER_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
const SLIDE_LAYOUT_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
const NOTES_SLIDE_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";

fn semantic_index(path: &str, kind: &str) -> Option<usize> {
    path.strip_prefix(kind)
        .and_then(|value| value.strip_prefix('['))
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| *index > 0)
}

fn relationship_targets(
    package: &OxmlPackage,
    source_part: &str,
    relationship_type: &str,
) -> Result<Vec<String>, HandlerError> {
    let rels = package
        .part_rels(source_part)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut matches = rels
        .by_type(relationship_type)
        .into_iter()
        .collect::<Vec<_>>();
    matches.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(matches
        .into_iter()
        .map(|rel| package.resolve_rel_target(source_part, &rel.target))
        .collect())
}

fn indexed_relationship_target(
    package: &OxmlPackage,
    source_part: &str,
    relationship_type: &str,
    index: usize,
) -> Result<String, HandlerError> {
    relationship_targets(package, source_part, relationship_type)?
        .into_iter()
        .nth(index - 1)
        .ok_or_else(|| HandlerError::PathNotFound(format!("{}[{}]", relationship_type, index)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_raw_paths_resolve_relationship_targets() {
        let mut package = OxmlPackage::create("semantic-raw.pptx");
        package.add_part(
            "ppt/presentation.xml",
            br#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#,
        );
        package.add_part(
            "ppt/_rels/presentation.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/master7.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide9.xml"/></Relationships>"#,
        );
        package.add_part("ppt/slideMasters/master7.xml", b"<p:sldMaster/>");
        package.add_part(
            "ppt/slideMasters/_rels/master7.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/layout3.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme5.xml"/></Relationships>"#,
        );
        package.add_part("ppt/slideLayouts/layout3.xml", b"<p:sldLayout/>");
        package.add_part("ppt/theme/theme5.xml", b"<a:theme/>");
        package.add_part("ppt/slides/slide9.xml", b"<p:sld/>");
        package.add_part(
            "ppt/slides/_rels/slide9.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide" Target="../notesSlides/notesSlide4.xml"/></Relationships>"#,
        );
        package.add_part("ppt/notesSlides/notesSlide4.xml", b"<p:notes/>");

        assert_eq!(
            resolve_raw_part_path(&package, "/presentation").unwrap(),
            "ppt/presentation.xml"
        );
        assert_eq!(
            resolve_raw_part_path(&package, "/theme").unwrap(),
            "ppt/theme/theme5.xml"
        );
        assert_eq!(
            resolve_raw_part_path(&package, "/slideMaster[1]").unwrap(),
            "ppt/slideMasters/master7.xml"
        );
        assert_eq!(
            resolve_raw_part_path(&package, "/slideLayout[1]").unwrap(),
            "ppt/slideLayouts/layout3.xml"
        );
        assert_eq!(
            resolve_raw_part_path(&package, "/noteSlide[1]").unwrap(),
            "ppt/notesSlides/notesSlide4.xml"
        );
    }
}
