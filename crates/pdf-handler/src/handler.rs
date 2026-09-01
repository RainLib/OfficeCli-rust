#![allow(clippy::redundant_closure, clippy::for_kv_map)]

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

    pub fn ensure_font_for_chars(
        &self,
        page_num: usize,
        characters: &std::collections::HashSet<char>,
        preferred_name: &str,
        font_file: Option<&str>,
    ) -> Result<String, HandlerError> {
        let mut reader = self.reader.borrow_mut();
        crate::font_embedder::ensure_cjk_font_for_chars(
            reader.document_mut(),
            page_num,
            characters,
            Some(preferred_name),
            font_file,
            true,
        )?
        .ok_or_else(|| {
            HandlerError::OperationFailed(
                "font initialization returned no page resource".to_string(),
            )
        })
    }

    pub fn add_ready_text_blocks(
        &self,
        page_num: usize,
        blocks: &[crate::modifier::ReadyTextBlock],
        font_name: &str,
    ) -> Result<(), HandlerError> {
        crate::modifier::add_text_blocks_with_ready_font(
            self.reader.borrow_mut().document_mut(),
            page_num,
            blocks,
            font_name,
        )
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

        // Find/replace: when `find` is present, scan all text and apply replacements.
        if let Some((find, replace, opts)) =
            handler_common::find_replace::extract_find_replace_props(properties)
        {
            let mut reader = self.reader.borrow_mut();
            let total = if path == "/" || path.is_empty() {
                crate::modifier::apply_find_replace_all_pages(
                    reader.document_mut(),
                    &find,
                    &replace,
                    &opts,
                )?
            } else if let Ok(page_num) = PdfNavigator::page_number_from_path(path) {
                crate::modifier::apply_find_replace_on_page(
                    reader.document_mut(),
                    page_num,
                    &find,
                    &replace,
                    &opts,
                )?
            } else {
                0
            };
            return Ok(vec![format!("replaced={}", total)]);
        }

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
                    false,
                );
            }

            if let (Some(text), None, None, None, None, None, None) = (
                text_val,
                font_val,
                size_val,
                color_val.as_ref(),
                char_spacing_val,
                word_spacing_val,
                bg_color_val.as_ref(),
            ) {
                crate::modifier::replace_text_at_path(
                    reader.document_mut(),
                    page_num,
                    text_index,
                    text,
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
            nav.validate_path(path).map_err(HandlerError::InvalidPath)?;
            Some(PdfNavigator::page_number_from_path(path).map_err(HandlerError::InvalidPath)?)
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
        properties: &HashMap<String, String>,
        _wrap: Option<&str>,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }
        match element_type {
            "page" => {
                let (w, h) = parse_page_dimensions(properties);
                let mut reader = self.reader.borrow_mut();
                let total =
                    crate::modifier::add_page_with_size(reader.document_mut(), w as f32, h as f32)?;
                reader.recount_pages();
                Ok(format!("/page[{}]", total))
            }
            "text" | "text-block" => {
                // Add text at (x, y) on a page. Parent should be /page[N].
                let parent = _parent.to_string();
                let page_num = page_num_from_parent(&parent)?;
                let text = properties
                    .get("text")
                    .or_else(|| properties.get("content"))
                    .map(|s| s.as_str())
                    .ok_or_else(|| {
                        HandlerError::InvalidArgument("text property required".to_string())
                    })?;
                let x = properties
                    .get("x")
                    .and_then(|s| s.parse::<f32>().ok())
                    .unwrap_or(72.0);
                let y = properties
                    .get("y")
                    .and_then(|s| s.parse::<f32>().ok())
                    .unwrap_or(720.0);
                let font = properties.get("font").map(|s| s.as_str());
                let size = properties
                    .get("size")
                    .and_then(|s| s.parse::<f32>().ok())
                    .unwrap_or(12.0);
                let mut reader = self.reader.borrow_mut();
                let characters: std::collections::HashSet<char> = properties
                    .get("fontChars")
                    .map(String::as_str)
                    .unwrap_or(text)
                    .chars()
                    .collect();
                let force_embed_font = properties
                    .get("forceEmbedFont")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
                let font_ready = properties
                    .get("fontReady")
                    .is_some_and(|value| value.eq_ignore_ascii_case("true") || value == "1");
                let embedded_font = if font_ready {
                    let font_name = font.ok_or_else(|| {
                        HandlerError::InvalidArgument(
                            "fontReady requires an explicit font resource name".to_string(),
                        )
                    })?;
                    let page_id = *reader
                        .document()
                        .get_pages()
                        .get(&(page_num as u32))
                        .ok_or_else(|| HandlerError::PathNotFound(format!("page {page_num}")))?;
                    let page_fonts =
                        reader.document().get_page_fonts(page_id).map_err(|error| {
                            HandlerError::OperationFailed(format!(
                                "failed to inspect page font resources: {error}"
                            ))
                        })?;
                    if !page_fonts.contains_key(font_name.as_bytes()) {
                        return Err(HandlerError::InvalidArgument(format!(
                            "fontReady resource {font_name} is not registered on page {page_num}"
                        )));
                    }
                    None
                } else {
                    crate::font_embedder::ensure_cjk_font_for_chars(
                        reader.document_mut(),
                        page_num,
                        &characters,
                        font,
                        properties.get("fontFile").map(String::as_str),
                        force_embed_font,
                    )?
                };
                if font_ready {
                    crate::modifier::add_text_block_with_ready_font(
                        reader.document_mut(),
                        page_num,
                        text,
                        x,
                        y,
                        font.expect("fontReady was validated above"),
                        size,
                    )?;
                } else {
                    crate::modifier::add_text_block(
                        reader.document_mut(),
                        page_num,
                        text,
                        x,
                        y,
                        embedded_font.as_deref().or(font),
                        size,
                    )?;
                }
                Ok(format!("/page[{}]/text", page_num))
            }
            "image" | "picture" => {
                let page_num = page_num_from_parent(_parent)?;
                let source = properties
                    .get("src")
                    .or_else(|| properties.get("path"))
                    .or_else(|| properties.get("file"))
                    .ok_or_else(|| {
                        HandlerError::InvalidArgument(
                            "PDF image requires src, path, or file".to_string(),
                        )
                    })?;
                let x = properties
                    .get("x")
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(54.0);
                let y = properties
                    .get("y")
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(540.0);
                let width = properties
                    .get("width")
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(240.0);
                let height = properties
                    .get("height")
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(180.0);
                let mut reader = self.reader.borrow_mut();
                crate::modifier::add_image_block(
                    reader.document_mut(),
                    page_num,
                    std::path::Path::new(source),
                    x,
                    y,
                    width,
                    height,
                )?;
                Ok(format!("/page[{page_num}]/image"))
            }
            other => Err(HandlerError::UnsupportedType(format!(
                "PDF does not support adding {}",
                other
            ))),
        }
    }

    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }

        let nav = PdfNavigator::new(self.reader.borrow().page_count());
        nav.validate_path(path).map_err(HandlerError::InvalidPath)?;

        let page_num =
            PdfNavigator::page_number_from_path(path).map_err(HandlerError::InvalidPath)?;

        let mut reader = self.reader.borrow_mut();
        crate::modifier::delete_page(reader.document_mut(), page_num)?;
        reader.recount_pages();

        Ok(Some(format!("removed page {}", page_num)))
    }

    fn move_element(
        &self,
        source: &str,
        _target_parent: Option<&str>,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }
        let from =
            PdfNavigator::page_number_from_path(source).map_err(HandlerError::InvalidPath)?;
        // For PDF, the destination is conveyed as a numeric position or as
        // `after:/page[N]` / `before:/page[N]`.
        let to = pdf_position_to_target_index(position, _target_parent)?;
        let mut reader = self.reader.borrow_mut();
        let moved_to = crate::modifier::move_page(reader.document_mut(), from, to)?;
        reader.recount_pages();
        Ok(format!("/page[{}]", moved_to))
    }

    fn copy_from(
        &self,
        source: &str,
        _target_parent: &str,
        _position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "PDF opened in read-only mode".to_string(),
            ));
        }
        let src_page =
            PdfNavigator::page_number_from_path(source).map_err(HandlerError::InvalidPath)?;

        // Open the source as a separate lopdf Document. We need a fresh load of
        // the file because the in-memory handler state can't serve as both
        // source and target for lopdf::copy_page_from.
        let src_path = self.reader.borrow().file_path().to_string();
        let source_doc = lopdf::Document::load(&src_path)
            .map_err(|e| HandlerError::OperationFailed(format!("reload source for copy: {}", e)))?;

        let mut reader = self.reader.borrow_mut();
        let new_page =
            crate::modifier::copy_page_from(reader.document_mut(), &source_doc, src_page)?;
        reader.recount_pages();
        Ok(format!("/page[{}]", new_page))
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
            nav.validate_path(path).map_err(HandlerError::InvalidPath)?;
            PdfNavigator::page_number_from_path(path).map_err(HandlerError::InvalidPath)?
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
    let hex = s.strip_prefix('#').unwrap_or(s);
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

/// Parse page dimensions from add-page properties. Accepts width/height as
/// points (PDF user units). Defaults to US Letter (612x792).
fn parse_page_dimensions(props: &HashMap<String, String>) -> (i64, i64) {
    let w = props
        .get("width")
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| props.get("page-width").and_then(|s| s.parse::<f64>().ok()))
        .unwrap_or(612.0);
    let h = props
        .get("height")
        .and_then(|s| s.parse::<f64>().ok())
        .or_else(|| props.get("page-height").and_then(|s| s.parse::<f64>().ok()))
        .unwrap_or(792.0);

    // Accept size=letter | a4 | legal.
    if let Some(preset) = props.get("size").map(|s| s.as_str()) {
        let (pw, ph) = match preset.to_ascii_lowercase().as_str() {
            "letter" => (612.0, 792.0),
            "legal" => (612.0, 1008.0),
            "a4" => (595.28, 841.89),
            "a3" => (841.89, 1190.55),
            _ => (w, h),
        };
        return (pw.round() as i64, ph.round() as i64);
    }
    (w.round() as i64, h.round() as i64)
}

/// Resolve a parent path like `/page[N]` to a 1-based page number.
fn page_num_from_parent(parent: &str) -> Result<usize, HandlerError> {
    PdfNavigator::page_number_from_path(parent).map_err(HandlerError::InvalidPath)
}

/// Translate an InsertPosition + optional target_parent into a 1-based target
/// page index for `modifier::move_page`.
fn pdf_position_to_target_index(
    position: InsertPosition,
    _target_parent: Option<&str>,
) -> Result<usize, HandlerError> {
    match position {
        InsertPosition::AtIndex(idx) => Ok(idx + 1),
        InsertPosition::AfterElement(anchor) => {
            let n =
                PdfNavigator::page_number_from_path(&anchor).map_err(HandlerError::InvalidPath)?;
            Ok(n + 1)
        }
        InsertPosition::BeforeElement(anchor) => {
            let n =
                PdfNavigator::page_number_from_path(&anchor).map_err(HandlerError::InvalidPath)?;
            Ok(n)
        }
        InsertPosition::Append => Err(HandlerError::InvalidArgument(
            "PDF move requires --position (index or after:/before:)".to_string(),
        )),
    }
}
