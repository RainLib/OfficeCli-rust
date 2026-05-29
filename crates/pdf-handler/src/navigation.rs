use handler_common::PathSegment;

/// PDF path system:
/// /page[1]                  — 1st page
/// /page[1]/text[1]          — 1st text object on page 1
/// /page[1]/image[1]         — 1st image on page 1
/// /page[1]/annotation[1]    — 1st annotation on page 1
/// /page[1]/link[1]          — 1st hyperlink on page 1
pub struct PdfNavigator {
    page_count: usize,
}

impl PdfNavigator {
    pub fn new(page_count: usize) -> Self {
        Self { page_count }
    }

    /// Parse a PDF path string into segments.
    pub fn parse_path(path: &str) -> Result<Vec<PathSegment>, String> {
        if !path.starts_with('/') {
            return Err(format!("PDF path must start with /: {}", path));
        }

        let segments = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| parse_pdf_path_segment(s))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(segments)
    }

    /// Validate that a path is within the document's page range.
    pub fn validate_path(&self, path: &str) -> Result<(), String> {
        let segments = Self::parse_path(path)?;

        if let Some(first) = segments.first() {
            if first.name == "page" {
                if let Some(idx) = first.index {
                    if idx > self.page_count || idx == 0 {
                        return Err(format!(
                            "page index {} out of range (1-{})",
                            idx, self.page_count
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Get page number from a path like /page[3]/text[1].
    pub fn page_number_from_path(path: &str) -> Result<usize, String> {
        let segments = Self::parse_path(path)?;
        if let Some(first) = segments.first() {
            if first.name == "page" {
                return Ok(first.index.unwrap_or(1));
            }
        }
        Err("path must start with /page[N]".to_string())
    }
}

fn parse_pdf_path_segment(s: &str) -> Result<PathSegment, String> {
    let mut name = s.to_string();
    let mut index = None;

    if let Some(bracket_start) = s.find('[') {
        name = s[..bracket_start].to_string();
        let bracket_content = &s[bracket_start + 1..];
        if let Some(bracket_end) = bracket_content.find(']') {
            let idx_str = &bracket_content[..bracket_end];
            if let Ok(idx) = idx_str.parse::<usize>() {
                index = Some(idx);
            }
        }
    }

    Ok(PathSegment::new(&name).with_index(index.unwrap_or(1)))
}
