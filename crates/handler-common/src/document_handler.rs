use crate::output_format::{BinaryInfo, RawOptions, ViewOptions};
use crate::*;
use std::collections::HashMap;

/// Common interface for all document types (Word/Excel/PowerPoint/PDF).
/// Each handler implements the three-layer architecture:
///   - Semantic layer: view (text/annotated/outline/stats/issues)
///   - Query layer: get, query, set, add, remove, move, copy
///   - Raw layer: raw XML/PDF access
pub trait DocumentHandler: Send {
    // === Format identification ===
    fn format_name(&self) -> &str;

    // === Semantic Layer ===
    fn view_as_text(&self, opts: ViewOptions) -> Result<String, HandlerError>;
    fn view_as_annotated(&self, opts: ViewOptions) -> Result<String, HandlerError>;
    fn view_as_outline(&self) -> Result<String, HandlerError>;
    fn view_as_stats(&self) -> Result<String, HandlerError>;
    fn view_as_issues(
        &self,
        issue_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentIssue>, HandlerError>;

    // === Structured JSON ===
    fn view_as_text_json(&self, opts: ViewOptions) -> Result<serde_json::Value, HandlerError>;
    fn view_as_outline_json(&self) -> Result<serde_json::Value, HandlerError>;
    fn view_as_stats_json(&self) -> Result<serde_json::Value, HandlerError>;

    // === View modes (optional) ===
    fn view_as_html(&self, _opts: ViewOptions) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode("html".to_string()))
    }
    fn view_as_svg(&self) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode("svg".to_string()))
    }

    /// View form fields (docx content controls and legacy form fields)
    fn view_as_forms(&self) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode("forms".to_string()))
    }

    // === Query Layer ===
    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError>;
    fn query(&self, selector: &str) -> Result<Vec<DocumentNode>, HandlerError>;
    fn set(
        &self,
        path: &str,
        properties: &HashMap<String, String>,
    ) -> Result<Vec<String>, HandlerError>;
    fn add(
        &self,
        parent: &str,
        element_type: &str,
        position: InsertPosition,
        properties: &HashMap<String, String>,
        wrap: Option<&str>,
    ) -> Result<String, HandlerError>;
    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError>;
    /// Remove an element with optional command-level modifiers. Formats that
    /// do not define modifiers retain ordinary physical deletion.
    fn remove_with_properties(
        &self,
        path: &str,
        _properties: &HashMap<String, String>,
    ) -> Result<Option<String>, HandlerError> {
        self.remove(path)
    }
    fn move_element(
        &self,
        source: &str,
        target_parent: Option<&str>,
        position: InsertPosition,
    ) -> Result<String, HandlerError>;
    fn copy_from(
        &self,
        source: &str,
        target_parent: &str,
        position: InsertPosition,
    ) -> Result<String, HandlerError>;

    /// Swap two elements identified by DOM paths. Returns the resolved
    /// (left, right) paths after swap. Default impl reports unsupported.
    fn swap(&self, _path1: &str, _path2: &str) -> Result<(String, String), HandlerError> {
        Err(HandlerError::UnsupportedMode("swap".to_string()))
    }

    /// Merge template placeholders ({{key}}) with key-value data.
    /// Returns (replaced_count, unresolved_count).
    fn merge(&self, _data: &HashMap<String, String>) -> Result<MergeResult, HandlerError> {
        Err(HandlerError::UnsupportedMode("merge".to_string()))
    }

    // === Raw Layer ===
    fn raw(&self, part_path: &str, opts: RawOptions) -> Result<String, HandlerError>;
    fn raw_set(
        &self,
        part_path: &str,
        xpath: &str,
        action: &str,
        xml: Option<&str>,
    ) -> Result<(), HandlerError>;
    fn add_part(
        &self,
        parent: &str,
        part_type: &str,
        properties: Option<&HashMap<String, String>>,
    ) -> Result<(String, String), HandlerError>;
    /// Import delimited text into a worksheet.  Keeping this on the handler
    /// interface lets the command use the resident document rather than
    /// reopening the package from disk.
    fn import_csv(
        &self,
        _parent: &str,
        _content: &str,
        _delimiter: char,
        _has_header: bool,
        _start_cell: &str,
    ) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode("import".to_string()))
    }
    fn validate(&self) -> Result<Vec<ValidationError>, HandlerError>;
    fn try_extract_binary(
        &self,
        path: &str,
        dest: &str,
    ) -> Result<Option<BinaryInfo>, HandlerError>;
    fn save(&self) -> Result<(), HandlerError>;

    // === **NEW**: Text Offset Mapping ===
    fn extract_text_with_offsets(&self) -> Result<TextOffsetMap, HandlerError>;
}

/// Result of a template merge operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MergeResult {
    pub replaced_count: usize,
    pub unresolved_count: usize,
}

/// Handler error type.
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("path not found: {0}")]
    PathNotFound(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("unsupported element type: {0}")]
    UnsupportedType(String),

    #[error("unsupported mode: {0}")]
    UnsupportedMode(String),

    #[error("unsupported property: {0}")]
    UnsupportedProperty(String),

    #[error("operation failed: {0}")]
    OperationFailed(String),

    #[error("document open error: {0}")]
    OpenError(String),

    #[error("document save error: {0}")]
    SaveError(String),

    #[error("validation error: {0}")]
    ValidationError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),
}
