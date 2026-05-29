use serde::{Deserialize, Serialize};

/// Output format for CLI commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat {
    Text,
    Json,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Text
    }
}

/// Options for view commands (line range, column filter).
#[derive(Debug, Clone)]
pub struct ViewOptions {
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub max_lines: Option<usize>,
    pub cols: Option<Vec<String>>,
    pub page: Option<usize>,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            start_line: None,
            end_line: None,
            max_lines: None,
            cols: None,
            page: None,
        }
    }
}

/// Options for raw commands.
#[derive(Debug, Clone)]
pub struct RawOptions {
    pub start_row: Option<usize>,
    pub end_row: Option<usize>,
    pub cols: Option<Vec<String>>,
}

impl Default for RawOptions {
    fn default() -> Self {
        Self {
            start_row: None,
            end_row: None,
            cols: None,
        }
    }
}

/// Binary extraction result.
#[derive(Debug, Clone)]
pub struct BinaryInfo {
    pub content_type: String,
    pub byte_count: usize,
}
