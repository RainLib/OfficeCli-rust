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
    fn view_as_html(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode("html".to_string()))
    }
    fn view_as_svg(&self) -> Result<String, HandlerError> {
        Err(HandlerError::UnsupportedMode("svg".to_string()))
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
    ) -> Result<String, HandlerError>;
    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError>;
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
