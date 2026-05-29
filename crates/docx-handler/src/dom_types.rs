use std::collections::HashMap;

/// Word XML element type variants (local names from the w: namespace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordElementType {
    Document,
    Body,
    Paragraph,           // w:p
    Run,                 // w:r
    Text,                // w:t
    Break,               // w:br
    Tab,                 // w:tab
    ParagraphMark,       // w:pPr placeholder
    RunProperties,       // w:rPr
    ParagraphProperties, // w:pPr
    Table,               // w:tbl
    TableRow,            // w:tr
    TableCell,           // w:tc
    TableProperties,     // w:tblPr
    TableRowProperties,  // w:trPr
    TableCellProperties, // w:tcPr
    Hyperlink,           // w:hyperlink
    BookmarkStart,       // w:bookmarkStart
    BookmarkEnd,         // w:bookmarkEnd
    FieldSimple,         // w:fldSimple
    Drawing,             // w:drawing
    InlineImage,         // wp:inline
    SectionProperties,   // w:sectPr
    FootnoteReference,   // w:footnoteReference
    EndnoteReference,    // w:endnoteReference
    CommentReference,    // w:commentReference
    MoveFrom,            // w:moveFrom
    MoveTo,              // w:moveTo
    MoveFromRangeStart,  // w:moveFromRangeStart
    MoveFromRangeEnd,    // w:moveFromRangeEnd
    MoveToRangeStart,    // w:moveToRangeStart
    MoveToRangeEnd,      // w:moveToRangeEnd
    CustomXml,           // w:customXml
    Sdt,                 // w:sdt
    SdtContent,          // w:sdtContent
    SdtPr,               // w:sdtPr
    ProofErr,            // w:proofErr
    AnnotationRef,       // w:annotationRef
    Unknown(String),
}

impl WordElementType {
    /// Convert from the w: namespace local name to a WordElementType.
    pub fn from_local_name(name: &str) -> Self {
        match name {
            "document" => Self::Document,
            "body" => Self::Body,
            "p" => Self::Paragraph,
            "r" => Self::Run,
            "t" => Self::Text,
            "br" => Self::Break,
            "tab" => Self::Tab,
            "pPr" => Self::ParagraphProperties,
            "rPr" => Self::RunProperties,
            "tbl" => Self::Table,
            "tr" => Self::TableRow,
            "tc" => Self::TableCell,
            "tblPr" => Self::TableProperties,
            "trPr" => Self::TableRowProperties,
            "tcPr" => Self::TableCellProperties,
            "hyperlink" => Self::Hyperlink,
            "bookmarkStart" => Self::BookmarkStart,
            "bookmarkEnd" => Self::BookmarkEnd,
            "fldSimple" => Self::FieldSimple,
            "drawing" => Self::Drawing,
            "inline" => Self::InlineImage,
            "sectPr" => Self::SectionProperties,
            "footnoteReference" => Self::FootnoteReference,
            "endnoteReference" => Self::EndnoteReference,
            "commentReference" => Self::CommentReference,
            "moveFrom" => Self::MoveFrom,
            "moveTo" => Self::MoveTo,
            "moveFromRangeStart" => Self::MoveFromRangeStart,
            "moveFromRangeEnd" => Self::MoveFromRangeEnd,
            "moveToRangeStart" => Self::MoveToRangeStart,
            "moveToRangeEnd" => Self::MoveToRangeEnd,
            "customXml" => Self::CustomXml,
            "sdt" => Self::Sdt,
            "sdtContent" => Self::SdtContent,
            "sdtPr" => Self::SdtPr,
            "proofErr" => Self::ProofErr,
            "annotationRef" => Self::AnnotationRef,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// Convert to the w: namespace local name.
    pub fn to_local_name(&self) -> &str {
        match self {
            Self::Document => "document",
            Self::Body => "body",
            Self::Paragraph => "p",
            Self::Run => "r",
            Self::Text => "t",
            Self::Break => "br",
            Self::Tab => "tab",
            Self::ParagraphProperties => "pPr",
            Self::RunProperties => "rPr",
            Self::Table => "tbl",
            Self::TableRow => "tr",
            Self::TableCell => "tc",
            Self::TableProperties => "tblPr",
            Self::TableRowProperties => "trPr",
            Self::TableCellProperties => "tcPr",
            Self::Hyperlink => "hyperlink",
            Self::BookmarkStart => "bookmarkStart",
            Self::BookmarkEnd => "bookmarkEnd",
            Self::FieldSimple => "fldSimple",
            Self::Drawing => "drawing",
            Self::InlineImage => "inline",
            Self::SectionProperties => "sectPr",
            Self::FootnoteReference => "footnoteReference",
            Self::EndnoteReference => "endnoteReference",
            Self::CommentReference => "commentReference",
            Self::MoveFrom => "moveFrom",
            Self::MoveTo => "moveTo",
            Self::MoveFromRangeStart => "moveFromRangeStart",
            Self::MoveFromRangeEnd => "moveFromRangeEnd",
            Self::MoveToRangeStart => "moveToRangeStart",
            Self::MoveToRangeEnd => "moveToRangeEnd",
            Self::CustomXml => "customXml",
            Self::Sdt => "sdt",
            Self::SdtContent => "sdtContent",
            Self::SdtPr => "sdtPr",
            Self::ProofErr => "proofErr",
            Self::AnnotationRef => "annotationRef",
            Self::ParagraphMark => "pPr", // same as ParagraphProperties for serialization
            Self::Unknown(s) => s.as_str(),
        }
    }

    /// Short path segment name (matches the path system).
    /// E.g., Paragraph -> "p", Run -> "r", Table -> "tbl"
    pub fn to_path_name(&self) -> &str {
        match self {
            Self::Body => "body",
            Self::Paragraph => "p",
            Self::Run => "r",
            Self::Text => "t",
            Self::Table => "tbl",
            Self::TableRow => "tr",
            Self::TableCell => "tc",
            Self::Hyperlink => "hyperlink",
            Self::Drawing => "drawing",
            Self::Break => "br",
            Self::Tab => "tab",
            Self::BookmarkStart => "bookmarkStart",
            Self::BookmarkEnd => "bookmarkEnd",
            Self::FootnoteReference => "footnoteRef",
            Self::EndnoteReference => "endnoteRef",
            Self::CommentReference => "commentRef",
            Self::MoveFrom => "moveFrom",
            Self::MoveTo => "moveTo",
            Self::Sdt => "sdt",
            Self::SdtContent => "sdtContent",
            other => other.to_local_name(),
        }
    }

    /// Whether this element type is a "body child" that gets indexed in paths.
    /// Only paragraphs and tables are direct children of body that get indexed.
    pub fn is_body_child(&self) -> bool {
        matches!(self, Self::Paragraph | Self::Table | Self::Sdt)
    }

    /// Whether this element is a "run container" whose children are runs.
    /// Paragraphs and hyperlinks contain runs.
    pub fn is_run_container(&self) -> bool {
        matches!(self, Self::Paragraph | Self::Hyperlink | Self::SdtContent)
    }

    /// Whether this element type carries text content (w:t).
    pub fn is_text_carrier(&self) -> bool {
        matches!(self, Self::Text)
    }
}

/// A node in the Word DOM tree.
#[derive(Debug, Clone)]
pub struct WordNode {
    /// The element type (e.g. Paragraph, Run, Text).
    pub element_type: WordElementType,
    /// XML attributes (key-value pairs).
    pub attributes: HashMap<String, String>,
    /// Text content (for w:t elements and text nodes).
    pub text_content: Option<String>,
    /// Child nodes.
    pub children: Vec<WordNode>,
    /// Whether w:t has xml:space="preserve".
    pub preserve_space: bool,
}

impl WordNode {
    pub fn new(element_type: WordElementType) -> Self {
        Self {
            element_type,
            attributes: HashMap::new(),
            text_content: None,
            children: Vec::new(),
            preserve_space: false,
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text_content = Some(text.into());
        self
    }

    pub fn with_attribute(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.attributes.insert(key.into(), val.into());
        self
    }

    pub fn with_children(mut self, children: Vec<WordNode>) -> Self {
        self.children = children;
        self
    }

    /// Get the run text from this paragraph.
    /// Concatenates all w:t text from child runs.
    pub fn paragraph_text(&self) -> String {
        let mut result = String::new();
        self.collect_text_into(&mut result);
        result
    }

    /// Recursively collect all text content from w:t children.
    fn collect_text_into(&self, buf: &mut String) {
        match &self.element_type {
            WordElementType::Text => {
                if let Some(t) = &self.text_content {
                    buf.push_str(t);
                }
            }
            WordElementType::Tab => {
                buf.push('\t');
            }
            WordElementType::Break => {
                // Line break within a run — add newline
                let break_type = self
                    .attributes
                    .get("type")
                    .map(|s| s.as_str())
                    .unwrap_or("");
                if break_type == "page" {
                    buf.push_str("\n--- Page Break ---\n");
                } else {
                    buf.push('\n');
                }
            }
            _ => {
                for child in &self.children {
                    child.collect_text_into(buf);
                }
            }
        }
    }

    /// Get all runs (w:r) that are direct children.
    pub fn runs(&self) -> Vec<&WordNode> {
        self.children
            .iter()
            .filter(|c| c.element_type == WordElementType::Run)
            .collect()
    }

    /// Get run properties (w:rPr) from this run, if present.
    pub fn run_properties(&self) -> Option<&WordNode> {
        self.children
            .iter()
            .find(|c| c.element_type == WordElementType::RunProperties)
    }

    /// Get paragraph properties (w:pPr) from this paragraph, if present.
    pub fn paragraph_properties(&self) -> Option<&WordNode> {
        self.children
            .iter()
            .find(|c| c.element_type == WordElementType::ParagraphProperties)
    }

    /// Check if the paragraph has a heading style.
    pub fn heading_level(&self) -> Option<u8> {
        let ppr = self.paragraph_properties()?;
        // Look for w:pStyle element in pPr children
        for child in &ppr.children {
            if child.element_type == WordElementType::Unknown("pStyle".into()) {
                if let Some(val) = child.attributes.get("val") {
                    if val.starts_with("Heading") || val.starts_with("heading") {
                        // Try to parse the number: "Heading1" -> 1, "Heading 1" -> 1
                        let num_part = val
                            .strip_prefix("Heading")
                            .or_else(|| val.strip_prefix("heading"))
                            .or_else(|| val.strip_prefix("Titre"))
                            .or_else(|| val.strip_prefix("titre"));
                        if let Some(num_str) = num_part {
                            let trimmed = num_str.trim();
                            if let Ok(n) = trimmed.parse::<u8>() {
                                return Some(n);
                            }
                        }
                        // Default heading level 1 if we can't parse
                        return Some(1);
                    }
                    if val == "Title" || val == "title" {
                        return Some(0); // Title = level 0
                    }
                }
            }
        }
        None
    }

    /// Check if run has bold property.
    pub fn is_bold(&self) -> bool {
        let rpr = self.run_properties();
        if let Some(rpr_node) = rpr {
            for child in &rpr_node.children {
                if child.element_type == WordElementType::Unknown("b".into()) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if run has italic property.
    pub fn is_italic(&self) -> bool {
        let rpr = self.run_properties();
        if let Some(rpr_node) = rpr {
            for child in &rpr_node.children {
                if child.element_type == WordElementType::Unknown("i".into()) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if the w:t element has xml:space="preserve".
    pub fn has_preserve_space(&self) -> bool {
        self.preserve_space
            || self.attributes.get("xml:space").map(|s| s.as_str()) == Some("preserve")
    }
}

/// The parsed Word document DOM.
/// Contains the root document node and provides search/navigation methods.
#[derive(Debug, Clone)]
pub struct WordDom {
    /// Root document element (w:document).
    pub root: WordNode,
}

impl WordDom {
    pub fn new(root: WordNode) -> Self {
        Self { root }
    }

    /// Get the body element (w:body) — first child of w:document.
    pub fn body(&self) -> Option<&WordNode> {
        self.root
            .children
            .iter()
            .find(|c| c.element_type == WordElementType::Body)
    }

    /// Get mutable body element.
    pub fn body_mut(&mut self) -> Option<&mut WordNode> {
        self.root
            .children
            .iter_mut()
            .find(|c| c.element_type == WordElementType::Body)
    }

    /// Get all body-level children (paragraphs, tables, sdt blocks).
    pub fn body_children(&self) -> Vec<&WordNode> {
        let body = self.body();
        if let Some(body) = body {
            body.children
                .iter()
                .filter(|c| c.element_type.is_body_child())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get all paragraphs in the body.
    pub fn paragraphs(&self) -> Vec<&WordNode> {
        let body = self.body();
        if let Some(body) = body {
            body.children
                .iter()
                .filter(|c| c.element_type == WordElementType::Paragraph)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get all tables in the body.
    pub fn tables(&self) -> Vec<&WordNode> {
        let body = self.body();
        if let Some(body) = body {
            body.children
                .iter()
                .filter(|c| c.element_type == WordElementType::Table)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Count body-level content elements (paragraphs + tables + sdt).
    pub fn body_content_count(&self) -> usize {
        self.body_children().len()
    }

    /// Extract all text from the document body as a single string.
    /// Paragraphs are separated by newlines.
    pub fn full_text(&self) -> String {
        let body = self.body();
        if let Some(body) = body {
            let mut result = String::new();
            let mut para_count = 0;
            for child in &body.children {
                if child.element_type == WordElementType::Paragraph {
                    if para_count > 0 {
                        result.push('\n');
                    }
                    result.push_str(&child.paragraph_text());
                    para_count += 1;
                } else if child.element_type == WordElementType::Table {
                    if para_count > 0 {
                        result.push('\n');
                    }
                    result.push_str(&self.table_text(child));
                    para_count += 1;
                } else if child.element_type == WordElementType::Sdt {
                    if para_count > 0 {
                        result.push('\n');
                    }
                    // Extract text from sdt content
                    for sdt_child in &child.children {
                        if sdt_child.element_type == WordElementType::SdtContent {
                            for content_child in &sdt_child.children {
                                if content_child.element_type == WordElementType::Paragraph {
                                    result.push_str(&content_child.paragraph_text());
                                }
                            }
                        }
                    }
                    para_count += 1;
                }
            }
            result
        } else {
            String::new()
        }
    }

    /// Extract text from a table element.
    pub fn table_text(&self, table: &WordNode) -> String {
        let mut result = String::new();
        let mut row_count = 0;
        for child in &table.children {
            if child.element_type == WordElementType::TableRow {
                if row_count > 0 {
                    result.push('\n');
                }
                let mut cell_texts = Vec::new();
                for tr_child in &child.children {
                    if tr_child.element_type == WordElementType::TableCell {
                        let mut cell_text = String::new();
                        for tc_child in &tr_child.children {
                            if tc_child.element_type == WordElementType::Paragraph {
                                if !cell_text.is_empty() {
                                    cell_text.push('\n');
                                }
                                cell_text.push_str(&tc_child.paragraph_text());
                            }
                        }
                        cell_texts.push(cell_text);
                    }
                }
                result.push_str(&cell_texts.join("\t"));
                row_count += 1;
            }
        }
        result
    }

    /// Compute document statistics.
    pub fn stats(&self) -> WordStats {
        let paragraphs = self.paragraphs();
        let tables = self.tables();
        let full_text = self.full_text();
        let word_count = count_words(&full_text);
        let char_count = full_text.chars().count();

        let mut heading_count = 0;
        for para in &paragraphs {
            if para.heading_level().is_some() {
                heading_count += 1;
            }
        }

        // Count runs and tables/rows/cells
        let mut run_count = 0;
        let mut row_count = 0;
        let mut cell_count = 0;
        for para in &paragraphs {
            run_count += para.runs().len();
        }
        for tbl in &tables {
            for child in &tbl.children {
                if child.element_type == WordElementType::TableRow {
                    row_count += 1;
                    for tr_child in &child.children {
                        if tr_child.element_type == WordElementType::TableCell {
                            cell_count += 1;
                        }
                    }
                }
            }
        }

        // Count inline images
        let mut image_count = 0;
        for para in &paragraphs {
            for run in para.runs() {
                for run_child in &run.children {
                    if run_child.element_type == WordElementType::Drawing {
                        image_count += 1;
                    }
                }
            }
        }

        WordStats {
            paragraph_count: paragraphs.len(),
            table_count: tables.len(),
            row_count,
            cell_count,
            run_count,
            word_count,
            char_count,
            heading_count,
            image_count,
        }
    }

    /// Build an outline from heading paragraphs.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        let paragraphs = self.paragraphs();
        let mut entries = Vec::new();
        for (i, para) in paragraphs.iter().enumerate() {
            if let Some(level) = para.heading_level() {
                entries.push(OutlineEntry {
                    level,
                    text: para.paragraph_text(),
                    para_index: i,
                });
            }
        }
        entries
    }
}

/// Document statistics.
#[derive(Debug, Clone)]
pub struct WordStats {
    pub paragraph_count: usize,
    pub table_count: usize,
    pub row_count: usize,
    pub cell_count: usize,
    pub run_count: usize,
    pub word_count: usize,
    pub char_count: usize,
    pub heading_count: usize,
    pub image_count: usize,
}

/// An outline entry (heading).
#[derive(Debug, Clone)]
pub struct OutlineEntry {
    pub level: u8,
    pub text: String,
    pub para_index: usize,
}

/// Count words in a text string (whitespace-separated tokens).
fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}
