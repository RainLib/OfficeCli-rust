mod bundle;
mod error;
mod hash;
mod html;
mod patch;
mod presentation;
mod types;
mod validate;

pub use bundle::{Bundle, BundleWriter, INDEX_PAGE_SIZE};
pub use error::HcdError;
pub use hash::{
    hash_bytes, hash_file, hash_reader, node_bloom, node_bloom_might_contain, stable_node_id,
};
pub use html::{
    extract_html_image_nodes, extract_html_text_nodes, image_visual_hash, validate_css_text,
    HtmlImageNode,
};
pub use patch::{
    apply_patch, extract_image_page, extract_text_page, get_image_node, get_text_node,
};
pub use presentation::{
    manifest_at_revision, render_standalone_html, render_standalone_html_with_transform,
    HtmlPresentationOptions, HtmlPresentationReport, DEFAULT_HTML_PRESENTATION_MAX_BYTES,
};
pub use types::*;
pub use validate::validate_bundle;

pub const HCD_SCHEMA_VERSION: &str = "hcd/1";
pub const HCD_PATCH_SCHEMA_VERSION: &str = "hcd-patch/1";
pub const HCD_PATCH_SCHEMA_VERSION_2: &str = "hcd-patch/2";
pub const HCD_PATCH_SCHEMA_VERSION_3: &str = "hcd-patch/3";
pub const HCD_SCHEMA_JSON: &str = include_str!("../schemas/hcd-1.schema.json");
pub const HCD_PATCH_SCHEMA_JSON: &str = include_str!("../schemas/hcd-patch-1.schema.json");
pub const HCD_PATCH_SCHEMA_V2_JSON: &str = include_str!("../schemas/hcd-patch-2.schema.json");
pub const HCD_PATCH_SCHEMA_V3_JSON: &str = include_str!("../schemas/hcd-patch-3.schema.json");
pub const DEFAULT_CHUNK_SOFT_BYTES: usize = 512 * 1024;
pub const DEFAULT_CHUNK_BLOCKS: usize = 256;
pub const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_CONTROL_PART_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_PATCH_JSON_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_STAGED_ASSET_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_REVISION: u64 = 100_000;

#[cfg(test)]
mod schema_tests {
    #[test]
    fn frozen_json_schemas_are_valid_json_and_have_stable_ids() {
        let hcd: serde_json::Value = serde_json::from_str(super::HCD_SCHEMA_JSON).unwrap();
        let patch: serde_json::Value = serde_json::from_str(super::HCD_PATCH_SCHEMA_JSON).unwrap();
        let patch_v2: serde_json::Value =
            serde_json::from_str(super::HCD_PATCH_SCHEMA_V2_JSON).unwrap();
        let patch_v3: serde_json::Value =
            serde_json::from_str(super::HCD_PATCH_SCHEMA_V3_JSON).unwrap();
        assert_eq!(hcd["$id"], "urn:officecli:hcd:1");
        assert_eq!(patch["$id"], "urn:officecli:hcd-patch:1");
        assert_eq!(patch_v2["$id"], "urn:officecli:hcd-patch:2");
        assert_eq!(patch_v3["$id"], "urn:officecli:hcd-patch:3");
    }

    #[test]
    fn frozen_types_reject_unknown_json_fields() {
        let source = r#"{"format":"docx","sha256":"00","sizeBytes":1,"sensitiveOriginal":"must-not-survive"}"#;
        assert!(serde_json::from_str::<super::SourceDescriptor>(source).is_err());

        let annotation = r#"{"annotationId":"a","nodeId":"n_00000000000000000000000000000000","start":0,"end":1,"kind":"mask","ignored":false,"originalText":"secret"}"#;
        assert!(serde_json::from_str::<super::Annotation>(annotation).is_err());
    }
}
