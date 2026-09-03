use hcd_core::{
    extract_text_page, Bundle, NodePrecondition, PatchBatch, PatchOperation,
    HCD_PATCH_SCHEMA_VERSION,
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let bundle_path = PathBuf::from(arguments.next().ok_or("missing HCD bundle path")?);
    let patch_path = PathBuf::from(arguments.next().ok_or("missing patch output path")?);
    if arguments.next().is_some() {
        return Err("usage: generate_stress_patch <bundle> <patch.json>".into());
    }
    if patch_path.exists() {
        return Err(format!("target already exists: {}", patch_path.display()).into());
    }

    let bundle = Bundle::open(bundle_path)?;
    let manifest = bundle.manifest()?;
    let page = extract_text_page(&bundle, None, 1)?;
    let entry = page
        .entries
        .first()
        .ok_or("stress bundle has no text node")?;
    let delete_count = usize::from(!entry.text.is_empty());
    let patch = PatchBatch {
        schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
        document_id: manifest.document_id,
        patch_id: "hcd-stress-single-node-export".to_string(),
        base_revision: manifest.revision,
        actor: BTreeMap::from([("type".to_string(), "STRESS_TEST".to_string())]),
        operations: vec![PatchOperation::TextSplice {
            node_id: entry.node_id.clone(),
            start: 0,
            delete_count,
            insert_text: "*".to_string(),
            precondition: NodePrecondition {
                node_hash: entry.node_hash.clone(),
            },
        }],
        metadata: BTreeMap::from([("reason".to_string(), "LOW_MEMORY_EXPORT".to_string())]),
    };
    serde_json::to_writer_pretty(BufWriter::new(File::create(patch_path)?), &patch)?;
    Ok(())
}
