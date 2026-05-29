use crate::content_stream::PdfColor;
use crate::navigation::PdfNavigator;
use crate::reader::PdfReader;
use crate::text_extract::PdfTextExtractor;
use crate::view::PdfViewer;
use handler_common::output_format::BinaryInfo;
use handler_common::*;
use std::cell::RefCell;
use std::collections::HashMap;

/// PDF document handler implementing DocumentHandler trait.
pub struct PdfHandler {
    reader: RefCell<PdfReader>,
    editable: bool,
}

impl PdfHandler {
    /// Open a PDF document.
    pub fn open(path: &str, editable: bool) -> Result<Self, HandlerError> {
        let reader = PdfReader::open(path)?;
        Ok(Self {
            reader: RefCell::new(reader),
            editable,
        })
    }
}

impl DocumentHandler for PdfHandler {
    fn format_name(&self) -> &str {
        "pdf"
    }

    fn view_as_text(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        let reader = self.reader.borrow();
        PdfViewer::new(PdfReader::open(reader.file_path())?).view_as_text(&opts)
    }

    fn view_as_annotated(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        let reader = self.reader.borrow();
        PdfViewer::new(PdfReader::open(reader.file_path())?).view_as_annotated(&opts)
    }

    fn view_as_outline(&self) -> Result<String, HandlerError> {
        let reader = self.reader.borrow();
        PdfViewer::new(PdfReader::open(reader.file_path())?).view_as_outline()
    }

    fn view_as_stats(&self) -> Result<String, HandlerError> {
        let reader = self.reader.borrow();
        PdfViewer::new(PdfReader::open(reader.file_path())?).view_as_stats()
    }

    fn view_as_issues(
        &self,
        issue_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentIssue>, HandlerError> {
        let reader = self.reader.borrow();
        PdfViewer::new(PdfReader::open(reader.file_path())?).view_as_issues(issue_type, limit)
    }

    fn view_as_html(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        crate::html_preview::view_as_html(&self.reader.borrow(), opts)
    }

    fn view_as_svg(&self) -> Result<String, HandlerError> {
        let reader = self.reader.borrow();
        crate::render::PdfRenderer::render_page_to_svg(reader.file_path(), 1)
    }

    fn view_as_text_json(&self, opts: ViewOptions) -> Result<serde_json::Value, HandlerError> {
        let text = self.view_as_text(opts)?;
        Ok(serde_json::json!({
            "format": "pdf",
            "text": text,
            "pageCount": self.reader.borrow().page_count()
        }))
    }

    fn view_as_outline_json(&self) -> Result<serde_json::Value, HandlerError> {
        let outline = self.view_as_outline()?;
        Ok(serde_json::json!({
            "format": "pdf",
            "outline": outline,
            "pageCount": self.reader.borrow().page_count()
        }))
    }

    fn view_as_stats_json(&self) -> Result<serde_json::Value, HandlerError> {
        let stats = self.view_as_stats()?;
        Ok(serde_json::json!({
            "format": "pdf",
            "stats": stats,
            "pageCount": self.reader.borrow().page_count()
        }))
    }

    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
        let reader = self.reader.borrow();

        if path == "/" {
            let mut root_node = DocumentNode::new("/", "pdf-document");
            if depth > 0 {
                let page_count = reader.page_count();
                let mut children = Vec::new();
                for i in 1..=page_count {
                    let page_text = reader.extract_page_text(i).unwrap_or_default();
                    let preview = if page_text.chars().count() > 80 {
                        format!("{}...", page_text.chars().take(80).collect::<String>())
                    } else {
                        page_text.clone()
                    };
                    let mut page_node = DocumentNode::new(&format!("/page[{}]", i), "page")
                        .with_text(&page_text)
                        .with_preview(&preview);

                    if depth > 1 {
                        if let Some(parsed) = reader.parse_page_text_blocks(i) {
                            let mut page_children = Vec::new();
                            for block in &parsed.text_blocks {
                                let block_path = format!("/page[{}]/text[{}]", i, block.index);
                                let mut block_node = DocumentNode::new(&block_path, "text-block")
                                    .with_text(&block.text)
                                    .with_format("bbox_x", serde_json::json!(block.bbox.x))
                                    .with_format("bbox_y", serde_json::json!(block.bbox.y))
                                    .with_format("bbox_width", serde_json::json!(block.bbox.width))
                                    .with_format(
                                        "bbox_height",
                                        serde_json::json!(block.bbox.height),
                                    );

                                if let Some(ref font) = block.style.font_name {
                                    block_node =
                                        block_node.with_format("font", serde_json::json!(font));
                                }
                                if let Some(size) = block.style.font_size {
                                    block_node = block_node
                                        .with_format("font_size", serde_json::json!(size));
                                }
                                page_children.push(block_node);
                            }
                            page_node = page_node.with_children(page_children);
                        }
                    }
                    children.push(page_node);
                }
                root_node = root_node.with_children(children);
            }
            return Ok(root_node);
        }

        // Check if path targets a specific text block: /page[N]/text[M]
        let text_path = parse_text_block_path(path);
        if let Some((page_num, text_index)) = text_path {
            let parsed = reader
                .parse_page_text_blocks(page_num)
                .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

            let block_idx = text_index - 1;
            if block_idx >= parsed.text_blocks.len() {
                return Err(HandlerError::PathNotFound(format!(
                    "text[{}] not found (page {} has {} text blocks)",
                    text_index,
                    page_num,
                    parsed.text_blocks.len()
                )));
            }

            let block = &parsed.text_blocks[block_idx];
            let mut node = DocumentNode::new(path, "text-block")
                .with_text(&block.text)
                .with_format("bbox_x", serde_json::json!(block.bbox.x))
                .with_format("bbox_y", serde_json::json!(block.bbox.y))
                .with_format("bbox_width", serde_json::json!(block.bbox.width))
                .with_format("bbox_height", serde_json::json!(block.bbox.height));

            if let Some(ref font) = block.style.font_name {
                node = node.with_format("font", serde_json::json!(font));
            }
            if let Some(size) = block.style.font_size {
                node = node.with_format("font_size", serde_json::json!(size));
            }
            if let Some(ref color) = block.style.fill_color {
                node = node.with_format("color", serde_json::json!(format_pdf_color(color)));
            }
            if let Some(ref bg) = block.style.bg_color {
                node = node.with_format("bgColor", serde_json::json!(format_pdf_color(bg)));
            }

            return Ok(node);
        }

        let nav = PdfNavigator::new(reader.page_count());
        nav.validate_path(path)
            .map_err(|e| HandlerError::InvalidPath(e))?;

        let page_num =
            PdfNavigator::page_number_from_path(path).map_err(|e| HandlerError::InvalidPath(e))?;

        let node = DocumentNode::new(path, "page")
            .with_text(reader.extract_page_text(page_num).unwrap_or_default());
        Ok(node)
    }

    fn query(&self, selector: &str) -> Result<Vec<DocumentNode>, HandlerError> {
        let parsed =
            Selector::parse(selector).map_err(|e| HandlerError::InvalidArgument(e.to_string()))?;
        let reader = self.reader.borrow();
        let mut results = Vec::new();

        if let Some(element_type) = &parsed.element_type {
            if element_type == "page" {
                for i in 1..=reader.page_count() {
                    let path = format!("/page[{}]", i);
                    let node = DocumentNode::new(&path, "page")
                        .with_text(reader.extract_page_text(i).unwrap_or_default());
                    results.push(node);
                }
            } else if element_type == "text" || element_type == "text-block" {
                // Return individual text blocks with bbox and style
                for page_num in 1..=reader.page_count() {
                    if let Some(parsed_stream) = reader.parse_page_text_blocks(page_num) {
                        for block in &parsed_stream.text_blocks {
                            let path = format!("/page[{}]/text[{}]", page_num, block.index);
                            let mut node = DocumentNode::new(&path, "text-block")
                                .with_text(&block.text)
                                .with_format("bbox_x", serde_json::json!(block.bbox.x))
                                .with_format("bbox_y", serde_json::json!(block.bbox.y))
                                .with_format("bbox_width", serde_json::json!(block.bbox.width))
                                .with_format("bbox_height", serde_json::json!(block.bbox.height));

                            if let Some(ref font) = block.style.font_name {
                                node = node.with_format("font", serde_json::json!(font));
                            }
                            if let Some(size) = block.style.font_size {
                                node = node.with_format("font_size", serde_json::json!(size));
                            }
                            if let Some(ref color) = block.style.fill_color {
                                node = node.with_format(
                                    "color",
                                    serde_json::json!(format_pdf_color(color)),
                                );
                            }
                            if let Some(ref bg) = block.style.bg_color {
                                node = node.with_format(
                                    "bgColor",
                                    serde_json::json!(format_pdf_color(bg)),
                                );
                            }
                            results.push(node);
                        }
                    }
                }
            }
        }
        Ok(results)
    }

    fn set(
        &self,
        path: &str,
        properties: &HashMap<String, String>,
    ) -> Result<Vec<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }

        let mut unsupported = Vec::new();

        // Check if global range paths highlit is requested
        if let Some(range_paths_str) = properties.get("range_paths") {
            let segments = handler_common::parse_range_paths(range_paths_str).map_err(|e| {
                HandlerError::InvalidArgument(format!("invalid range paths: {}", e))
            })?;

            let mut reader = self.reader.borrow_mut();

            if properties.contains_key("color") {
                if let Some(color_str) = properties.get("color") {
                    if let Some(color) = parse_color(color_str) {
                        crate::modifier::apply_range_text_colors(
                            reader.document_mut(),
                            &color,
                            &segments,
                        )?;
                    }
                }
            }

            if properties.contains_key("bgColor") || !properties.contains_key("color") {
                let bg_color = properties
                    .get("bgColor")
                    .and_then(|s| parse_color(s))
                    .unwrap_or(PdfColor::Rgb(1.0, 1.0, 0.0)); // default yellow
                crate::modifier::apply_range_highlights(
                    reader.document_mut(),
                    &bg_color,
                    &segments,
                )?;
            }

            for (key, _) in properties {
                if !matches!(key.as_str(), "range_paths" | "bgColor" | "color") {
                    unsupported.push(key.clone());
                }
            }
            return Ok(unsupported);
        }

        // Check if path targets a specific text block: /page[N]/text[M]
        let text_path = parse_text_block_path(path);
        if let Some((page_num, text_index)) = text_path {
            let text_val = properties.get("text").map(|s| s.as_str());
            let font_val = properties.get("font").map(|s| s.as_str());
            let size_val = properties.get("size").and_then(|s| s.parse::<f32>().ok());
            let color_val = properties.get("color").and_then(|s| parse_color(s));
            let char_spacing_val = properties
                .get("charSpacing")
                .and_then(|s| s.parse::<f32>().ok());
            let word_spacing_val = properties
                .get("wordSpacing")
                .and_then(|s| s.parse::<f32>().ok());
            let bg_color_val = properties.get("bgColor").and_then(|s| parse_color(s));
            let font_file_val = properties.get("fontFile").map(|s| s.as_str());

            let mut reader = self.reader.borrow_mut();

            // Before the actual text edit, give the embedder a chance to add a
            // fallback font for any character the existing page fonts can't render.
            // The embedder itself scans page fonts and is a no-op when every char
            // is already supported, so this is safe to call unconditionally.
            // Skipping this check based on ASCII/CJK heuristics is wrong: PowerPoint
            // exports use subsetted fonts that may omit even ASCII glyphs like '*'.
            if let Some(text_str) = text_val {
                let chars_needed: std::collections::HashSet<char> = text_str.chars().collect();
                let _ = crate::font_embedder::ensure_cjk_font_for_chars(
                    reader.document_mut(),
                    page_num,
                    &chars_needed,
                    font_val,
                    font_file_val,
                );
            }

            if text_val.is_some()
                && font_val.is_none()
                && size_val.is_none()
                && color_val.is_none()
                && char_spacing_val.is_none()
                && word_spacing_val.is_none()
                && bg_color_val.is_none()
            {
                crate::modifier::replace_text_at_path(
                    reader.document_mut(),
                    page_num,
                    text_index,
                    text_val.unwrap(),
                    font_val,
                )?;
            } else {
                crate::modifier::replace_text_with_style(
                    reader.document_mut(),
                    page_num,
                    text_index,
                    text_val,
                    font_val,
                    size_val,
                    color_val.as_ref(),
                    char_spacing_val,
                    word_spacing_val,
                    bg_color_val.as_ref(),
                )?;
            }

            for (key, _) in properties {
                if !matches!(
                    key.as_str(),
                    "text"
                        | "content"
                        | "font"
                        | "size"
                        | "color"
                        | "charSpacing"
                        | "wordSpacing"
                        | "bgColor"
                        | "fontFile"
                ) {
                    unsupported.push(key.clone());
                }
            }

            return Ok(unsupported);
        }

        // Page-level path: /page[N]
        let page_num = if path == "/" {
            None
        } else {
            let nav = PdfNavigator::new(self.reader.borrow().page_count());
            nav.validate_path(path)
                .map_err(|e| HandlerError::InvalidPath(e))?;
            Some(
                PdfNavigator::page_number_from_path(path)
                    .map_err(|e| HandlerError::InvalidPath(e))?,
            )
        };

        for (key, value) in properties {
            match key.as_str() {
                "text" | "content" => {
                    let mut reader = self.reader.borrow_mut();
                    if let Some(page) = page_num {
                        crate::modifier::replace_text_on_page(reader.document_mut(), page, value)?;
                    } else {
                        let page_count = reader.page_count();
                        for page in 1..=page_count {
                            crate::modifier::replace_text_on_page(
                                reader.document_mut(),
                                page,
                                value,
                            )
                            .ok();
                        }
                    }
                }
                other => unsupported.push(other.to_string()),
            }
        }

        Ok(unsupported)
    }

    fn add(
        &self,
        _parent: &str,
        element_type: &str,
        _position: InsertPosition,
        _properties: &HashMap<String, String>,
    ) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedType(format!(
            "PDF does not support adding {}",
            element_type
        )))
    }

    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }

        let nav = PdfNavigator::new(self.reader.borrow().page_count());
        nav.validate_path(path)
            .map_err(|e| HandlerError::InvalidPath(e))?;

        let page_num =
            PdfNavigator::page_number_from_path(path).map_err(|e| HandlerError::InvalidPath(e))?;

        let mut reader = self.reader.borrow_mut();
        crate::modifier::delete_page(reader.document_mut(), page_num)?;
        reader.recount_pages();

        Ok(Some(format!("removed page {}", page_num)))
    }

    fn move_element(
        &self,
        _source: &str,
        _target_parent: Option<&str>,
        _position: InsertPosition,
    ) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode(
            "PDF does not support moving elements".to_string(),
        ))
    }

    fn copy_from(
        &self,
        _source: &str,
        _target_parent: &str,
        _position: InsertPosition,
    ) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode(
            "PDF does not support copying elements".to_string(),
        ))
    }

    fn raw(&self, part_path: &str, _opts: RawOptions) -> Result<String, HandlerError> {
        let reader = self.reader.borrow();
        let page_num = part_path
            .strip_prefix("/page[")
            .and_then(|s| s.strip_suffix("]"))
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| HandlerError::InvalidPath(part_path.to_string()))?;

        let pages = reader.document().get_pages();
        let page_id = pages
            .get(&(page_num as u32))
            .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

        reader
            .document()
            .get_page_content(*page_id)
            .map(|content| String::from_utf8_lossy(&content).to_string())
            .map_err(|e| {
                HandlerError::OperationFailed(format!("failed to get page content: {}", e))
            })
    }

    fn raw_set(
        &self,
        part_path: &str,
        _xpath: &str,
        action: &str,
        content: Option<&str>,
    ) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }

        let page_num = part_path
            .strip_prefix("/page[")
            .and_then(|s| s.strip_suffix("]"))
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| HandlerError::InvalidPath(part_path.to_string()))?;

        let mut reader = self.reader.borrow_mut();
        let pages = reader.document().get_pages();
        let page_id = pages
            .get(&(page_num as u32))
            .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

        match action {
            "replace_content" => {
                let new_content = content.ok_or_else(|| {
                    HandlerError::InvalidArgument(
                        "content required for replace_content".to_string(),
                    )
                })?;
                let new_bytes = new_content.as_bytes();
                crate::modifier::replace_page_content(reader.document_mut(), *page_id, new_bytes)?;
                Ok(())
            }
            _ => Err(HandlerError::UnsupportedMode(format!(
                "PDF raw_set action '{}' not supported",
                action
            ))),
        }
    }

    fn add_part(
        &self,
        _parent: &str,
        _part_type: &str,
        _properties: Option<&HashMap<String, String>>,
    ) -> Result<(String, String), HandlerError> {
        Err(HandlerError::UnsupportedMode(
            "PDF does not support adding parts".to_string(),
        ))
    }

    fn validate(&self) -> Result<Vec<ValidationError>, HandlerError> {
        let reader = self.reader.borrow();
        let viewer = PdfViewer::new(PdfReader::open(reader.file_path())?);
        viewer.validate()
    }

    fn try_extract_binary(
        &self,
        path: &str,
        dest: &str,
    ) -> Result<Option<BinaryInfo>, HandlerError> {
        // PDF binary extraction: extract embedded images from a page
        let page_num = if path.starts_with("/page[") {
            let nav = PdfNavigator::new(self.reader.borrow().page_count());
            nav.validate_path(path)
                .map_err(|e| HandlerError::InvalidPath(e))?;
            PdfNavigator::page_number_from_path(path).map_err(|e| HandlerError::InvalidPath(e))?
        } else {
            return Err(HandlerError::InvalidPath(path.to_string()));
        };

        let reader = self.reader.borrow();
        let pages = reader.document().get_pages();
        let page_id = pages
            .get(&(page_num as u32))
            .ok_or_else(|| HandlerError::PathNotFound(format!("page {}", page_num)))?;

        let doc = reader.document();

        // Look for image streams in the document objects associated with this page
        let content_ids = doc.get_page_contents(*page_id);
        for content_id in content_ids {
            if let Ok(lopdf::Object::Stream(stream)) = doc.get_object(content_id) {
                // Check if this is an image stream
                if let Ok(subtype_obj) = stream.dict.get(b"Subtype") {
                    if let Ok(name) = subtype_obj.as_name_str() {
                        if name == "Image" {
                            std::fs::write(dest, &stream.content)
                                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
                            return Ok(Some(BinaryInfo {
                                content_type: "image/raw".to_string(),
                                byte_count: stream.content.len(),
                            }));
                        }
                    }
                }
            }
        }

        // Search all objects for image streams referenced by this page
        for (_, obj) in doc.objects.iter() {
            if let lopdf::Object::Stream(stream) = obj {
                if let Ok(subtype_obj) = stream.dict.get(b"Subtype") {
                    if let Ok(name) = subtype_obj.as_name_str() {
                        if name == "Image" {
                            std::fs::write(dest, &stream.content)
                                .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
                            return Ok(Some(BinaryInfo {
                                content_type: "image/raw".to_string(),
                                byte_count: stream.content.len(),
                            }));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    fn save(&self) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }

        let file_path = self.reader.borrow().file_path().to_string();
        let mut reader = self.reader.borrow_mut();
        // Remove "Prev" key from the trailer dictionary. Since lopdf saves
        // the PDF as a single flattened document, keeping a legacy "Prev"
        // key from incremental updates will point to invalid offsets and
        // corrupt the file trailer for subsequent loads.
        reader.document_mut().trailer.remove(b"Prev");
        reader
            .document_mut()
            .save(&file_path)
            .map_err(|e| HandlerError::SaveError(format!("failed to save PDF: {}", e)))?;
        Ok(())
    }

    fn extract_text_with_offsets(&self) -> Result<TextOffsetMap, HandlerError> {
        let reader = self.reader.borrow();
        let extractor = PdfTextExtractor::new(PdfReader::open(reader.file_path())?);
        Ok(extractor.extract_with_offsets())
    }
}

/// Parse a text block path like /page[1]/text[3] into (page_num, text_index).
/// Returns None if the path doesn't contain a text[N] segment.
fn parse_text_block_path(path: &str) -> Option<(usize, usize)> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() != 2 {
        return None;
    }

    // Parse page[N]
    let page_part = parts[0];
    if !page_part.starts_with("page") {
        return None;
    }
    let page_num = page_part
        .strip_prefix("page[")
        .and_then(|s| s.strip_suffix("]"))
        .and_then(|s| s.parse::<usize>().ok())?;

    // Parse text[M]
    let text_part = parts[1];
    if !text_part.starts_with("text") {
        return None;
    }
    let text_index = text_part
        .strip_prefix("text[")
        .and_then(|s| s.strip_suffix("]"))
        .and_then(|s| s.parse::<usize>().ok())?;

    Some((page_num, text_index))
}

/// Parse a color string into a PdfColor.
/// Supports: "FF0000", "#FF0000", "rgb(255,0,0)", "1.0 0.0 0.0 rg"
fn parse_color(s: &str) -> Option<PdfColor> {
    let s = s.trim();

    // Hex format: FF0000 or #FF0000
    let hex = if s.starts_with('#') { &s[1..] } else { s };
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()? as f32 / 255.0;
        return Some(PdfColor::Rgb(r, g, b));
    }

    // rgb(r,g,b) format
    if s.starts_with("rgb(") && s.ends_with(')') {
        let inner = &s[4..s.len() - 1];
        let parts: Vec<f32> = inner
            .split(',')
            .filter_map(|p| p.trim().parse::<f32>().ok())
            .collect();
        if parts.len() == 3 {
            return Some(PdfColor::Rgb(
                parts[0] / 255.0,
                parts[1] / 255.0,
                parts[2] / 255.0,
            ));
        }
    }

    None
}

/// Format a PdfColor as a hex color string (e.g. #FF0000).
fn format_pdf_color(color: &PdfColor) -> String {
    match color {
        PdfColor::Gray(g) => {
            let val = (g * 255.0).round() as u8;
            format!("#{:02X}{:02X}{:02X}", val, val, val)
        }
        PdfColor::Rgb(r, g, b) => {
            let rv = (r * 255.0).round() as u8;
            let gv = (g * 255.0).round() as u8;
            let bv = (b * 255.0).round() as u8;
            format!("#{:02X}{:02X}{:02X}", rv, gv, bv)
        }
        PdfColor::Cmyk(c, m, y, k) => {
            let r = (((1.0 - c) * (1.0 - k)) * 255.0).round() as u8;
            let g = (((1.0 - m) * (1.0 - k)) * 255.0).round() as u8;
            let b = (((1.0 - y) * (1.0 - k)) * 255.0).round() as u8;
            format!("#{:02X}{:02X}{:02X}", r, g, b)
        }
    }
}
