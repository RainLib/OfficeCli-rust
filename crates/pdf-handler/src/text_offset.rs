use crate::reader::PdfReader;
use crate::text_extract::PdfTextExtractor;
use handler_common::TextOffsetMap;

/// Generate text+offset→path mapping for a PDF document.
pub fn extract_pdf_text_with_offsets(reader: &PdfReader) -> TextOffsetMap {
    let cloned = PdfReader::open(reader.file_path())
        .unwrap_or_else(|_| PdfReader::fallback(reader.page_count(), reader.file_path()));
    let extractor = PdfTextExtractor::new(cloned);
    extractor.extract_with_offsets()
}
