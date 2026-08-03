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
        if !properties.contains_key("find") && !properties.contains_key("range_paths") {
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
        self.package
            .borrow()
            .read_part_xml(part_path)
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
        crate::raw::apply_raw_set(
            &mut self.package.borrow_mut(),
            part_path,
            xpath,
            action,
            xml,
        )
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
