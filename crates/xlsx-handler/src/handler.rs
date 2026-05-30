use handler_common::output_format::{BinaryInfo, RawOptions};
use handler_common::*;
use oxml::OxmlPackage;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::add;
use crate::mutations;
use crate::navigation;
use crate::query;
use crate::raw;
use crate::text_offset;
use crate::view;

pub struct ExcelHandler {
    package: RefCell<OxmlPackage>,
    editable: bool,
}

impl ExcelHandler {
    pub fn open(path: &str, editable: bool) -> Result<Self, HandlerError> {
        let package = OxmlPackage::open(path, editable)
            .map_err(|e| HandlerError::OpenError(e.to_string()))?;
        Ok(Self {
            package: RefCell::new(package),
            editable,
        })
    }
}

impl DocumentHandler for ExcelHandler {
    fn format_name(&self) -> &str {
        "xlsx"
    }

    fn view_as_text(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        let pkg = self.package.borrow();
        view::view_as_text(&pkg, &opts)
    }

    fn view_as_annotated(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        let pkg = self.package.borrow();
        let model =
            crate::helpers::build_workbook_model(&pkg).map_err(HandlerError::OperationFailed)?;

        let mut output = String::new();
        for ws in &model.sheets {
            output.push_str(&format!("=== {} ===\n", ws.name));
            let cell_refs: Vec<&crate::dom_types::Cell> = ws.cells.values().collect();
            let mut sorted = cell_refs;
            sorted.sort_by(|a, b| (a.row, a.col).cmp(&(b.row, b.col)));

            for cell in sorted {
                let type_label = match cell.value_type {
                    crate::dom_types::CellValueType::Number => "num",
                    crate::dom_types::CellValueType::SharedString => "str",
                    crate::dom_types::CellValueType::InlineString => "istr",
                    crate::dom_types::CellValueType::Boolean => "bool",
                    crate::dom_types::CellValueType::Error => "err",
                };
                let style_tag = cell
                    .style_index
                    .map(|si| format!("[s:{}]", si))
                    .unwrap_or_default();
                let formula_tag = cell
                    .formula
                    .as_ref()
                    .map(|f| format!(" [f:{}]", f))
                    .unwrap_or_default();
                output.push_str(&format!(
                    "  {}{}: {}  ({}){}\n",
                    cell.ref_str, style_tag, cell.display_value, type_label, formula_tag,
                ));
            }
            output.push('\n');
        }

        // Apply line range from opts
        if opts.start_line.is_some() || opts.end_line.is_some() {
            let lines: Vec<&str> = output.lines().collect();
            let start = opts.start_line.unwrap_or(1).min(lines.len()).max(1) - 1;
            let end = opts.end_line.unwrap_or(lines.len()).min(lines.len());
            let max = opts.max_lines.unwrap_or(usize::MAX);
            let effective_end = (start + max).min(end);
            return Ok(lines[start..effective_end].join("\n"));
        }

        Ok(output)
    }

    fn view_as_outline(&self) -> Result<String, HandlerError> {
        let pkg = self.package.borrow();
        view::view_as_outline(&pkg)
    }

    fn view_as_stats(&self) -> Result<String, HandlerError> {
        let pkg = self.package.borrow();
        view::view_as_stats(&pkg)
    }

    fn view_as_issues(
        &self,
        issue_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentIssue>, HandlerError> {
        let pkg = self.package.borrow();
        crate::view::view_as_issues(&pkg, issue_type, limit)
    }

    fn view_as_html(&self, _opts: ViewOptions) -> Result<String, HandlerError> {
        let pkg = self.package.borrow();
        crate::html_preview::view_as_html(&pkg)
    }

    fn view_as_text_json(&self, opts: ViewOptions) -> Result<serde_json::Value, HandlerError> {
        let pkg = self.package.borrow();
        view::view_as_text_json(&pkg, &opts)
    }

    fn view_as_outline_json(&self) -> Result<serde_json::Value, HandlerError> {
        let pkg = self.package.borrow();
        view::view_as_outline_json(&pkg)
    }

    fn view_as_stats_json(&self) -> Result<serde_json::Value, HandlerError> {
        let pkg = self.package.borrow();
        view::view_as_stats_json(&pkg)
    }

    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
        let pkg = self.package.borrow();
        navigation::get_node_at_path(&pkg, path, depth)
    }

    fn query(&self, selector: &str) -> Result<Vec<DocumentNode>, HandlerError> {
        let pkg = self.package.borrow();
        query::query_cells(&pkg, selector)
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
        let mut pkg = self.package.borrow_mut();
        if let Some(range_paths_str) = properties.get("range_paths") {
            let segments = handler_common::parse_range_paths(range_paths_str).map_err(|e| {
                HandlerError::InvalidArgument(format!("invalid range paths: {}", e))
            })?;
            mutations::apply_xlsx_range_highlights(&mut pkg, properties, &segments)
        } else {
            mutations::set_cell_properties(&mut pkg, path, properties)
        }
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
        let mut pkg = self.package.borrow_mut();
        add::add_element(&mut pkg, parent, element_type, position, properties)
    }

    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        mutations::remove_element(&mut pkg, path)
    }

    fn move_element(
        &self,
        source: &str,
        target_parent: Option<&str>,
        _position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        mutations::move_cell(&mut pkg, source, target_parent)
    }

    fn copy_from(
        &self,
        source: &str,
        target_parent: &str,
        _position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "package opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        mutations::copy_cell(&mut pkg, source, target_parent)
    }

    fn raw(&self, part_path: &str, _opts: RawOptions) -> Result<String, HandlerError> {
        let pkg = self.package.borrow();
        pkg.read_part_xml(part_path)
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
        let mut pkg = self.package.borrow_mut();
        raw::raw_set(&mut pkg, part_path, xpath, action, xml)
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
        let mut pkg = self.package.borrow_mut();
        crate::raw::add_part(&mut pkg, parent, part_type, properties)
    }

    fn validate(&self) -> Result<Vec<ValidationError>, HandlerError> {
        let pkg = self.package.borrow();
        crate::view::validate(&pkg)
    }

    fn try_extract_binary(
        &self,
        path: &str,
        dest: &str,
    ) -> Result<Option<BinaryInfo>, HandlerError> {
        let pkg = self.package.borrow();
        let content_types = pkg.content_types();

        // Search for media parts (images, charts, etc.)
        let media_path = if path.starts_with("/image") {
            let parts = pkg.list_parts();
            if let Some(idx_str) = path
                .strip_prefix("/image[")
                .and_then(|s| s.strip_suffix(']'))
            {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let image_parts: Vec<&String> = parts
                        .into_iter()
                        .filter(|p| p.starts_with("xl/media/"))
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
        let mut pkg = self.package.borrow_mut();
        pkg.save()
            .map_err(|e| HandlerError::SaveError(e.to_string()))
    }

    fn extract_text_with_offsets(&self) -> Result<TextOffsetMap, HandlerError> {
        let pkg = self.package.borrow();
        text_offset::build_text_offset_map_internal(&pkg)
    }
}
