use crate::bundle::{
    atomic_write_json, finalize_root_hash, hash_descriptor, now_epoch_ms, read_json_bounded,
    Bundle, INDEX_PAGE_SIZE,
};
use crate::hash::{hash_bytes, node_bloom_might_contain};
use crate::{
    extract_html_image_nodes, extract_html_text_nodes, image_visual_hash, AnnotationSet,
    ApplyResult, AssetDescriptor, FidelityWarning, HcdError, ImageExtractEntry, ImageExtractPage,
    ImageGeometry, ImageGeometryUnit, ImageNodeLookup, ImageNodeState, NodeStylePatch, PatchBatch,
    PatchOperation, RevisionRecord, TextExtractEntry, TextExtractPage, TextNodeLookup,
    HCD_PATCH_SCHEMA_VERSION, HCD_PATCH_SCHEMA_VERSION_2, HCD_PATCH_SCHEMA_VERSION_3,
    HCD_SCHEMA_VERSION, MAX_CONTROL_PART_BYTES, MAX_PATCH_JSON_BYTES,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;

const MAX_PATCH_OPERATIONS: usize = 10_000;
const MAX_PATCH_INSERT_BYTES: usize = 2 * 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;
const MAX_ANNOTATION_KIND_BYTES: usize = 128;
const MAX_ACTOR_ENTRIES: usize = 64;
const MAX_METADATA_ENTRIES: usize = 128;
const MAX_ACTOR_BYTES: usize = 64 * 1024;
const MAX_METADATA_BYTES: usize = 256 * 1024;

#[derive(Clone)]
struct Splice {
    start: usize,
    delete_count: usize,
    insert_text: String,
    node_hash: String,
}

#[derive(Clone)]
struct StyleChange {
    style: NodeStylePatch,
    node_hash: String,
}

#[derive(Clone, Default)]
struct ImageChange {
    asset_hash: Option<String>,
    geometry: Option<ImageGeometry>,
    visual_hash: String,
}

pub fn apply_patch(
    bundle: &Bundle,
    patch: &PatchBatch,
    expected_revision: u64,
) -> Result<ApplyResult, HcdError> {
    let _write_guard = bundle.acquire_write_lock()?;
    let mut manifest = bundle.manifest()?;
    if manifest.revision > crate::MAX_REVISION {
        return Err(HcdError::ResourceLimit(format!(
            "manifest revision {} exceeds the maximum {}",
            manifest.revision,
            crate::MAX_REVISION
        )));
    }
    validate_patch_identity(&manifest, patch)?;
    let patch_bytes = serde_json::to_vec(patch)?;
    if patch_bytes.len() as u64 > MAX_PATCH_JSON_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "serialized patch is {} bytes; maximum is {MAX_PATCH_JSON_BYTES}",
            patch_bytes.len()
        )));
    }
    let patch_hash = hash_bytes(&patch_bytes);
    if let Some(result) = find_idempotent_result(bundle, &manifest, &patch.patch_id, &patch_hash)? {
        return Ok(result);
    }
    validate_patch_header(&manifest, patch, expected_revision)?;

    let mut stale_content_nodes = HashSet::new();
    if patch.base_revision < manifest.revision {
        for revision in patch.base_revision + 1..=manifest.revision {
            let record = bundle.revision(revision)?;
            stale_content_nodes.extend(record.dirty_node_ids);
        }
        if let Some(node_id) = patch
            .operations
            .iter()
            .filter(|operation| operation.is_content_change())
            .filter_map(PatchOperation::node_id)
            .find(|node_id| stale_content_nodes.contains(*node_id))
        {
            return Err(HcdError::RevisionConflict(format!(
                "node {node_id} changed after base revision {}",
                patch.base_revision
            )));
        }
    }

    let splices = collect_splices(patch)?;
    let styles = collect_styles(patch)?;
    let images = collect_image_changes(patch)?;
    let annotation_node_ids: HashSet<String> = patch
        .operations
        .iter()
        .filter_map(|operation| match operation {
            PatchOperation::AnnotationUpsert { annotation } => Some(annotation.node_id.clone()),
            _ => None,
        })
        .collect();
    let target_node_ids: HashSet<String> = splices
        .keys()
        .cloned()
        .chain(styles.keys().cloned())
        .chain(images.keys().cloned())
        .chain(annotation_node_ids.iter().cloned())
        .collect();

    let new_revision = manifest.revision + 1;
    let new_index_prefix = format!("indexes/rev-{new_revision:020}");
    let new_index_root = bundle.root().join(&new_index_prefix);
    let content_changed = !splices.is_empty() || !styles.is_empty() || !images.is_empty();
    if content_changed {
        fs::create_dir_all(&new_index_root)?;
    }

    let mut found_nodes: HashMap<String, usize> = HashMap::new();
    let mut dirty_nodes = BTreeSet::new();
    let mut dirty_chunks = BTreeSet::new();
    let mut dirty_parts = BTreeSet::new();
    let mut root_hasher = Sha256::new();
    let current_asset_index_href = bundle.asset_index_href_for_revision(manifest.revision)?;
    let mut asset_index = bundle.read_asset_index_for_revision(manifest.revision)?;
    let mut asset_by_hash: HashMap<String, AssetDescriptor> = asset_index
        .iter()
        .map(|asset| (asset.hash.clone(), asset.clone()))
        .collect();
    for change in images.values() {
        let Some(hash) = change.asset_hash.as_deref() else {
            continue;
        };
        if asset_by_hash.contains_key(hash) {
            continue;
        }
        let staged = bundle.staged_asset(hash)?;
        validate_staged_asset(bundle, &staged, hash)?;
        asset_by_hash.insert(hash.to_string(), staged.clone());
        asset_index.push(staged);
    }
    asset_index.sort_by(|left, right| left.hash.cmp(&right.hash));
    asset_index.dedup_by(|left, right| left.hash == right.hash);
    let asset_index_href =
        if asset_index == bundle.read_asset_index_for_revision(manifest.revision)? {
            current_asset_index_href
        } else {
            bundle.write_json_object("assets/indexes", &asset_index)?.0
        };

    for page_number in 0..manifest.index_page_count {
        let mut page = bundle.read_index_page(&manifest, page_number)?;
        for descriptor in &mut page.chunks {
            let candidates: Vec<&String> = target_node_ids
                .iter()
                .filter(|node_id| node_bloom_might_contain(&descriptor.node_bloom, node_id))
                .collect();
            if candidates.is_empty() {
                hash_descriptor(&mut root_hasher, descriptor);
                continue;
            }

            let mut source_map = bundle.read_map(descriptor)?;
            let mut html = bundle.read_chunk(descriptor)?;
            let mut html_nodes = extract_html_text_nodes(&html)?;
            let mut image_nodes = extract_html_image_nodes(&html)?;
            let mut chunk_changed = false;
            for entry in &mut source_map.entries {
                if !target_node_ids.contains(&entry.node_id) {
                    continue;
                }
                let current_text = html_nodes.get(&entry.node_id).ok_or_else(|| {
                    HcdError::InvalidBundle(format!(
                        "mapped node {} is missing from canonical HTML",
                        entry.node_id
                    ))
                })?;
                let actual_node_hash = hash_bytes(current_text.as_bytes());
                if actual_node_hash != entry.node_hash {
                    return Err(HcdError::InvalidBundle(format!(
                        "node {} HTML hash {} does not match source-map hash {}",
                        entry.node_id, actual_node_hash, entry.node_hash
                    )));
                }
                found_nodes.insert(entry.node_id.clone(), current_text.chars().count());
                if entry.source.node_kind == "image"
                    && (styles.contains_key(&entry.node_id) || splices.contains_key(&entry.node_id))
                {
                    return Err(HcdError::Unsupported(format!(
                        "image node {} accepts image.replace/image.geometry, not text or node.style operations",
                        entry.node_id
                    )));
                }
                if let Some(change) = images.get(&entry.node_id) {
                    if entry.source.node_kind != "image" {
                        return Err(HcdError::Unsupported(format!(
                            "node {} is not an image node",
                            entry.node_id
                        )));
                    }
                    let current = image_nodes.get(&entry.node_id).ok_or_else(|| {
                        HcdError::InvalidBundle(format!(
                            "mapped image node {} is missing visual state",
                            entry.node_id
                        ))
                    })?;
                    if current.visual_hash != change.visual_hash {
                        return Err(HcdError::PreconditionFailed(format!(
                            "image node {} expected visual hash {}, actual {}",
                            entry.node_id, change.visual_hash, current.visual_hash
                        )));
                    }
                    let asset_hash = change
                        .asset_hash
                        .clone()
                        .or_else(|| current.asset_hash.clone());
                    let geometry = change.geometry.clone().or_else(|| current.geometry.clone());
                    if let Some(hash) = change.asset_hash.as_deref() {
                        let asset = asset_by_hash.get(hash).ok_or_else(|| {
                            HcdError::InvalidBundle(format!("asset {hash} is unavailable"))
                        })?;
                        replace_image_asset(&mut html, &entry.node_id, asset)?;
                    }
                    if let Some(geometry) = change.geometry.as_ref() {
                        replace_image_geometry(&mut html, &entry.node_id, geometry)?;
                    }
                    let new_visual_hash =
                        image_visual_hash(asset_hash.as_deref(), geometry.as_ref());
                    set_element_attribute(
                        &mut html,
                        &entry.node_id,
                        "data-hcd-visual-hash",
                        &new_visual_hash,
                    )?;
                    image_nodes.insert(
                        entry.node_id.clone(),
                        crate::HtmlImageNode {
                            visual_hash: new_visual_hash,
                            asset_hash,
                            geometry,
                        },
                    );
                    dirty_nodes.insert(entry.node_id.clone());
                    dirty_parts.insert(entry.source.part.clone());
                    chunk_changed = true;
                }
                let style_change = styles.get(&entry.node_id);
                if let Some(change) = style_change {
                    if !entry.source.editable {
                        return Err(HcdError::Unsupported(format!(
                            "node {} is read-only",
                            entry.node_id
                        )));
                    }
                    if change.node_hash != entry.node_hash {
                        return Err(HcdError::PreconditionFailed(format!(
                            "node {} expected hash {}, actual {}",
                            entry.node_id, change.node_hash, entry.node_hash
                        )));
                    }
                }
                if let Some(node_splices) = splices.get(&entry.node_id) {
                    if !entry.source.editable {
                        return Err(HcdError::Unsupported(format!(
                            "node {} is read-only",
                            entry.node_id
                        )));
                    }
                    for splice in node_splices {
                        if splice.node_hash != entry.node_hash {
                            return Err(HcdError::PreconditionFailed(format!(
                                "node {} expected hash {}, actual {}",
                                entry.node_id, splice.node_hash, entry.node_hash
                            )));
                        }
                    }
                    let replacement = splice_text(current_text, node_splices)?;
                    let replacement_hash = hash_bytes(replacement.as_bytes());
                    replace_node_text(&mut html, &entry.node_id, &replacement, &replacement_hash)?;
                    entry.node_hash = replacement_hash;
                    found_nodes.insert(entry.node_id.clone(), replacement.chars().count());
                    html_nodes.insert(entry.node_id.clone(), replacement);
                    dirty_nodes.insert(entry.node_id.clone());
                    dirty_parts.insert(entry.source.part.clone());
                    chunk_changed = true;
                }
                if let Some(change) = style_change {
                    apply_node_style(&mut html, &entry.node_id, &change.style)?;
                    dirty_nodes.insert(entry.node_id.clone());
                    dirty_parts.insert(entry.source.part.clone());
                    chunk_changed = true;
                }
            }

            if chunk_changed {
                let (html_href, html_hash) = bundle.write_chunk_object(&html)?;
                let (map_href, map_hash) = bundle.write_json_object("maps", &source_map)?;
                descriptor.html_href = html_href;
                descriptor.html_hash = html_hash;
                descriptor.map_href = map_href;
                descriptor.map_hash = map_hash;
                descriptor.byte_length = html.len() as u64;
                descriptor.text_chars = html_nodes.values().map(|text| text.chars().count()).sum();
                dirty_chunks.insert(descriptor.chunk_id.clone());
            }
            hash_descriptor(&mut root_hasher, descriptor);
        }

        if content_changed {
            page.revision = new_revision;
            let new_path = new_index_root.join(format!("{page_number:06}.json"));
            // Each index page is bounded to 128 descriptors. The descriptors
            // and their content-addressed objects can be reused, but the
            // page-level revision must belong to the new immutable view.
            atomic_write_json(&new_path, &page)?;
        }
    }

    for node_id in &target_node_ids {
        if !found_nodes.contains_key(node_id) {
            return Err(HcdError::NodeNotFound(node_id.clone()));
        }
    }

    validate_annotation_ranges(patch, &found_nodes)?;
    let (annotation_href, annotation_root_hash) = apply_annotations(bundle, &manifest, patch)?;
    let root_hash = if content_changed {
        finalize_root_hash(bundle, root_hasher, &asset_index_href)?
    } else {
        manifest.root_hash.clone()
    };

    manifest.revision = new_revision;
    manifest.root_hash = root_hash.clone();
    manifest.annotation_root_hash = annotation_root_hash.clone();
    manifest.annotation_href = annotation_href;
    if content_changed {
        manifest.index_prefix = new_index_prefix.clone();
    }

    let result = ApplyResult {
        document_id: manifest.document_id.clone(),
        patch_id: patch.patch_id.clone(),
        base_revision: patch.base_revision,
        revision: new_revision,
        root_hash: root_hash.clone(),
        annotation_root_hash: annotation_root_hash.clone(),
        dirty_node_ids: dirty_nodes.iter().cloned().collect(),
        dirty_chunk_ids: dirty_chunks.iter().cloned().collect(),
        dirty_source_parts: dirty_parts.iter().cloned().collect(),
        warnings: styles
            .keys()
            .map(|node_id| FidelityWarning {
                code: "HCD_PRESENTATION_STYLE_ONLY".to_string(),
                message: "node.style changes canonical HCD/HTML presentation; current source-backed Office/PDF exporters reject revisions containing presentation styles instead of silently dropping them".to_string(),
                node_id: Some(node_id.clone()),
                source_part: None,
            })
            .chain(images.keys().map(|node_id| FidelityWarning {
                code: "HCD_IMAGE_PATCH_SEMANTIC_EXPORT".to_string(),
                message: "image changes are canonical in HCD and pure-Rust semantic exports; source-backed Office/PDF export rejects them before writing until format-specific media rewrites are implemented".to_string(),
                node_id: Some(node_id.clone()),
                source_part: None,
            }))
            .collect(),
        idempotent_replay: false,
    };
    let record = RevisionRecord {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        document_id: manifest.document_id.clone(),
        revision: new_revision,
        parent_revision: Some(new_revision - 1),
        patch_id: Some(patch.patch_id.clone()),
        patch_hash: Some(patch_hash),
        patch_base_revision: Some(patch.base_revision),
        root_hash,
        annotation_root_hash,
        index_prefix: manifest.index_prefix.clone(),
        asset_index_href,
        created_at_epoch_ms: now_epoch_ms(),
        dirty_node_ids: result.dirty_node_ids.clone(),
        dirty_chunk_ids: result.dirty_chunk_ids.clone(),
        dirty_source_parts: result.dirty_source_parts.clone(),
    };
    bundle.write_revision(&record)?;
    bundle.write_manifest(&manifest)?;
    Ok(result)
}

pub fn extract_text_page(
    bundle: &Bundle,
    cursor: Option<&str>,
    limit: usize,
) -> Result<TextExtractPage, HcdError> {
    let manifest = bundle.manifest()?;
    let limit = limit.clamp(1, 10_000);
    let (mut sequence, mut entry_offset) = parse_cursor(cursor)?;
    let mut entries = Vec::with_capacity(limit.min(1024));

    while sequence < manifest.chunk_count && entries.len() < limit {
        let page_number = sequence / INDEX_PAGE_SIZE;
        let descriptor_offset = sequence % INDEX_PAGE_SIZE;
        let page = bundle.read_index_page(&manifest, page_number)?;
        let descriptor = page.chunks.get(descriptor_offset).ok_or_else(|| {
            HcdError::InvalidBundle(format!("missing chunk descriptor at sequence {sequence}"))
        })?;
        let source_map = bundle.read_map(descriptor)?;
        let html = bundle.read_chunk(descriptor)?;
        let html_nodes = extract_html_text_nodes(&html)?;
        while entry_offset < source_map.entries.len() && entries.len() < limit {
            let entry = &source_map.entries[entry_offset];
            if entry.source.node_kind == "image" {
                entry_offset += 1;
                continue;
            }
            let text = html_nodes.get(&entry.node_id).ok_or_else(|| {
                HcdError::InvalidBundle(format!(
                    "mapped node {} is missing from canonical HTML",
                    entry.node_id
                ))
            })?;
            let actual_hash = hash_bytes(text.as_bytes());
            if actual_hash != entry.node_hash {
                return Err(HcdError::InvalidBundle(format!(
                    "node {} HTML hash {} does not match source-map hash {}",
                    entry.node_id, actual_hash, entry.node_hash
                )));
            }
            entries.push(TextExtractEntry {
                chunk_id: descriptor.chunk_id.clone(),
                node_id: entry.node_id.clone(),
                text: text.clone(),
                node_hash: entry.node_hash.clone(),
                source: entry.source.clone(),
            });
            entry_offset += 1;
        }
        if entry_offset >= source_map.entries.len() {
            sequence += 1;
            entry_offset = 0;
        }
    }

    let next_cursor =
        (sequence < manifest.chunk_count).then(|| format!("{sequence}:{entry_offset}"));
    Ok(TextExtractPage {
        document_id: manifest.document_id,
        revision: manifest.revision,
        entries,
        next_cursor,
    })
}

/// Resolve one editable-text IR node by its stable HCD node ID at the current
/// bundle revision. Chunk bloom filters and source maps are consulted before
/// materializing HTML, so lookup does not require a full-document text buffer.
pub fn get_text_node(bundle: &Bundle, node_id: &str) -> Result<TextNodeLookup, HcdError> {
    validate_node_id(node_id)?;
    let manifest = bundle.manifest()?;
    let mut found = None;

    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(&manifest, page_number)?;
        for descriptor in &page.chunks {
            if !node_bloom_might_contain(&descriptor.node_bloom, node_id) {
                continue;
            }
            let source_map = bundle.read_map(descriptor)?;
            let mut matches = source_map
                .entries
                .iter()
                .filter(|entry| entry.node_id == node_id);
            let Some(entry) = matches.next() else {
                continue;
            };
            if entry.source.node_kind == "image" {
                continue;
            }
            if matches.next().is_some() || found.is_some() {
                return Err(HcdError::InvalidBundle(format!(
                    "node ID {node_id} occurs in more than one source map"
                )));
            }
            let html = bundle.read_chunk(descriptor)?;
            let html_nodes = extract_html_text_nodes(&html)?;
            let text = html_nodes.get(node_id).ok_or_else(|| {
                HcdError::InvalidBundle(format!(
                    "mapped node {node_id} is missing from canonical HTML"
                ))
            })?;
            let actual_hash = hash_bytes(text.as_bytes());
            if actual_hash != entry.node_hash {
                return Err(HcdError::InvalidBundle(format!(
                    "node {node_id} HTML hash {actual_hash} does not match source-map hash {}",
                    entry.node_hash
                )));
            }
            found = Some(TextExtractEntry {
                chunk_id: descriptor.chunk_id.clone(),
                node_id: entry.node_id.clone(),
                text: text.clone(),
                node_hash: entry.node_hash.clone(),
                source: entry.source.clone(),
            });
        }
    }

    let node = found.ok_or_else(|| HcdError::NodeNotFound(node_id.to_string()))?;
    Ok(TextNodeLookup {
        document_id: manifest.document_id,
        revision: manifest.revision,
        node,
    })
}

pub fn get_image_node(bundle: &Bundle, node_id: &str) -> Result<ImageNodeLookup, HcdError> {
    validate_node_id(node_id)?;
    let manifest = bundle.manifest()?;
    let mut found = None;
    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(&manifest, page_number)?;
        for descriptor in &page.chunks {
            if !node_bloom_might_contain(&descriptor.node_bloom, node_id) {
                continue;
            }
            let source_map = bundle.read_map(descriptor)?;
            let Some(entry) = source_map
                .entries
                .iter()
                .find(|entry| entry.node_id == node_id && entry.source.node_kind == "image")
            else {
                continue;
            };
            if found.is_some() {
                return Err(HcdError::InvalidBundle(format!(
                    "image node ID {node_id} occurs in more than one source map"
                )));
            }
            let html = bundle.read_chunk(descriptor)?;
            let images = extract_html_image_nodes(&html)?;
            let image = images.get(node_id).ok_or_else(|| {
                HcdError::InvalidBundle(format!(
                    "mapped image node {node_id} is missing from canonical HTML"
                ))
            })?;
            found = Some(ImageNodeLookup {
                document_id: manifest.document_id.clone(),
                revision: manifest.revision,
                chunk_id: descriptor.chunk_id.clone(),
                node: ImageNodeState {
                    node_id: node_id.to_string(),
                    visual_hash: image.visual_hash.clone(),
                    asset_hash: image.asset_hash.clone(),
                    geometry: image.geometry.clone(),
                    source: entry.source.clone(),
                },
            });
        }
    }
    found.ok_or_else(|| HcdError::NodeNotFound(node_id.to_string()))
}

pub fn extract_image_page(
    bundle: &Bundle,
    cursor: Option<&str>,
    limit: usize,
) -> Result<ImageExtractPage, HcdError> {
    let manifest = bundle.manifest()?;
    let limit = limit.clamp(1, 10_000);
    let (mut sequence, mut entry_offset) = parse_cursor(cursor)?;
    let mut entries = Vec::with_capacity(limit.min(256));
    while sequence < manifest.chunk_count && entries.len() < limit {
        let page_number = sequence / INDEX_PAGE_SIZE;
        let descriptor_offset = sequence % INDEX_PAGE_SIZE;
        let page = bundle.read_index_page(&manifest, page_number)?;
        let descriptor = page.chunks.get(descriptor_offset).ok_or_else(|| {
            HcdError::InvalidBundle(format!("missing chunk descriptor at sequence {sequence}"))
        })?;
        let source_map = bundle.read_map(descriptor)?;
        let mut images = None;
        while entry_offset < source_map.entries.len() && entries.len() < limit {
            let entry = &source_map.entries[entry_offset];
            entry_offset += 1;
            if entry.source.node_kind != "image" {
                continue;
            }
            let images = match &images {
                Some(images) => images,
                None => images.insert(extract_html_image_nodes(&bundle.read_chunk(descriptor)?)?),
            };
            let image = images.get(&entry.node_id).ok_or_else(|| {
                HcdError::InvalidBundle(format!(
                    "mapped image node {} is missing from canonical HTML",
                    entry.node_id
                ))
            })?;
            entries.push(ImageExtractEntry {
                chunk_id: descriptor.chunk_id.clone(),
                node: ImageNodeState {
                    node_id: entry.node_id.clone(),
                    visual_hash: image.visual_hash.clone(),
                    asset_hash: image.asset_hash.clone(),
                    geometry: image.geometry.clone(),
                    source: entry.source.clone(),
                },
            });
        }
        if entry_offset >= source_map.entries.len() {
            sequence += 1;
            entry_offset = 0;
        }
    }
    let next_cursor =
        (sequence < manifest.chunk_count).then(|| format!("{sequence}:{entry_offset}"));
    Ok(ImageExtractPage {
        document_id: manifest.document_id,
        revision: manifest.revision,
        entries,
        next_cursor,
    })
}

fn validate_patch_header(
    manifest: &crate::HcdManifest,
    patch: &PatchBatch,
    expected_revision: u64,
) -> Result<(), HcdError> {
    if manifest.revision >= crate::MAX_REVISION {
        return Err(HcdError::ResourceLimit(format!(
            "HCD revision limit {} has been reached; compact or archive the document before applying more patches",
            crate::MAX_REVISION
        )));
    }
    validate_patch_identity(manifest, patch)?;
    if manifest.revision != expected_revision {
        return Err(HcdError::RevisionConflict(format!(
            "expected head {expected_revision}, actual {}",
            manifest.revision
        )));
    }
    if patch.base_revision > manifest.revision {
        return Err(HcdError::RevisionConflict(format!(
            "base revision {} is ahead of head {}",
            patch.base_revision, manifest.revision
        )));
    }
    Ok(())
}

fn validate_patch_identity(
    manifest: &crate::HcdManifest,
    patch: &PatchBatch,
) -> Result<(), HcdError> {
    if patch.schema_version != HCD_PATCH_SCHEMA_VERSION
        && patch.schema_version != HCD_PATCH_SCHEMA_VERSION_2
        && patch.schema_version != HCD_PATCH_SCHEMA_VERSION_3
    {
        return Err(HcdError::InvalidPatch(format!(
            "unsupported schema version {}",
            patch.schema_version
        )));
    }
    validate_identifier("documentId", &patch.document_id)?;
    if patch.document_id != manifest.document_id {
        return Err(HcdError::InvalidPatch(
            "documentId does not match the bundle manifest".to_string(),
        ));
    }
    if patch.patch_id.trim().is_empty() {
        return Err(HcdError::InvalidPatch("patchId is required".to_string()));
    }
    if patch.patch_id.len() > MAX_IDENTIFIER_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "patchId exceeds {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    if patch.operations.is_empty() || patch.operations.len() > MAX_PATCH_OPERATIONS {
        return Err(HcdError::ResourceLimit(format!(
            "patch operation count must be between 1 and {MAX_PATCH_OPERATIONS}"
        )));
    }
    validate_string_map("actor", &patch.actor, MAX_ACTOR_ENTRIES, MAX_ACTOR_BYTES)?;
    validate_string_map(
        "metadata",
        &patch.metadata,
        MAX_METADATA_ENTRIES,
        MAX_METADATA_BYTES,
    )?;
    let mut inserted = 0usize;
    for operation in &patch.operations {
        match operation {
            PatchOperation::TextSplice {
                node_id,
                insert_text,
                precondition,
                ..
            } => {
                validate_node_id(node_id)?;
                validate_sha256("nodeHash", &precondition.node_hash)?;
                if insert_text.len() > MAX_PATCH_INSERT_BYTES {
                    return Err(HcdError::ResourceLimit(format!(
                        "one text.splice inserts {} bytes; maximum is {MAX_PATCH_INSERT_BYTES}",
                        insert_text.len()
                    )));
                }
                inserted = inserted.checked_add(insert_text.len()).ok_or_else(|| {
                    HcdError::ResourceLimit("patch insert byte count overflowed".to_string())
                })?;
            }
            PatchOperation::NodeStyle {
                node_id,
                style,
                precondition,
            } => {
                if patch.schema_version == HCD_PATCH_SCHEMA_VERSION {
                    return Err(HcdError::InvalidPatch(
                        "node.style requires schemaVersion hcd-patch/2".to_string(),
                    ));
                }
                validate_node_id(node_id)?;
                validate_sha256("nodeHash", &precondition.node_hash)?;
                validate_node_style(style)?;
            }
            PatchOperation::ImageReplace {
                node_id,
                asset_hash,
                precondition,
            } => {
                if patch.schema_version != HCD_PATCH_SCHEMA_VERSION_3 {
                    return Err(HcdError::InvalidPatch(
                        "image.replace requires schemaVersion hcd-patch/3".to_string(),
                    ));
                }
                validate_node_id(node_id)?;
                validate_sha256("assetHash", asset_hash)?;
                validate_sha256("visualHash", &precondition.visual_hash)?;
            }
            PatchOperation::ImageGeometry {
                node_id,
                geometry,
                precondition,
            } => {
                if patch.schema_version != HCD_PATCH_SCHEMA_VERSION_3 {
                    return Err(HcdError::InvalidPatch(
                        "image.geometry requires schemaVersion hcd-patch/3".to_string(),
                    ));
                }
                validate_node_id(node_id)?;
                validate_sha256("visualHash", &precondition.visual_hash)?;
                crate::html::validate_image_geometry(geometry)?;
            }
            PatchOperation::AnnotationUpsert { annotation } => {
                validate_annotation(annotation)?;
            }
            PatchOperation::AnnotationRemove { annotation_id } => {
                validate_identifier("annotationId", annotation_id)?;
            }
        }
    }
    if inserted > MAX_PATCH_INSERT_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "patch inserts {inserted} bytes; maximum is {MAX_PATCH_INSERT_BYTES}"
        )));
    }
    Ok(())
}

fn validate_string_map(
    name: &str,
    values: &BTreeMap<String, String>,
    maximum_entries: usize,
    maximum_bytes: usize,
) -> Result<(), HcdError> {
    if values.len() > maximum_entries {
        return Err(HcdError::ResourceLimit(format!(
            "{name} contains {} entries; maximum is {maximum_entries}",
            values.len()
        )));
    }
    let mut bytes = 0usize;
    for (key, value) in values {
        if key.is_empty() || key.len() > MAX_IDENTIFIER_BYTES {
            return Err(HcdError::InvalidPatch(format!(
                "{name} key length must be between 1 and {MAX_IDENTIFIER_BYTES} bytes"
            )));
        }
        bytes = bytes
            .checked_add(key.len())
            .and_then(|total| total.checked_add(value.len()))
            .ok_or_else(|| HcdError::ResourceLimit(format!("{name} byte count overflowed")))?;
    }
    if bytes > maximum_bytes {
        return Err(HcdError::ResourceLimit(format!(
            "{name} contains {bytes} bytes; maximum is {maximum_bytes}"
        )));
    }
    Ok(())
}

fn validate_annotation(annotation: &crate::Annotation) -> Result<(), HcdError> {
    validate_identifier("annotationId", &annotation.annotation_id)?;
    validate_node_id(&annotation.node_id)?;
    if annotation.kind.trim().is_empty() || annotation.kind.len() > MAX_ANNOTATION_KIND_BYTES {
        return Err(HcdError::InvalidPatch(format!(
            "annotation kind length must be between 1 and {MAX_ANNOTATION_KIND_BYTES} bytes"
        )));
    }
    if let Some(rule_id) = &annotation.rule_id {
        validate_identifier("ruleId", rule_id)?;
    }
    if annotation
        .confidence
        .is_some_and(|confidence| !confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(HcdError::InvalidPatch(
            "annotation confidence must be finite and between 0 and 1".to_string(),
        ));
    }
    Ok(())
}

fn validate_identifier(name: &str, value: &str) -> Result<(), HcdError> {
    if value.trim().is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(HcdError::InvalidPatch(format!(
            "{name} length must be between 1 and {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_node_id(value: &str) -> Result<(), HcdError> {
    if value.len() != 34
        || !value.starts_with("n_")
        || !value[2..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(HcdError::InvalidPatch(format!(
            "nodeId {value:?} must match n_[0-9a-f]{{32}}"
        )));
    }
    Ok(())
}

fn validate_sha256(name: &str, value: &str) -> Result<(), HcdError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(HcdError::InvalidPatch(format!(
            "{name} must be a lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

fn validate_node_style(style: &NodeStylePatch) -> Result<(), HcdError> {
    if style.text_color.is_none() && style.background_color.is_none() && style.border.is_none() {
        return Err(HcdError::InvalidPatch(
            "node.style must set textColor, backgroundColor, or border".to_string(),
        ));
    }
    if let Some(color) = &style.text_color {
        validate_hex_color("textColor", color)?;
    }
    if let Some(color) = &style.background_color {
        validate_hex_color("backgroundColor", color)?;
    }
    if let Some(border) = &style.border {
        validate_hex_color("border.color", &border.color)?;
        if !border.width_pt.is_finite() || !(0.0..=12.0).contains(&border.width_pt) {
            return Err(HcdError::InvalidPatch(
                "border.widthPt must be finite, greater than 0, and at most 12".to_string(),
            ));
        }
        if border.width_pt == 0.0 {
            return Err(HcdError::InvalidPatch(
                "border.widthPt must be greater than 0".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_hex_color(name: &str, color: &str) -> Result<(), HcdError> {
    if color.len() != 7
        || !color.starts_with('#')
        || !color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(HcdError::InvalidPatch(format!(
            "{name} must be a #RRGGBB color"
        )));
    }
    Ok(())
}

fn collect_splices(patch: &PatchBatch) -> Result<BTreeMap<String, Vec<Splice>>, HcdError> {
    let mut grouped: BTreeMap<String, Vec<Splice>> = BTreeMap::new();
    for operation in &patch.operations {
        if let PatchOperation::TextSplice {
            node_id,
            start,
            delete_count,
            insert_text,
            precondition,
        } = operation
        {
            if insert_text.chars().any(is_forbidden_xml_character) {
                return Err(HcdError::InvalidPatch(format!(
                    "insertText for {node_id} contains an invalid XML character"
                )));
            }
            grouped.entry(node_id.clone()).or_default().push(Splice {
                start: *start,
                delete_count: *delete_count,
                insert_text: insert_text.clone(),
                node_hash: precondition.node_hash.clone(),
            });
        }
    }
    Ok(grouped)
}

fn collect_styles(patch: &PatchBatch) -> Result<BTreeMap<String, StyleChange>, HcdError> {
    let mut styles = BTreeMap::new();
    for operation in &patch.operations {
        if let PatchOperation::NodeStyle {
            node_id,
            style,
            precondition,
        } = operation
        {
            if styles
                .insert(
                    node_id.clone(),
                    StyleChange {
                        style: style.clone(),
                        node_hash: precondition.node_hash.clone(),
                    },
                )
                .is_some()
            {
                return Err(HcdError::InvalidPatch(format!(
                    "patch contains more than one node.style for {node_id}"
                )));
            }
        }
    }
    Ok(styles)
}

fn collect_image_changes(patch: &PatchBatch) -> Result<BTreeMap<String, ImageChange>, HcdError> {
    let mut images: BTreeMap<String, ImageChange> = BTreeMap::new();
    for operation in &patch.operations {
        let (node_id, visual_hash) = match operation {
            PatchOperation::ImageReplace {
                node_id,
                precondition,
                ..
            }
            | PatchOperation::ImageGeometry {
                node_id,
                precondition,
                ..
            } => (node_id, &precondition.visual_hash),
            _ => continue,
        };
        let change = images
            .entry(node_id.clone())
            .or_insert_with(|| ImageChange {
                visual_hash: visual_hash.clone(),
                ..ImageChange::default()
            });
        if change.visual_hash != *visual_hash {
            return Err(HcdError::InvalidPatch(format!(
                "image operations for {node_id} use different visualHash preconditions"
            )));
        }
        match operation {
            PatchOperation::ImageReplace { asset_hash, .. } => {
                if change.asset_hash.replace(asset_hash.clone()).is_some() {
                    return Err(HcdError::InvalidPatch(format!(
                        "patch contains more than one image.replace for {node_id}"
                    )));
                }
            }
            PatchOperation::ImageGeometry { geometry, .. } => {
                if change.geometry.replace(geometry.clone()).is_some() {
                    return Err(HcdError::InvalidPatch(format!(
                        "patch contains more than one image.geometry for {node_id}"
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(images)
}

fn splice_text(text: &str, splices: &[Splice]) -> Result<String, HcdError> {
    let mut ordered = splices.to_vec();
    ordered.sort_by_key(|splice| splice.start);
    let char_count = text.chars().count();
    let mut previous_end = 0usize;
    for splice in &ordered {
        let end = splice
            .start
            .checked_add(splice.delete_count)
            .ok_or_else(|| HcdError::InvalidPatch("splice range overflowed usize".to_string()))?;
        if end > char_count {
            return Err(HcdError::InvalidPatch(format!(
                "splice range {}..{} exceeds node length {}",
                splice.start, end, char_count
            )));
        }
        if splice.start < previous_end {
            return Err(HcdError::InvalidPatch(
                "overlapping text.splice operations on one node".to_string(),
            ));
        }
        previous_end = end;
    }

    let mut chars: Vec<char> = text.chars().collect();
    for splice in ordered.into_iter().rev() {
        chars.splice(
            splice.start..splice.start + splice.delete_count,
            splice.insert_text.chars(),
        );
    }
    Ok(chars.into_iter().collect())
}

fn replace_node_text(
    html: &mut String,
    node_id: &str,
    text: &str,
    node_hash: &str,
) -> Result<(), HcdError> {
    let needle = format!("data-hcd-id=\"{}\"", escape_attribute(node_id));
    let attribute = html
        .find(&needle)
        .ok_or_else(|| HcdError::InvalidBundle(format!("node {node_id} missing from HTML")))?;
    let content_start = html[attribute + needle.len()..]
        .find('>')
        .map(|offset| attribute + needle.len() + offset + 1)
        .ok_or_else(|| HcdError::InvalidBundle(format!("node {node_id} has no start tag end")))?;
    let content_end = html[content_start..]
        .find("</span>")
        .map(|offset| content_start + offset)
        .ok_or_else(|| HcdError::InvalidBundle(format!("node {node_id} has no closing span")))?;
    html.replace_range(content_start..content_end, &escape_text(text));
    let tag_start = html[..attribute].rfind('<').unwrap_or(attribute);
    let tag_end = html[attribute..]
        .find('>')
        .map(|offset| attribute + offset)
        .ok_or_else(|| HcdError::InvalidBundle(format!("node {node_id} has no start tag")))?;
    let hash_marker = "data-hcd-node-hash=\"";
    let hash_start = html[tag_start..tag_end]
        .find(hash_marker)
        .map(|offset| tag_start + offset + hash_marker.len())
        .ok_or_else(|| {
            HcdError::InvalidBundle(format!("node {node_id} has no node hash attribute"))
        })?;
    let hash_end = html[hash_start..tag_end]
        .find('"')
        .map(|offset| hash_start + offset)
        .ok_or_else(|| HcdError::InvalidBundle(format!("node {node_id} has invalid node hash")))?;
    html.replace_range(hash_start..hash_end, node_hash);
    Ok(())
}

fn apply_node_style(
    html: &mut String,
    node_id: &str,
    style: &NodeStylePatch,
) -> Result<(), HcdError> {
    let canonical_marker = format!("data-hcd-id=\"{}\"", escape_attribute(node_id));
    let mut text_properties = BTreeMap::new();
    if let Some(color) = &style.text_color {
        text_properties.insert("color".to_string(), color.to_ascii_lowercase());
    }
    update_start_tag_style(html, &canonical_marker, &text_properties, true)?;

    let bbox_marker = format!("data-hcd-text-node=\"{}\"", escape_attribute(node_id));
    let has_pdf_bbox = html.contains(&bbox_marker);
    if let Some(color) = &style.background_color {
        let mut background = BTreeMap::new();
        background.insert("background-color".to_string(), color.to_ascii_lowercase());
        update_start_tag_style(
            html,
            if has_pdf_bbox {
                &bbox_marker
            } else {
                &canonical_marker
            },
            &background,
            true,
        )?;
    }

    if let Some(border) = &style.border {
        let value = format!(
            "{}pt {} {}",
            compact_css_number(border.width_pt),
            border.style.as_css(),
            border.color.to_ascii_lowercase()
        );
        let mut border_properties = BTreeMap::new();
        for side in ["top", "right", "bottom", "left"] {
            border_properties.insert(format!("border-{side}"), value.clone());
        }
        if has_pdf_bbox {
            update_start_tag_style(html, &bbox_marker, &border_properties, true)?;
        } else {
            update_start_tag_style(html, &canonical_marker, &border_properties, true)?;
        }
    }
    Ok(())
}

fn validate_staged_asset(
    bundle: &Bundle,
    asset: &AssetDescriptor,
    expected_hash: &str,
) -> Result<(), HcdError> {
    if asset.hash != expected_hash {
        return Err(HcdError::InvalidBundle(format!(
            "staged asset descriptor hash {} does not match {expected_hash}",
            asset.hash
        )));
    }
    let path = bundle.resolve_href(&asset.href)?;
    let metadata = fs::metadata(&path)?;
    if metadata.len() != asset.byte_length {
        return Err(HcdError::InvalidBundle(format!(
            "staged asset {expected_hash} expected {} bytes, found {}",
            asset.byte_length,
            metadata.len()
        )));
    }
    let actual = crate::hash_file(path)?;
    if actual != expected_hash {
        return Err(HcdError::InvalidBundle(format!(
            "staged asset {expected_hash} contains hash {actual}"
        )));
    }
    Ok(())
}

fn replace_image_asset(
    html: &mut String,
    node_id: &str,
    asset: &AssetDescriptor,
) -> Result<(), HcdError> {
    set_element_attribute(html, node_id, "data-hcd-asset-hash", &asset.hash)?;
    set_element_attribute(html, node_id, "data-hcd-image-asset-patched", "true")?;
    let (_, target_end) = element_start_tag_range(html, node_id)?;
    let target_tag = &html[..=target_end];
    let image_start = if target_tag[target_tag.rfind('<').unwrap_or(0)..].starts_with("<img") {
        target_tag.rfind('<').unwrap_or(0)
    } else {
        html[target_end + 1..]
            .find("<img")
            .map(|offset| target_end + 1 + offset)
            .ok_or_else(|| {
                HcdError::InvalidBundle(format!("image node {node_id} has no img child"))
            })?
    };
    let image_end = html[image_start..]
        .find('>')
        .map(|offset| image_start + offset)
        .ok_or_else(|| {
            HcdError::InvalidBundle(format!("image node {node_id} img is not closed"))
        })?;
    set_attribute_in_range(
        html,
        image_start,
        image_end,
        "src",
        &format!("asset://sha256/{}", asset.hash),
    )?;
    let image_end = html[image_start..]
        .find('>')
        .map(|offset| image_start + offset)
        .ok_or_else(|| {
            HcdError::InvalidBundle(format!("image node {node_id} img is not closed"))
        })?;
    set_attribute_in_range(
        html,
        image_start,
        image_end,
        "data-hcd-asset-href",
        &asset.href,
    )
}

fn replace_image_geometry(
    html: &mut String,
    node_id: &str,
    geometry: &ImageGeometry,
) -> Result<(), HcdError> {
    crate::html::validate_image_geometry(geometry)?;
    set_element_attribute(html, node_id, "data-hcd-image-geometry-patched", "true")?;
    for (name, value) in [
        ("data-hcd-x", canonical_f64(geometry.x)),
        ("data-hcd-y", canonical_f64(geometry.y)),
        ("data-hcd-width", canonical_f64(geometry.width)),
        ("data-hcd-height", canonical_f64(geometry.height)),
    ] {
        set_element_attribute(html, node_id, name, &value)?;
    }
    set_element_attribute(
        html,
        node_id,
        "data-hcd-geometry-unit",
        match geometry.unit {
            ImageGeometryUnit::Emu => "emu",
            ImageGeometryUnit::Pt => "pt",
        },
    )?;
    if geometry.unit == ImageGeometryUnit::Emu {
        for (name, value) in [
            ("data-hcd-x-emu", canonical_f64(geometry.x)),
            ("data-hcd-y-emu", canonical_f64(geometry.y)),
            ("data-hcd-width-emu", canonical_f64(geometry.width)),
            ("data-hcd-height-emu", canonical_f64(geometry.height)),
        ] {
            set_element_attribute(html, node_id, name, &value)?;
        }
    } else {
        set_element_attribute(
            html,
            node_id,
            "data-hcd-bbox",
            &format!(
                "{},{},{},{}",
                canonical_f64(geometry.x),
                canonical_f64(geometry.y),
                canonical_f64(geometry.width),
                canonical_f64(geometry.height)
            ),
        )?;
    }
    let scale = match geometry.unit {
        ImageGeometryUnit::Emu => 96.0 / 914_400.0,
        ImageGeometryUnit::Pt => 1.0,
    };
    let suffix = match geometry.unit {
        ImageGeometryUnit::Emu => "px",
        ImageGeometryUnit::Pt => "pt",
    };
    let mut properties = BTreeMap::new();
    properties.insert("position".to_string(), "absolute".to_string());
    properties.insert(
        "left".to_string(),
        format!("{}{suffix}", canonical_f64(geometry.x * scale)),
    );
    properties.insert(
        "top".to_string(),
        format!("{}{suffix}", canonical_f64(geometry.y * scale)),
    );
    properties.insert(
        "width".to_string(),
        format!("{}{suffix}", canonical_f64(geometry.width * scale)),
    );
    properties.insert(
        "height".to_string(),
        format!("{}{suffix}", canonical_f64(geometry.height * scale)),
    );
    let marker = format!("data-hcd-id=\"{}\"", escape_attribute(node_id));
    update_start_tag_style(html, &marker, &properties, false)
}

fn set_element_attribute(
    html: &mut String,
    node_id: &str,
    name: &str,
    value: &str,
) -> Result<(), HcdError> {
    let (start, end) = element_start_tag_range(html, node_id)?;
    set_attribute_in_range(html, start, end, name, value)
}

fn element_start_tag_range(html: &str, node_id: &str) -> Result<(usize, usize), HcdError> {
    let marker = format!("data-hcd-id=\"{}\"", escape_attribute(node_id));
    let offset = html
        .find(&marker)
        .ok_or_else(|| HcdError::InvalidBundle(format!("image node {node_id} is missing")))?;
    let start = html[..offset]
        .rfind('<')
        .ok_or_else(|| HcdError::InvalidBundle(format!("image node {node_id} has no start tag")))?;
    let end = html[offset..]
        .find('>')
        .map(|relative| offset + relative)
        .ok_or_else(|| HcdError::InvalidBundle(format!("image node {node_id} is not closed")))?;
    Ok((start, end))
}

fn set_attribute_in_range(
    html: &mut String,
    tag_start: usize,
    tag_end: usize,
    name: &str,
    value: &str,
) -> Result<(), HcdError> {
    let tag = &html[tag_start..=tag_end];
    let marker = format!(" {name}=\"");
    let escaped = escape_attribute(value);
    let mut replacement = tag.to_string();
    if let Some(start) = replacement.find(&marker) {
        let value_start = start + marker.len();
        let value_end = replacement[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .ok_or_else(|| HcdError::InvalidBundle(format!("attribute {name} is not closed")))?;
        replacement.replace_range(value_start..value_end, &escaped);
    } else {
        let insertion = replacement
            .rfind("/>")
            .unwrap_or_else(|| replacement.len().saturating_sub(1));
        replacement.insert_str(insertion, &format!(" {name}=\"{escaped}\""));
    }
    html.replace_range(tag_start..=tag_end, &replacement);
    Ok(())
}

fn canonical_f64(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    format!("{value:.6}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn update_start_tag_style(
    html: &mut String,
    marker: &str,
    updates: &BTreeMap<String, String>,
    mark_patched: bool,
) -> Result<(), HcdError> {
    let marker_offset = html.find(marker).ok_or_else(|| {
        HcdError::InvalidBundle(format!(
            "style target containing {marker} is missing from HTML"
        ))
    })?;
    let tag_start = html[..marker_offset].rfind('<').ok_or_else(|| {
        HcdError::InvalidBundle(format!("style target containing {marker} has no start tag"))
    })?;
    let tag_end = html[marker_offset..]
        .find('>')
        .map(|offset| marker_offset + offset)
        .ok_or_else(|| {
            HcdError::InvalidBundle(format!("style target containing {marker} is not closed"))
        })?;
    let tag = &html[tag_start..=tag_end];
    let mut declarations = BTreeMap::new();
    let style_marker = " style=\"";
    let style_range = tag.find(style_marker).map(|start| {
        let value_start = start + style_marker.len();
        let value_end = tag[value_start..]
            .find('"')
            .map(|offset| value_start + offset)
            .unwrap_or(value_start);
        (value_start, value_end)
    });
    if let Some((value_start, value_end)) = style_range {
        for declaration in tag[value_start..value_end].split(';') {
            let Some((property, value)) = declaration.split_once(':') else {
                continue;
            };
            declarations.insert(
                property.trim().to_ascii_lowercase(),
                value.trim().to_string(),
            );
        }
    }
    declarations.extend(updates.clone());
    let style_value = declarations
        .iter()
        .map(|(property, value)| format!("{property}:{value}"))
        .collect::<Vec<_>>()
        .join(";");
    crate::html::validate_inline_style(&style_value)?;

    let mut replacement = tag.to_string();
    if let Some((value_start, value_end)) = style_range {
        replacement.replace_range(value_start..value_end, &style_value);
    } else if !style_value.is_empty() {
        replacement.insert_str(replacement.len() - 1, &format!(" style=\"{style_value}\""));
    }
    if mark_patched && !replacement.contains(" data-hcd-style-patched=\"true\"") {
        replacement.insert_str(replacement.len() - 1, " data-hcd-style-patched=\"true\"");
    }
    html.replace_range(tag_start..=tag_end, &replacement);
    Ok(())
}

fn compact_css_number(value: f32) -> String {
    let formatted = format!("{value:.3}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn apply_annotations(
    bundle: &Bundle,
    manifest: &crate::HcdManifest,
    patch: &PatchBatch,
) -> Result<(Option<String>, String), HcdError> {
    let has_annotation_ops = patch.operations.iter().any(|operation| {
        matches!(
            operation,
            PatchOperation::AnnotationUpsert { .. } | PatchOperation::AnnotationRemove { .. }
        )
    });
    if !has_annotation_ops {
        return Ok((
            manifest.annotation_href.clone(),
            manifest.annotation_root_hash.clone(),
        ));
    }

    let mut set = if let Some(href) = &manifest.annotation_href {
        read_json_bounded(
            &bundle.resolve_href(href)?,
            MAX_CONTROL_PART_BYTES,
            "annotation set",
        )?
    } else {
        AnnotationSet {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            annotations: Vec::new(),
        }
    };
    for operation in &patch.operations {
        match operation {
            PatchOperation::AnnotationUpsert { annotation } => {
                if let Some(existing) = set
                    .annotations
                    .iter_mut()
                    .find(|existing| existing.annotation_id == annotation.annotation_id)
                {
                    *existing = annotation.clone();
                } else {
                    set.annotations.push(annotation.clone());
                }
            }
            PatchOperation::AnnotationRemove { annotation_id } => {
                set.annotations
                    .retain(|annotation| annotation.annotation_id != *annotation_id);
            }
            PatchOperation::TextSplice { .. }
            | PatchOperation::NodeStyle { .. }
            | PatchOperation::ImageReplace { .. }
            | PatchOperation::ImageGeometry { .. } => {}
        }
    }
    set.annotations
        .sort_by(|left, right| left.annotation_id.cmp(&right.annotation_id));
    let encoded = serde_json::to_vec(&set)?;
    if encoded.len() as u64 > MAX_CONTROL_PART_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "annotation set is {} bytes; maximum is {MAX_CONTROL_PART_BYTES}",
            encoded.len()
        )));
    }
    let (href, hash) = bundle.write_json_object("annotations", &set)?;
    Ok((Some(href), hash))
}

fn validate_annotation_ranges(
    patch: &PatchBatch,
    found_nodes: &HashMap<String, usize>,
) -> Result<(), HcdError> {
    for operation in &patch.operations {
        if let PatchOperation::AnnotationUpsert { annotation } = operation {
            let length = found_nodes
                .get(&annotation.node_id)
                .ok_or_else(|| HcdError::NodeNotFound(annotation.node_id.clone()))?;
            if annotation.start > annotation.end || annotation.end > *length {
                return Err(HcdError::InvalidPatch(format!(
                    "annotation {} range {}..{} exceeds node length {}",
                    annotation.annotation_id, annotation.start, annotation.end, length
                )));
            }
        }
    }
    Ok(())
}

fn find_idempotent_result(
    bundle: &Bundle,
    manifest: &crate::HcdManifest,
    patch_id: &str,
    patch_hash: &str,
) -> Result<Option<ApplyResult>, HcdError> {
    for revision in 1..=manifest.revision {
        let record = bundle.revision(revision)?;
        if record.patch_id.as_deref() == Some(patch_id) {
            if record.patch_hash.as_deref() != Some(patch_hash) {
                return Err(HcdError::InvalidPatch(format!(
                    "patchId {patch_id} was already used with a different payload"
                )));
            }
            return Ok(Some(ApplyResult {
                document_id: record.document_id,
                patch_id: patch_id.to_string(),
                base_revision: record.patch_base_revision.unwrap_or(0),
                revision: record.revision,
                root_hash: record.root_hash,
                annotation_root_hash: record.annotation_root_hash,
                dirty_node_ids: record.dirty_node_ids,
                dirty_chunk_ids: record.dirty_chunk_ids,
                dirty_source_parts: record.dirty_source_parts,
                warnings: Vec::new(),
                idempotent_replay: true,
            }));
        }
    }
    Ok(None)
}

fn parse_cursor(cursor: Option<&str>) -> Result<(usize, usize), HcdError> {
    let Some(cursor) = cursor else {
        return Ok((0, 0));
    };
    let (sequence, offset) = cursor
        .split_once(':')
        .ok_or_else(|| HcdError::InvalidPatch("invalid extract cursor".to_string()))?;
    Ok((
        sequence
            .parse()
            .map_err(|_| HcdError::InvalidPatch("invalid cursor sequence".to_string()))?,
        offset
            .parse()
            .map_err(|_| HcdError::InvalidPatch("invalid cursor offset".to_string()))?,
    ))
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attribute(text: &str) -> String {
    escape_text(text)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn is_forbidden_xml_character(ch: char) -> bool {
    matches!(ch as u32, 0x0..=0x8 | 0xB | 0xC | 0xE..=0x1F | 0xFFFE | 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_scalar_splice_handles_emoji() {
        let result = splice_text(
            "甲😀乙",
            &[Splice {
                start: 1,
                delete_count: 1,
                insert_text: "**".to_string(),
                node_hash: "unused".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(result, "甲**乙");
    }

    #[test]
    fn overlapping_splices_are_rejected() {
        let error = splice_text(
            "abcdef",
            &[
                Splice {
                    start: 1,
                    delete_count: 3,
                    insert_text: "x".to_string(),
                    node_hash: "unused".to_string(),
                },
                Splice {
                    start: 2,
                    delete_count: 1,
                    insert_text: "y".to_string(),
                    node_hash: "unused".to_string(),
                },
            ],
        )
        .unwrap_err();
        assert!(error.to_string().contains("overlapping"));
    }

    #[test]
    fn generated_span_text_is_replaced_and_escaped() {
        let mut html = "<p><span data-hcd-id=\"n_1\" data-hcd-node-hash=\"oldhash\">old</span></p>"
            .to_string();
        replace_node_text(&mut html, "n_1", "a < b", "newhash").unwrap();
        assert_eq!(
            html,
            "<p><span data-hcd-id=\"n_1\" data-hcd-node-hash=\"newhash\">a &lt; b</span></p>"
        );
    }

    #[test]
    fn node_style_targets_text_and_pdf_bbox_without_changing_node_hash() {
        let node_id = "n_00000000000000000000000000000000";
        let mut html = format!(
            "<p class=\"hcd-pdf-text\" data-hcd-text-node=\"{node_id}\" style=\"position:absolute;left:10pt\"><span data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{}\">old</span></p>",
            "a".repeat(64)
        );
        apply_node_style(
            &mut html,
            node_id,
            &crate::NodeStylePatch {
                text_color: Some("#D70015".to_string()),
                background_color: Some("#FFF2A8".to_string()),
                border: Some(crate::NodeBorder {
                    color: "#0A84FF".to_string(),
                    width_pt: 2.0,
                    style: crate::NodeBorderStyle::Dashed,
                }),
            },
        )
        .unwrap();

        assert!(html.contains("color:#d70015"));
        assert!(html.contains("background-color:#fff2a8"));
        assert!(html.contains("border-top:2pt dashed #0a84ff"));
        assert!(html.contains("data-hcd-style-patched=\"true\""));
        assert!(html.contains(&format!("data-hcd-node-hash=\"{}\"", "a".repeat(64))));
        assert_eq!(extract_html_text_nodes(&html).unwrap()[node_id], "old");
    }

    #[test]
    fn node_style_validation_rejects_empty_and_unsafe_values() {
        let empty = crate::NodeStylePatch {
            text_color: None,
            background_color: None,
            border: None,
        };
        assert!(validate_node_style(&empty).is_err());

        let unsafe_color = crate::NodeStylePatch {
            text_color: Some("red;url(x)".to_string()),
            background_color: None,
            border: None,
        };
        assert!(validate_node_style(&unsafe_color).is_err());
    }

    #[test]
    fn patch_revisions_every_bounded_index_page() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = crate::BundleWriter::create(&bundle_path).unwrap();
        writer.write_styles("").unwrap();
        for index in 0..=crate::INDEX_PAGE_SIZE {
            let node_id = format!("n_{index:032x}");
            let chunk_id = format!("c_{index:032x}");
            let text = format!("value-{index}");
            let node_hash = hash_bytes(text.as_bytes());
            let html = format!(
                "<p><span data-hcd-id=\"{node_id}\" data-hcd-node-hash=\"{node_hash}\">{text}</span></p>"
            );
            writer
                .write_chunk(
                    chunk_id.clone(),
                    "body".to_string(),
                    html,
                    crate::ChunkSourceMap {
                        schema_version: HCD_SCHEMA_VERSION.to_string(),
                        chunk_id,
                        entries: vec![crate::NodeMapEntry {
                            node_id,
                            node_hash,
                            source: crate::SourceAnchor {
                                part: "text/source.txt".to_string(),
                                text_ordinal: index as u64 + 1,
                                paragraph_id: None,
                                text_id: None,
                                node_kind: "line".to_string(),
                                editable: true,
                            },
                        }],
                    },
                    1,
                    false,
                )
                .unwrap();
        }
        writer
            .finish(crate::HcdManifest {
                schema_version: HCD_SCHEMA_VERSION.to_string(),
                document_id: "multi-index".to_string(),
                profile: "semantic-flow".to_string(),
                revision: 0,
                source: crate::SourceDescriptor {
                    format: "txt".to_string(),
                    sha256: "0".repeat(64),
                    size_bytes: 1,
                },
                root_hash: String::new(),
                annotation_root_hash: String::new(),
                annotation_href: None,
                index_prefix: String::new(),
                index_page_count: 0,
                chunk_count: 0,
                styles_href: String::new(),
                capabilities: crate::HcdCapabilities::default(),
                fidelity: None,
                state: "IMPORTING".to_string(),
                warnings: Vec::new(),
            })
            .unwrap();
        let bundle = crate::Bundle::open(&bundle_path).unwrap();
        let first = get_text_node(&bundle, "n_00000000000000000000000000000000").unwrap();
        apply_patch(
            &bundle,
            &crate::PatchBatch {
                schema_version: HCD_PATCH_SCHEMA_VERSION.to_string(),
                document_id: "multi-index".to_string(),
                patch_id: "multi-index-1".to_string(),
                base_revision: 0,
                actor: BTreeMap::new(),
                operations: vec![crate::PatchOperation::TextSplice {
                    node_id: first.node.node_id,
                    start: 0,
                    delete_count: first.node.text.chars().count(),
                    insert_text: "changed".to_string(),
                    precondition: crate::NodePrecondition {
                        node_hash: first.node.node_hash,
                    },
                }],
                metadata: BTreeMap::new(),
            },
            0,
        )
        .unwrap();
        let manifest = bundle.manifest().unwrap();
        assert_eq!(manifest.index_page_count, 2);
        assert_eq!(bundle.read_index_page(&manifest, 0).unwrap().revision, 1);
        assert_eq!(bundle.read_index_page(&manifest, 1).unwrap().revision, 1);
        let mut rendered = Vec::new();
        crate::render_standalone_html(
            &bundle,
            &crate::HtmlPresentationOptions {
                revision: Some(1),
                ..crate::HtmlPresentationOptions::default()
            },
            &mut rendered,
        )
        .unwrap();
        assert!(String::from_utf8(rendered).unwrap().contains("changed"));
    }

    #[test]
    fn runtime_patch_validation_enforces_frozen_ids_hashes_and_annotations() {
        assert!(validate_node_id("n_00000000000000000000000000000000").is_ok());
        assert!(validate_node_id("n_NOT_HEX").is_err());
        assert!(validate_sha256("nodeHash", &"a".repeat(64)).is_ok());
        assert!(validate_sha256("nodeHash", &"A".repeat(64)).is_err());

        let mut annotation = crate::Annotation {
            annotation_id: "a-1".to_string(),
            node_id: "n_00000000000000000000000000000000".to_string(),
            start: 0,
            end: 1,
            kind: "mask".to_string(),
            rule_id: None,
            confidence: Some(0.5),
            ignored: false,
        };
        assert!(validate_annotation(&annotation).is_ok());
        annotation.confidence = Some(1.5);
        assert!(validate_annotation(&annotation).is_err());
        annotation.confidence = Some(0.5);
        annotation.annotation_id = "x".repeat(MAX_IDENTIFIER_BYTES + 1);
        assert!(validate_annotation(&annotation).is_err());
    }
}
