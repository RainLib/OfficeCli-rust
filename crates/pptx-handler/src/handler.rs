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
        crate::view::view_as_stats_json(&self.package.borrow())
    }

    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
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
        crate::view::set_shape_text(&mut self.package.borrow_mut(), path, properties)
    }

    fn add(
        &self,
        parent: &str,
        element_type: &str,
        position: InsertPosition,
        properties: &HashMap<String, String>,
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
