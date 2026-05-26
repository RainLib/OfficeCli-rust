use crate::reader::PdfReader;
use handler_common::{DocumentIssue, IssueSeverity, HandlerError, ValidationError, ViewOptions};

/// PDF view modes implementation.
pub struct PdfViewer {
    reader: PdfReader,
}

impl PdfViewer {
    pub fn new(reader: PdfReader) -> Self { Self { reader } }

    pub fn view_as_text(&self, opts: &ViewOptions) -> Result<String, HandlerError> {
        let full_text = self.reader.extract_all_text();
        let lines: Vec<&str> = full_text.lines().collect();
        let start = opts.start_line.unwrap_or(0);
        let end = opts.end_line.unwrap_or(lines.len());
        let max = opts.max_lines.unwrap_or(lines.len());
        let end = end.min(lines.len()).min(start + max);
        if start >= lines.len() { return Ok(String::new()); }
        Ok(lines[start..end].join("\n"))
    }

    pub fn view_as_annotated(&self, opts: &ViewOptions) -> Result<String, HandlerError> {
        let mut result = String::new();
        let mut block_num = 0;
        let start = opts.start_line.unwrap_or(0);
        let end = opts.end_line.unwrap_or(usize::MAX);
        let max = opts.max_lines.unwrap_or(usize::MAX);

        for page_num in 1..=self.reader.page_count() {
            result.push_str(&format!("=== Page {} ===\n", page_num));
            if let Some(parsed) = self.reader.parse_page_text_blocks(page_num) {
                for block in &parsed.text_blocks {
                    block_num += 1;
                    if block_num < start { continue; }
                    if block_num > end || block_num >= start + max { break; }
                    let bbox = &block.bbox;
                    let style = &block.style;
                    let font_info = style.font_name.as_deref().unwrap_or("-");
                    let size_info = style.font_size.map(|s| format!("{:.0}", s)).unwrap_or("-".to_string());
                    result.push_str(&format!(
                        "  {} | ({:.0},{:.0}) w={:.1} h={:.0} [{} {}] {}\n",
                        block_num, bbox.x, bbox.y, bbox.width, bbox.height, font_info, size_info, block.text
                    ));
                }
            }
        }
        Ok(result)
    }

    pub fn view_as_outline(&self) -> Result<String, HandlerError> {
        let mut result = String::new();
        result.push_str("PDF Document\n");
        result.push_str(&format!("  Pages: {}\n", self.reader.page_count()));
        for page_num in 1..=self.reader.page_count() {
            result.push_str(&format!("  page[{}]:\n", page_num));
            if let Some(parsed) = self.reader.parse_page_text_blocks(page_num) {
                for block in &parsed.text_blocks {
                    let preview = if block.text.chars().count() > 50 {
                        format!("{}...", block.text.chars().take(50).collect::<String>())
                    } else {
                        block.text.clone()
                    };
                    let bbox = &block.bbox;
                    let font = block.style.font_name.as_deref().unwrap_or("-");
                    let size = block.style.font_size.map(|s| format!("{:.0}", s)).unwrap_or("-".to_string());
                    result.push_str(&format!(
                        "    text[{}]: ({:.0},{:.0}) {:.1}×{:.0} [{} {}] \"{}\"\n",
                        block.index, bbox.x, bbox.y, bbox.width, bbox.height, font, size, preview
                    ));
                }
                if parsed.text_blocks.is_empty() {
                    result.push_str("    (no text blocks)\n");
                }
            } else {
                result.push_str("    (empty)\n");
            }
        }
        Ok(result)
    }

    pub fn view_as_stats(&self) -> Result<String, HandlerError> {
        let mut total_chars = 0;
        let mut total_lines = 0;
        for page_num in 1..=self.reader.page_count() {
            if let Some(page_text) = self.reader.extract_page_text(page_num) {
                total_chars += page_text.chars().count();
                total_lines += page_text.lines().count();
            }
        }
        Ok(format!("PDF Statistics\n  Pages: {}\n  Total chars: {}\n  Total lines: {}\n",
            self.reader.page_count(), total_chars, total_lines))
    }

    pub fn view_as_issues(&self, issue_type: Option<&str>, limit: Option<usize>) -> Result<Vec<DocumentIssue>, HandlerError> {
        let mut issues = Vec::new();
        let limit = limit.unwrap_or(50);
        for page_num in 1..=self.reader.page_count() {
            if let Some(page_text) = self.reader.extract_page_text(page_num) {
                if page_text.trim().is_empty() && issues.len() < limit {
                    issues.push(DocumentIssue {
                        severity: IssueSeverity::Info,
                        issue_type: "EmptyPage".to_string(),
                        description: format!("page {} contains no extractable text", page_num),
                        path: Some(format!("/page[{}]", page_num)),
                    });
                }
            }
        }
        if let Some(filter) = issue_type { issues.retain(|i| i.issue_type == filter); }
        Ok(issues)
    }

    /// Validate the PDF document structure.
    pub fn validate(&self) -> Result<Vec<ValidationError>, HandlerError> {
        let mut errors = Vec::new();

        // Check that the document has pages
        if self.reader.page_count() == 0 {
            errors.push(ValidationError {
                error_type: "structure".to_string(),
                description: "PDF has no pages".to_string(),
                path: Some("/".to_string()),
                part: None,
            });
        }

        // Check that each page has content
        for page_num in 1..=self.reader.page_count() {
            let pages = self.reader.document().get_pages();
            if !pages.contains_key(&(page_num as u32)) {
                errors.push(ValidationError {
                    error_type: "structure".to_string(),
                    description: format!("page {} referenced but not found in page tree", page_num),
                    path: Some(format!("/page[{}]", page_num)),
                    part: None,
                });
            }
        }

        Ok(errors)
    }
}