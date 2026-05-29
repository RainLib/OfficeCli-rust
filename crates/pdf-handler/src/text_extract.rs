use crate::content_stream::PdfColor;
use crate::reader::PdfReader;
use handler_common::{BBoxSpan, StyleSpan, TextOffsetMap};

/// Extract text from PDF with offset→path mapping, including bbox and style info.
pub struct PdfTextExtractor {
    reader: PdfReader,
}

impl PdfTextExtractor {
    pub fn new(reader: PdfReader) -> Self {
        Self { reader }
    }

    pub fn extract_with_offsets(&self) -> TextOffsetMap {
        let mut map = TextOffsetMap::empty("pdf");

        for page_num in 1..=self.reader.page_count() {
            let page_path = format!("/page[{}]", page_num);

            if let Some(parsed) = self.reader.parse_page_text_blocks(page_num) {
                for block in &parsed.text_blocks {
                    let text_path = format!("{}/text[{}]", page_path, block.index);

                    let bbox = Some(BBoxSpan {
                        x: block.bbox.x,
                        y: block.bbox.y,
                        width: block.bbox.width,
                        height: block.bbox.height,
                    });

                    let color_str = block.style.fill_color.as_ref().map(|c| match c {
                        PdfColor::Gray(g) => format!(
                            "rgb({},{},{})",
                            (g * 255.0) as u8,
                            (g * 255.0) as u8,
                            (g * 255.0) as u8
                        ),
                        PdfColor::Rgb(r, g, b) => format!(
                            "rgb({},{},{})",
                            (r * 255.0) as u8,
                            (g * 255.0) as u8,
                            (b * 255.0) as u8
                        ),
                        PdfColor::Cmyk(c, m, y, k) => {
                            let r = (1.0 - c) * (1.0 - k);
                            let g = (1.0 - m) * (1.0 - k);
                            let b = (1.0 - y) * (1.0 - k);
                            format!(
                                "rgb({},{},{})",
                                (r * 255.0) as u8,
                                (g * 255.0) as u8,
                                (b * 255.0) as u8
                            )
                        }
                    });

                    let style = Some(StyleSpan {
                        font: block.style.font_name.clone(),
                        size: block.style.font_size,
                        color: color_str,
                    });

                    map.push_span_with_metadata(&block.text, &text_path, "text-block", bbox, style);
                    map.push_span_with_metadata("\n", &page_path, "line-break", None, None);
                }
            }

            if page_num < self.reader.page_count() {
                map.push_span_with_metadata(
                    "\n\n",
                    &format!("/page[{}]", page_num),
                    "page-break",
                    None,
                    None,
                );
            }
        }

        map.meta.total_chars = map.full_text.len();
        map.meta.total_spans = map.spans.len();
        map
    }

    pub fn extract_text(&self) -> String {
        self.reader.extract_all_text()
    }
}
