use handler_common::HandlerError;
use lopdf::Document as LopdfDocument;
use crate::content_stream::{ParsedContentStream, parse_page_content_stream};

/// PDF document reader using lopdf.
pub struct PdfReader {
    doc: LopdfDocument,
    page_count: usize,
    file_path: String,
}

impl PdfReader {
    /// Open a PDF document.
    pub fn open(path: &str) -> Result<Self, HandlerError> {
        let mut doc = LopdfDocument::load(path)
            .map_err(|e| HandlerError::OpenError(format!("failed to open PDF: {}", e)))?;
        let _ = doc.decompress();
        let page_count = Self::count_pages(&doc);
        Ok(Self { doc, page_count, file_path: path.to_string() })
    }

    pub fn page_count(&self) -> usize { self.page_count }
    pub fn document(&self) -> &LopdfDocument { &self.doc }
    pub fn document_mut(&mut self) -> &mut LopdfDocument { &mut self.doc }
    pub fn file_path(&self) -> &str { &self.file_path }

    /// Recount pages from the document (e.g. after deleting a page).
    pub fn recount_pages(&mut self) {
        self.page_count = Self::count_pages(&self.doc);
    }

    /// Create a fallback reader with an empty document (used when re-loading fails).
    pub fn fallback(page_count: usize, file_path: &str) -> Self {
        Self { doc: LopdfDocument::new(), page_count, file_path: file_path.to_string() }
    }

    /// Extract text from all pages.
    pub fn extract_all_text(&self) -> String {
        let mut full_text = String::new();
        for i in 1..=self.page_count {
            if let Some(page_text) = self.extract_page_text(i) {
                if !full_text.is_empty() { full_text.push('\n'); }
                full_text.push_str(&page_text);
            }
        }
        full_text
    }

    /// Extract text from a specific page.
    pub fn extract_page_text(&self, page_num: usize) -> Option<String> {
        let parsed = self.parse_page_text_blocks(page_num)?;
        let mut text = String::new();
        for block in &parsed.text_blocks {
            if !text.is_empty() {
                // Check if this block is on a new line relative to the previous one
                // (different y coordinate indicates a new line)
                text.push('\n');
            }
            text.push_str(&block.text);
        }
        Some(text)
    }

    /// Parse a page's content stream into structured text blocks with bbox info.
    pub fn parse_page_text_blocks(&self, page_num: usize) -> Option<ParsedContentStream> {
        let pages = self.doc.get_pages();
        let page_id = pages.get(&(page_num as u32))?;
        let content = self.doc.get_page_content(*page_id).ok()?;
        parse_page_content_stream(&content, *page_id, &self.doc).ok()
    }

    fn count_pages(doc: &LopdfDocument) -> usize {
        doc.get_pages().len()
    }
}