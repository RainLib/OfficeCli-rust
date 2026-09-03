use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDescriptor {
    pub format: String,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssetDescriptor {
    pub source_part: String,
    pub hash: String,
    pub href: String,
    pub byte_length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HcdCapabilities {
    pub text_patch: bool,
    pub annotations: bool,
    pub structure_patch: bool,
    pub style_patch: bool,
    pub exact_pagination: bool,
}

impl Default for HcdCapabilities {
    fn default() -> Self {
        Self {
            text_patch: true,
            annotations: true,
            structure_patch: false,
            style_patch: false,
            exact_pagination: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HcdManifest {
    pub schema_version: String,
    pub document_id: String,
    pub profile: String,
    pub revision: u64,
    pub source: SourceDescriptor,
    pub root_hash: String,
    pub annotation_root_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation_href: Option<String>,
    pub index_prefix: String,
    pub index_page_count: usize,
    pub chunk_count: usize,
    pub styles_href: String,
    pub capabilities: HcdCapabilities,
    /// Import-time fidelity contract for the canonical HTML representation.
    /// A source-backed export can still preserve opaque package parts exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fidelity: Option<FidelityReport>,
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<FidelityWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GridChunkAddress {
    /// Stable HCD identity for a worksheet. It is derived from documentId and
    /// the OOXML worksheet part, rather than from the display name.
    pub sheet_id: String,
    pub sheet_name: String,
    pub sheet_index: usize,
    pub sheet_state: String,
    pub kind: GridChunkKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_end: Option<u32>,
    /// Worksheet default grid dimensions in EMU. Cell-window descriptors
    /// carry these values so a virtualized canvas can establish the same
    /// coordinate system before it downloads drawing chunks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_column_width_emu: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_row_height_emu: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GridChunkKind {
    Cells,
    Picture,
    Chart,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChunkDescriptor {
    pub sequence: usize,
    pub chunk_id: String,
    pub region: String,
    pub html_href: String,
    pub html_hash: String,
    pub map_href: String,
    pub map_hash: String,
    pub byte_length: u64,
    pub block_count: usize,
    pub node_count: usize,
    pub text_chars: usize,
    pub node_bloom: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_node_id: Option<String>,
    #[serde(default)]
    pub continuation: bool,
    /// Optional format-specific random-access address. Grid clients can select
    /// visible worksheet windows without downloading every HTML fragment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<GridChunkAddress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChunkIndexPage {
    pub schema_version: String,
    pub revision: u64,
    pub page: usize,
    pub chunks: Vec<ChunkDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceAnchor {
    pub part: String,
    pub text_ordinal: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paragraph_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_id: Option<String>,
    pub node_kind: String,
    #[serde(default)]
    pub editable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeMapEntry {
    pub node_id: String,
    pub node_hash: String,
    pub source: SourceAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChunkSourceMap {
    pub schema_version: String,
    pub chunk_id: String,
    pub entries: Vec<NodeMapEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Annotation {
    pub annotation_id: String,
    pub node_id: String,
    pub start: usize,
    pub end: usize,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub ignored: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnnotationSet {
    pub schema_version: String,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchBatch {
    pub schema_version: String,
    pub document_id: String,
    pub patch_id: String,
    pub base_revision: u64,
    #[serde(default)]
    pub actor: BTreeMap<String, String>,
    pub operations: Vec<PatchOperation>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
pub enum PatchOperation {
    #[serde(rename = "text.splice", rename_all = "camelCase")]
    TextSplice {
        node_id: String,
        start: usize,
        delete_count: usize,
        insert_text: String,
        precondition: NodePrecondition,
    },
    /// Presentation-layer styling for one canonical editable text node.
    /// This changes HCD HTML and its root hash. Source-backed exporters must
    /// either support the style or reject the export before writing output.
    #[serde(rename = "node.style", rename_all = "camelCase")]
    NodeStyle {
        node_id: String,
        style: NodeStylePatch,
        precondition: NodePrecondition,
    },
    #[serde(rename = "annotation.upsert", rename_all = "camelCase")]
    AnnotationUpsert { annotation: Annotation },
    #[serde(rename = "annotation.remove", rename_all = "camelCase")]
    AnnotationRemove { annotation_id: String },
}

impl PatchOperation {
    pub fn node_id(&self) -> Option<&str> {
        match self {
            Self::TextSplice { node_id, .. } => Some(node_id),
            Self::NodeStyle { node_id, .. } => Some(node_id),
            Self::AnnotationUpsert { annotation } => Some(&annotation.node_id),
            Self::AnnotationRemove { .. } => None,
        }
    }

    pub fn is_content_change(&self) -> bool {
        matches!(self, Self::TextSplice { .. } | Self::NodeStyle { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeStylePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<NodeBorder>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeBorder {
    pub color: String,
    pub width_pt: f32,
    pub style: NodeBorderStyle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeBorderStyle {
    Solid,
    Dashed,
    Dotted,
    Double,
}

impl NodeBorderStyle {
    pub fn as_css(self) -> &'static str {
        match self {
            Self::Solid => "solid",
            Self::Dashed => "dashed",
            Self::Dotted => "dotted",
            Self::Double => "double",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodePrecondition {
    pub node_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyResult {
    pub document_id: String,
    pub patch_id: String,
    pub base_revision: u64,
    pub revision: u64,
    pub root_hash: String,
    pub annotation_root_hash: String,
    pub dirty_node_ids: Vec<String>,
    pub dirty_chunk_ids: Vec<String>,
    pub dirty_source_parts: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<FidelityWarning>,
    #[serde(default)]
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevisionRecord {
    pub schema_version: String,
    pub document_id: String,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch_base_revision: Option<u64>,
    pub root_hash: String,
    pub annotation_root_hash: String,
    pub index_prefix: String,
    pub created_at_epoch_ms: u128,
    #[serde(default)]
    pub dirty_node_ids: Vec<String>,
    #[serde(default)]
    pub dirty_chunk_ids: Vec<String>,
    #[serde(default)]
    pub dirty_source_parts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FidelityLevel {
    Exact,
    High,
    Semantic,
    Visual,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FidelityWarning {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_part: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FidelityReport {
    pub schema_version: String,
    pub level: FidelityLevel,
    #[serde(default)]
    pub preserved: Vec<String>,
    #[serde(default)]
    pub flattened: Vec<String>,
    #[serde(default)]
    pub dropped: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<FidelityWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationIssue {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValidationReport {
    pub valid: bool,
    pub document_id: Option<String>,
    pub revision: Option<u64>,
    pub issues: Vec<ValidationIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextExtractEntry {
    pub chunk_id: String,
    pub node_id: String,
    pub text: String,
    pub node_hash: String,
    pub source: SourceAnchor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextNodeLookup {
    pub document_id: String,
    pub revision: u64,
    #[serde(flatten)]
    pub node: TextExtractEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextExtractPage {
    pub document_id: String,
    pub revision: u64,
    pub entries: Vec<TextExtractEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ImportEvent {
    ImportStarted {
        document_id: String,
        source_sha256: String,
    },
    ChunkReady {
        descriptor: ChunkDescriptor,
    },
    AssetReady {
        hash: String,
        href: String,
        byte_length: u64,
    },
    Completed {
        manifest: HcdManifest,
    },
    Failed {
        document_id: String,
        error: String,
    },
}
