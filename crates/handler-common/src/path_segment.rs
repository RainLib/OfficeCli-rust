/// A segment in a document path, e.g. "p[3]" from "/body/p[3]/r[1]".
#[derive(Debug, Clone)]
pub struct PathSegment {
    /// Element name (e.g. "p", "r", "tbl", "slide", "shape", "Sheet1")
    pub name: String,
    /// Optional positional index (1-based, matching XPath convention)
    pub index: Option<usize>,
    /// Optional attribute selector (e.g. [@paraId=XXX], [@id=5])
    pub attribute: Option<(String, String)>,
}

impl PathSegment {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            index: None,
            attribute: None,
        }
    }

    pub fn with_index(mut self, index: usize) -> Self {
        self.index = Some(index);
        self
    }

    pub fn with_attribute(mut self, key: &str, value: &str) -> Self {
        self.attribute = Some((key.to_string(), value.to_string()));
        self
    }

    /// Format as a path fragment, e.g. "p[3]" or "shape[@id=5]"
    pub fn to_path_fragment(&self) -> String {
        let base = &self.name;
        match (&self.index, &self.attribute) {
            (Some(idx), None) => format!("{}[{}]", base, idx),
            (None, Some((k, v))) => format!("{}[@{}={}]", base, k, v),
            (Some(idx), Some((k, v))) => format!("{}[{}][@{}={}]", base, idx, k, v),
            (None, None) => base.to_string(),
        }
    }
}
