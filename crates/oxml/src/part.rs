/// Represents a package part (XML or binary content within the OOXML ZIP).
pub struct PackagePart {
    /// Path within the ZIP archive (e.g. "word/document.xml")
    pub path: String,
    /// Content type (e.g. "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml")
    pub content_type: String,
    /// Whether this part has been modified
    pub dirty: bool,
}
