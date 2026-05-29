use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a node in the document DOM tree.
/// Universal abstraction across Word/Excel/PowerPoint/PDF.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentNode {
    #[serde(rename = "path")]
    pub path: String,

    #[serde(rename = "type")]
    pub element_type: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,

    #[serde(default)]
    pub child_count: usize,

    #[serde(default)]
    pub format: HashMap<String, Option<serde_json::Value>>,

    #[serde(default)]
    pub children: Vec<DocumentNode>,
}

impl DocumentNode {
    pub fn new(path: &str, element_type: &str) -> Self {
        Self {
            path: path.to_string(),
            element_type: element_type.to_string(),
            text: None,
            preview: None,
            style: None,
            child_count: 0,
            format: HashMap::new(),
            children: Vec::new(),
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = Some(preview.into());
        self
    }

    pub fn with_style(mut self, style: impl Into<String>) -> Self {
        self.style = Some(style.into());
        self
    }

    pub fn with_format(mut self, key: &str, value: serde_json::Value) -> Self {
        self.format.insert(key.to_string(), Some(value));
        self
    }

    pub fn with_children(mut self, children: Vec<DocumentNode>) -> Self {
        self.child_count = children.len();
        self.children = children;
        self
    }
}
