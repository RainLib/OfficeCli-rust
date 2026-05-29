/// CSS-like selector for document queries.
/// Supports: element type, attribute filters, positional predicates.
/// Example: "p[@style=Normal]", "shape[@id=5]", "cell[bold]"
#[derive(Debug, Clone)]
pub struct Selector {
    /// Element type to match (e.g. "p", "shape", "cell")
    pub element_type: Option<String>,
    /// Attribute filters: key=value pairs
    pub attributes: Vec<(String, String)>,
    /// Style shorthand filters (e.g. "bold" -> font.bold=true)
    pub style_shorthands: Vec<String>,
    /// Positional predicate (e.g. first, last, nth)
    pub position: Option<SelectorPosition>,
}

#[derive(Debug, Clone)]
pub enum SelectorPosition {
    First,
    Last,
    Nth(usize),
}

impl Selector {
    pub fn parse(input: &str) -> Result<Self, SelectorParseError> {
        // Minimal parser for CSS-like selectors
        // Format: type[@attr=val][shorthand][position]
        let input = input.trim();
        if input.is_empty() {
            return Ok(Self {
                element_type: None,
                attributes: Vec::new(),
                style_shorthands: Vec::new(),
                position: None,
            });
        }

        let mut element_type = None;
        let mut attributes = Vec::new();
        let mut style_shorthands = Vec::new();

        // Split on bracket groups
        let mut remaining = input;
        // Extract element type (before first bracket)
        if let Some(bracket_pos) = remaining.find('[') {
            element_type = Some(remaining[..bracket_pos].to_string());
            remaining = &remaining[bracket_pos..];
        } else if !remaining.contains('.') && !remaining.contains(':') {
            element_type = Some(remaining.to_string());
            remaining = "";
        }

        // Parse bracket groups
        while remaining.starts_with('[') {
            if let Some(end) = remaining.find(']') {
                let content = &remaining[1..end];
                remaining = &remaining[end + 1..];
                if content.starts_with('@') {
                    // Attribute selector: [@key=value]
                    let attr_content = &content[1..];
                    if let Some(eq_pos) = attr_content.find('=') {
                        attributes.push((
                            attr_content[..eq_pos].to_string(),
                            attr_content[eq_pos + 1..].to_string(),
                        ));
                    }
                } else {
                    // Style shorthand: [bold], [size=12]
                    if let Some(_eq_pos) = content.find('=') {
                        style_shorthands.push(content.to_string());
                    } else {
                        style_shorthands.push(content.to_string());
                    }
                }
            } else {
                return Err(SelectorParseError::UnmatchedBracket);
            }
        }

        Ok(Self {
            element_type,
            attributes,
            style_shorthands,
            position: None,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SelectorParseError {
    #[error("unmatched bracket in selector")]
    UnmatchedBracket,
}
