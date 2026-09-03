use crate::bundle::{
    finalize_root_hash, hash_descriptor, read_json_bounded, read_text_bounded, Bundle,
};
use crate::hash::{hash_file, node_bloom_might_contain};
use crate::{
    AnnotationSet, HcdError, RevisionRecord, ValidationIssue, ValidationReport, HCD_SCHEMA_VERSION,
    MAX_CHUNK_BYTES, MAX_CONTROL_PART_BYTES, MAX_REVISION,
};
use quick_xml::events::Event;
use quick_xml::Reader;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;

const NODE_ID_RUN_CAPACITY: usize = 262_144;
const MAX_HTML_DEPTH: usize = 256;
const MAX_HTML_ELEMENTS: usize = 100_000;
const MAX_HCD_TABLE_ROWS: usize = 1_048_576;
const MAX_HCD_TABLE_COLUMNS: usize = 16_384;
const MAX_HCD_TABLE_FRAGMENTS: usize = 1_000_000;

#[derive(Debug)]
struct HcdTableFragment {
    node_id: String,
    ordinal: usize,
    row_start: usize,
    row_end: usize,
    fragment_row_count: usize,
    column_count: usize,
    final_fragment: bool,
    total_row_count: Option<usize>,
    actual_row_count: usize,
}

struct ActiveHcdTable {
    node_id: String,
    next_fragment: usize,
    next_row: usize,
    column_count: usize,
}

#[derive(Default)]
struct HcdTableFragmentTracker {
    active: Option<ActiveHcdTable>,
}

impl HcdTableFragmentTracker {
    fn observe(&mut self, fragment: &HcdTableFragment) -> Result<bool, HcdError> {
        let first_fragment = fragment.ordinal == 0;
        if first_fragment {
            if let Some(active) = &self.active {
                return Err(HcdError::InvalidBundle(format!(
                    "HCD table {} started before table {} reached its final fragment",
                    fragment.node_id, active.node_id
                )));
            }
            if fragment.row_start != usize::from(fragment.fragment_row_count > 0) {
                return Err(HcdError::InvalidBundle(format!(
                    "HCD table {} first fragment must start at row 1 (or 0 when empty)",
                    fragment.node_id
                )));
            }
            self.active = Some(ActiveHcdTable {
                node_id: fragment.node_id.clone(),
                next_fragment: 0,
                next_row: fragment.row_start,
                column_count: fragment.column_count,
            });
        }
        let active = self.active.as_mut().ok_or_else(|| {
            HcdError::InvalidBundle(format!(
                "HCD table {} continuation has no first fragment",
                fragment.node_id
            ))
        })?;
        if active.node_id != fragment.node_id
            || active.next_fragment != fragment.ordinal
            || active.next_row != fragment.row_start
            || active.column_count != fragment.column_count
        {
            return Err(HcdError::InvalidBundle(format!(
                "HCD table {} fragment {} is missing, duplicated, out of order, or inconsistent",
                fragment.node_id, fragment.ordinal
            )));
        }
        active.next_fragment += 1;
        active.next_row = fragment.row_end.saturating_add(1);
        if fragment.final_fragment {
            let expected_rows = fragment.total_row_count.unwrap_or(0);
            if expected_rows != fragment.row_end {
                return Err(HcdError::InvalidBundle(format!(
                    "HCD table {} final row count {expected_rows} does not match row end {}",
                    fragment.node_id, fragment.row_end
                )));
            }
            self.active = None;
        }
        Ok(first_fragment)
    }

    fn finish(self) -> Result<(), HcdError> {
        if let Some(active) = self.active {
            return Err(HcdError::InvalidBundle(format!(
                "HCD table {} ended before its final fragment",
                active.node_id
            )));
        }
        Ok(())
    }
}

pub fn validate_bundle(bundle: &Bundle) -> Result<ValidationReport, HcdError> {
    let manifest = match bundle.manifest() {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(ValidationReport {
                valid: false,
                document_id: None,
                revision: None,
                issues: vec![issue(
                    "MANIFEST_INVALID",
                    error.to_string(),
                    "manifest.json",
                )],
            });
        }
    };
    let mut issues = Vec::new();
    if manifest.schema_version != HCD_SCHEMA_VERSION {
        issues.push(issue(
            "SCHEMA_VERSION",
            format!("unsupported schema version {}", manifest.schema_version),
            "manifest.json",
        ));
    }
    if !matches!(
        manifest.profile.as_str(),
        "semantic-flow" | "grid" | "slide-canvas" | "fixed-layout"
    ) {
        issues.push(issue(
            "PROFILE_UNSUPPORTED",
            format!("unsupported profile {}", manifest.profile),
            "manifest.json",
        ));
    }
    if manifest.state != "COMPLETE" {
        issues.push(issue(
            "BUNDLE_INCOMPLETE",
            format!("bundle state is {}", manifest.state),
            "manifest.json",
        ));
    }
    validate_manifest_fields(&manifest, &mut issues);
    validate_revision_chain(bundle, &manifest, &mut issues);

    let styles_path = match bundle.resolve_href(&manifest.styles_href) {
        Ok(path) => path,
        Err(error) => {
            issues.push(issue("UNSAFE_HREF", error.to_string(), "manifest.json"));
            bundle.root().join("__invalid_styles__")
        }
    };
    match read_text_bounded(&styles_path, MAX_CONTROL_PART_BYTES, "HCD stylesheet") {
        Ok(styles) => {
            if let Err(error) = crate::html::validate_css_text(&styles) {
                issues.push(issue(
                    "UNSAFE_CSS",
                    error.to_string(),
                    &manifest.styles_href,
                ));
            }
        }
        Err(error) => issues.push(issue(
            "STYLES_MISSING",
            error.to_string(),
            &manifest.styles_href,
        )),
    }
    let asset_hashes = validate_assets(bundle, &mut issues);
    let annotations = load_annotations(bundle, &manifest, &mut issues);
    let mut annotation_targets: HashMap<[u8; 16], Option<usize>> = annotations
        .as_ref()
        .into_iter()
        .flat_map(|set| set.annotations.iter())
        .filter_map(|annotation| decode_node_id(&annotation.node_id))
        .map(|node_id| (node_id, None))
        .collect();

    let mut root_hasher = Sha256::new();
    let mut expected_sequence = 0usize;
    let mut node_ids = NodeIdTracker::new()?;
    let mut table_fragments = HcdTableFragmentTracker::default();
    let mut textual_range_end = 0u64;
    for page_number in 0..manifest.index_page_count {
        let page = match bundle.read_index_page(&manifest, page_number) {
            Ok(page) => page,
            Err(error) => {
                issues.push(issue(
                    "INDEX_PAGE_INVALID",
                    error.to_string(),
                    &format!("{}/{page_number:06}.json", manifest.index_prefix),
                ));
                continue;
            }
        };
        if page.schema_version != HCD_SCHEMA_VERSION {
            issues.push(issue(
                "INDEX_SCHEMA_VERSION",
                format!("unsupported index schema {}", page.schema_version),
                &format!("{}/{page_number:06}.json", manifest.index_prefix),
            ));
        }
        if page.page != page_number || page.revision > manifest.revision {
            issues.push(issue(
                "INDEX_METADATA_MISMATCH",
                format!(
                    "index page reports page {} revision {}",
                    page.page, page.revision
                ),
                &format!("{}/{page_number:06}.json", manifest.index_prefix),
            ));
        }
        if page.chunks.len() > crate::INDEX_PAGE_SIZE {
            issues.push(issue(
                "INDEX_PAGE_TOO_LARGE",
                format!("index page contains {} chunks", page.chunks.len()),
                &format!("{}/{page_number:06}.json", manifest.index_prefix),
            ));
        }
        for descriptor in page.chunks {
            validate_descriptor(&descriptor, &mut issues);
            if descriptor.sequence != expected_sequence {
                issues.push(issue(
                    "CHUNK_SEQUENCE",
                    format!(
                        "expected chunk sequence {expected_sequence}, found {}",
                        descriptor.sequence
                    ),
                    &descriptor.html_href,
                ));
                expected_sequence = descriptor.sequence;
            }
            expected_sequence += 1;
            hash_descriptor(&mut root_hasher, &descriptor);

            let html_path = match bundle.resolve_href(&descriptor.html_href) {
                Ok(path) => path,
                Err(error) => {
                    issues.push(issue(
                        "UNSAFE_HREF",
                        error.to_string(),
                        &descriptor.html_href,
                    ));
                    continue;
                }
            };
            match fs::metadata(&html_path) {
                Ok(metadata) => {
                    if metadata.len() as usize > MAX_CHUNK_BYTES {
                        issues.push(issue(
                            "CHUNK_TOO_LARGE",
                            format!("chunk is {} bytes", metadata.len()),
                            &descriptor.html_href,
                        ));
                    }
                    if metadata.len() != descriptor.byte_length {
                        issues.push(issue(
                            "CHUNK_SIZE_MISMATCH",
                            format!(
                                "manifest says {}, file is {}",
                                descriptor.byte_length,
                                metadata.len()
                            ),
                            &descriptor.html_href,
                        ));
                    }
                }
                Err(error) => {
                    issues.push(issue(
                        "CHUNK_MISSING",
                        error.to_string(),
                        &descriptor.html_href,
                    ));
                    continue;
                }
            }
            match hash_file(&html_path) {
                Ok(hash) if hash != descriptor.html_hash => issues.push(issue(
                    "CHUNK_HASH_MISMATCH",
                    format!("expected {}, actual {hash}", descriptor.html_hash),
                    &descriptor.html_href,
                )),
                Ok(_) => {}
                Err(error) => issues.push(issue(
                    "CHUNK_READ_ERROR",
                    error.to_string(),
                    &descriptor.html_href,
                )),
            }
            match validate_html_fragment(&html_path, &asset_hashes) {
                Ok(fragments) => {
                    for fragment in fragments {
                        match table_fragments.observe(&fragment) {
                            Ok(true) => {
                                if let Some(node_id) = decode_node_id(&fragment.node_id) {
                                    node_ids.push(node_id)?;
                                }
                            }
                            Ok(false) => {}
                            Err(error) => issues.push(issue(
                                "HCD_TABLE_FRAGMENT_SEQUENCE_INVALID",
                                error.to_string(),
                                &descriptor.html_href,
                            )),
                        }
                    }
                }
                Err(error) => issues.push(issue(
                    "HTML_INVALID",
                    error.to_string(),
                    &descriptor.html_href,
                )),
            }
            let canonical_nodes = match bundle
                .read_chunk(&descriptor)
                .and_then(|html| crate::extract_html_text_nodes(&html))
            {
                Ok(nodes) => nodes,
                Err(error) => {
                    issues.push(issue(
                        "CANONICAL_TEXT_INVALID",
                        error.to_string(),
                        &descriptor.html_href,
                    ));
                    Default::default()
                }
            };
            let actual_text_chars: usize = canonical_nodes
                .values()
                .map(|text| text.chars().count())
                .sum();
            if actual_text_chars != descriptor.text_chars {
                issues.push(issue(
                    "TEXT_LENGTH_MISMATCH",
                    format!(
                        "index says {} Unicode scalars, HTML contains {actual_text_chars}",
                        descriptor.text_chars
                    ),
                    &descriptor.html_href,
                ));
            }

            let map_path = match bundle.resolve_href(&descriptor.map_href) {
                Ok(path) => path,
                Err(error) => {
                    issues.push(issue(
                        "UNSAFE_HREF",
                        error.to_string(),
                        &descriptor.map_href,
                    ));
                    continue;
                }
            };
            match hash_file(&map_path) {
                Ok(hash) if hash != descriptor.map_hash => issues.push(issue(
                    "MAP_HASH_MISMATCH",
                    format!("expected {}, actual {hash}", descriptor.map_hash),
                    &descriptor.map_href,
                )),
                Ok(_) => {}
                Err(error) => {
                    issues.push(issue(
                        "MAP_READ_ERROR",
                        error.to_string(),
                        &descriptor.map_href,
                    ));
                    continue;
                }
            }
            match bundle.read_map(&descriptor) {
                Ok(source_map) => {
                    if source_map.schema_version != HCD_SCHEMA_VERSION {
                        issues.push(issue(
                            "MAP_SCHEMA_VERSION",
                            format!("unsupported map schema {}", source_map.schema_version),
                            &descriptor.map_href,
                        ));
                    }
                    if source_map.chunk_id != descriptor.chunk_id {
                        issues.push(issue(
                            "MAP_CHUNK_MISMATCH",
                            format!(
                                "map chunk {} does not match {}",
                                source_map.chunk_id, descriptor.chunk_id
                            ),
                            &descriptor.map_href,
                        ));
                    }
                    if source_map.entries.len() != descriptor.node_count {
                        issues.push(issue(
                            "MAP_NODE_COUNT",
                            format!(
                                "manifest says {}, map contains {}",
                                descriptor.node_count,
                                source_map.entries.len()
                            ),
                            &descriptor.map_href,
                        ));
                    }
                    if canonical_nodes.len() != source_map.entries.len() {
                        issues.push(issue(
                            "HTML_MAP_NODE_COUNT",
                            format!(
                                "HTML contains {} canonical nodes, map contains {}",
                                canonical_nodes.len(),
                                source_map.entries.len()
                            ),
                            &descriptor.html_href,
                        ));
                    }
                    let expected_first = source_map
                        .entries
                        .first()
                        .map(|entry| entry.node_id.as_str());
                    let expected_last = source_map
                        .entries
                        .last()
                        .map(|entry| entry.node_id.as_str());
                    if descriptor.first_node_id.as_deref() != expected_first
                        || descriptor.last_node_id.as_deref() != expected_last
                    {
                        issues.push(issue(
                            "CHUNK_NODE_BOUNDS_MISMATCH",
                            "firstNodeId/lastNodeId do not match the source map".to_string(),
                            &descriptor.map_href,
                        ));
                    }
                    for entry in source_map.entries {
                        validate_map_entry(
                            &entry,
                            &descriptor.map_href,
                            &manifest.source,
                            &mut textual_range_end,
                            &mut issues,
                        );
                        if let Some(node_id) = decode_node_id(&entry.node_id) {
                            node_ids.push(node_id)?;
                            if let (Some(length), Some(target)) = (
                                canonical_nodes
                                    .get(&entry.node_id)
                                    .map(|text| text.chars().count()),
                                annotation_targets.get_mut(&node_id),
                            ) {
                                *target = Some(length);
                            }
                        } else {
                            issues.push(issue(
                                "NODE_ID_INVALID",
                                format!("invalid canonical node id {}", entry.node_id),
                                &descriptor.map_href,
                            ));
                        }
                        if !node_bloom_might_contain(&descriptor.node_bloom, &entry.node_id) {
                            issues.push(issue(
                                "NODE_BLOOM_INVALID",
                                format!("bloom omits node {}", entry.node_id),
                                &descriptor.map_href,
                            ));
                        }
                        match canonical_nodes.get(&entry.node_id) {
                            Some(text) => {
                                let actual_hash = crate::hash_bytes(text.as_bytes());
                                if actual_hash != entry.node_hash {
                                    issues.push(issue(
                                        "NODE_HASH_MISMATCH",
                                        format!(
                                            "node {} expected {}, actual {actual_hash}",
                                            entry.node_id, entry.node_hash
                                        ),
                                        &descriptor.map_href,
                                    ));
                                }
                            }
                            None => issues.push(issue(
                                "MAP_NODE_MISSING_FROM_HTML",
                                format!("mapped node {} is absent from HTML", entry.node_id),
                                &descriptor.map_href,
                            )),
                        }
                    }
                }
                Err(error) => issues.push(issue(
                    "MAP_INVALID",
                    error.to_string(),
                    &descriptor.map_href,
                )),
            }
        }
    }
    if expected_sequence != manifest.chunk_count {
        issues.push(issue(
            "CHUNK_COUNT_MISMATCH",
            format!(
                "manifest says {}, indexes contain {expected_sequence}",
                manifest.chunk_count
            ),
            "manifest.json",
        ));
    }
    if let Err(error) = table_fragments.finish() {
        issues.push(issue(
            "HCD_TABLE_FRAGMENT_SEQUENCE_INVALID",
            error.to_string(),
            "chunks/sha256",
        ));
    }
    if let Some(duplicate) = node_ids.finish()? {
        issues.push(issue(
            "DUPLICATE_NODE_ID",
            format!("duplicate node id {}", encode_node_id(&duplicate)),
            "maps/sha256",
        ));
    }
    match finalize_root_hash(bundle, root_hasher) {
        Ok(actual_root) if actual_root != manifest.root_hash => issues.push(issue(
            "ROOT_HASH_MISMATCH",
            format!("expected {}, actual {actual_root}", manifest.root_hash),
            "manifest.json",
        )),
        Ok(_) => {}
        Err(error) => issues.push(issue(
            "ROOT_HASH_READ_ERROR",
            error.to_string(),
            "manifest.json",
        )),
    }

    if let Some(annotations) = annotations {
        validate_annotation_ranges(&annotations, &annotation_targets, &mut issues);
    }

    Ok(ValidationReport {
        valid: issues.is_empty(),
        document_id: Some(manifest.document_id),
        revision: Some(manifest.revision),
        issues,
    })
}

fn validate_manifest_fields(manifest: &crate::HcdManifest, issues: &mut Vec<ValidationIssue>) {
    if manifest.document_id.trim().is_empty() || manifest.document_id.len() > 256 {
        issues.push(issue(
            "DOCUMENT_ID_INVALID",
            "documentId must contain between 1 and 256 bytes".to_string(),
            "manifest.json",
        ));
    }
    if !valid_sha256(&manifest.source.sha256)
        || !valid_sha256(&manifest.root_hash)
        || !valid_sha256(&manifest.annotation_root_hash)
    {
        issues.push(issue(
            "MANIFEST_HASH_INVALID",
            "source/root/annotation hashes must be lowercase SHA-256 digests".to_string(),
            "manifest.json",
        ));
    }
    if !matches!(
        manifest.source.format.as_str(),
        "docx" | "xlsx" | "pptx" | "pdf" | "html" | "md" | "txt"
    ) {
        issues.push(issue(
            "SOURCE_FORMAT_INVALID",
            format!("unsupported source format {}", manifest.source.format),
            "manifest.json",
        ));
    }
    if manifest.source.size_bytes > 256 * 1024 * 1024 {
        issues.push(issue(
            "SOURCE_SIZE_INVALID",
            format!("source size {} exceeds 256 MiB", manifest.source.size_bytes),
            "manifest.json",
        ));
    }
    if manifest.revision > MAX_REVISION {
        issues.push(issue(
            "REVISION_LIMIT_EXCEEDED",
            format!(
                "manifest revision {} exceeds the maximum {MAX_REVISION}",
                manifest.revision
            ),
            "manifest.json",
        ));
    }
    if manifest.styles_href != "styles.css" {
        issues.push(issue(
            "STYLES_HREF_INVALID",
            format!(
                "stylesHref must be styles.css, found {}",
                manifest.styles_href
            ),
            "manifest.json",
        ));
    }
    if !valid_index_prefix(&manifest.index_prefix) {
        issues.push(issue(
            "INDEX_PREFIX_INVALID",
            format!("invalid indexPrefix {}", manifest.index_prefix),
            "manifest.json",
        ));
    }
    let expected_pages = manifest.chunk_count.div_ceil(crate::INDEX_PAGE_SIZE);
    if manifest.index_page_count != expected_pages {
        issues.push(issue(
            "INDEX_PAGE_COUNT_MISMATCH",
            format!(
                "{} chunks require {expected_pages} index pages, manifest says {}",
                manifest.chunk_count, manifest.index_page_count
            ),
            "manifest.json",
        ));
    }
}

fn validate_revision_chain(
    bundle: &Bundle,
    manifest: &crate::HcdManifest,
    issues: &mut Vec<ValidationIssue>,
) {
    if manifest.revision > MAX_REVISION {
        return;
    }
    let mut previous: Option<RevisionRecord> = None;
    let mut patch_ids = HashSet::new();
    for revision in 0..=manifest.revision {
        let path = format!("revisions/{revision:020}.json");
        let record = match bundle.revision(revision) {
            Ok(record) => record,
            Err(error) => {
                issues.push(issue("REVISION_INVALID", error.to_string(), &path));
                break;
            }
        };
        if record.schema_version != HCD_SCHEMA_VERSION {
            issues.push(issue(
                "REVISION_SCHEMA_VERSION",
                format!("unsupported revision schema {}", record.schema_version),
                &path,
            ));
        }
        if record.document_id != manifest.document_id {
            issues.push(issue(
                "REVISION_DOCUMENT_MISMATCH",
                "revision documentId does not match the manifest".to_string(),
                &path,
            ));
        }
        if record.revision != revision {
            issues.push(issue(
                "REVISION_NUMBER_MISMATCH",
                format!(
                    "revision file {revision} contains revision {}",
                    record.revision
                ),
                &path,
            ));
        }
        let expected_parent = revision.checked_sub(1);
        if record.parent_revision != expected_parent {
            issues.push(issue(
                "REVISION_PARENT_MISMATCH",
                format!(
                    "revision {revision} parent is {:?}, expected {expected_parent:?}",
                    record.parent_revision
                ),
                &path,
            ));
        }
        if !valid_sha256(&record.root_hash) || !valid_sha256(&record.annotation_root_hash) {
            issues.push(issue(
                "REVISION_HASH_INVALID",
                "revision roots must be lowercase SHA-256 digests".to_string(),
                &path,
            ));
        }
        if !valid_index_prefix(&record.index_prefix) {
            issues.push(issue(
                "REVISION_INDEX_PREFIX_INVALID",
                format!("invalid revision indexPrefix {}", record.index_prefix),
                &path,
            ));
        }

        if revision == 0 {
            if record.patch_id.is_some()
                || record.patch_hash.is_some()
                || record.patch_base_revision.is_some()
                || !record.dirty_node_ids.is_empty()
                || !record.dirty_chunk_ids.is_empty()
                || !record.dirty_source_parts.is_empty()
                || record.index_prefix != "indexes/rev-00000000000000000000"
            {
                issues.push(issue(
                    "REVISION_ZERO_INVALID",
                    "revision 0 must not contain patch/dirty state and must use the revision 0 index"
                        .to_string(),
                    &path,
                ));
            }
        } else {
            validate_patch_revision(&record, previous.as_ref(), &mut patch_ids, &path, issues);
        }

        if revision == manifest.revision
            && (record.root_hash != manifest.root_hash
                || record.annotation_root_hash != manifest.annotation_root_hash
                || record.index_prefix != manifest.index_prefix)
        {
            issues.push(issue(
                "REVISION_HEAD_MISMATCH",
                "head revision roots/indexPrefix do not match manifest.json".to_string(),
                &path,
            ));
        }
        previous = Some(record);
    }
}

fn validate_patch_revision(
    record: &RevisionRecord,
    previous: Option<&RevisionRecord>,
    patch_ids: &mut HashSet<String>,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let valid_patch_id = record
        .patch_id
        .as_ref()
        .is_some_and(|patch_id| !patch_id.trim().is_empty() && patch_id.len() <= 256);
    if !valid_patch_id {
        issues.push(issue(
            "REVISION_PATCH_ID_INVALID",
            "non-zero revision requires a patchId of 1..256 bytes".to_string(),
            path,
        ));
    } else if let Some(patch_id) = &record.patch_id {
        if !patch_ids.insert(patch_id.clone()) {
            issues.push(issue(
                "REVISION_PATCH_ID_DUPLICATE",
                format!("patchId {patch_id} is reused by multiple revisions"),
                path,
            ));
        }
    }
    if record
        .patch_hash
        .as_deref()
        .is_none_or(|hash| !valid_sha256(hash))
    {
        issues.push(issue(
            "REVISION_PATCH_HASH_INVALID",
            "non-zero revision requires a lowercase SHA-256 patchHash".to_string(),
            path,
        ));
    }
    if record
        .patch_base_revision
        .is_none_or(|base| base >= record.revision)
    {
        issues.push(issue(
            "REVISION_PATCH_BASE_INVALID",
            "patchBaseRevision must be lower than the target revision".to_string(),
            path,
        ));
    }

    validate_revision_dirty_set(
        &record.dirty_node_ids,
        "node",
        |value| valid_prefixed_id(value, "n_", 32),
        path,
        issues,
    );
    validate_revision_dirty_set(
        &record.dirty_chunk_ids,
        "chunk",
        |value| valid_prefixed_id(value, "c_", 32),
        path,
        issues,
    );
    validate_revision_dirty_set(
        &record.dirty_source_parts,
        "source part",
        valid_source_part,
        path,
        issues,
    );

    let content_changed = !record.dirty_node_ids.is_empty();
    if content_changed == record.dirty_chunk_ids.is_empty()
        || content_changed == record.dirty_source_parts.is_empty()
    {
        issues.push(issue(
            "REVISION_DIRTY_SET_INCONSISTENT",
            "dirty node/chunk/source-part sets do not consistently describe a content change"
                .to_string(),
            path,
        ));
    }
    if let Some(previous) = previous {
        let expected_index = if content_changed {
            format!("indexes/rev-{:020}", record.revision)
        } else {
            previous.index_prefix.clone()
        };
        if record.index_prefix != expected_index {
            issues.push(issue(
                "REVISION_INDEX_TRANSITION_INVALID",
                format!(
                    "revision {} indexPrefix is {}, expected {expected_index}",
                    record.revision, record.index_prefix
                ),
                path,
            ));
        }
        if !content_changed && record.root_hash != previous.root_hash {
            issues.push(issue(
                "ANNOTATION_REVISION_CHANGED_BODY_ROOT",
                "annotation-only revision changed the body root hash".to_string(),
                path,
            ));
        }
    }
}

fn validate_revision_dirty_set(
    values: &[String],
    kind: &str,
    is_valid: impl Fn(&str) -> bool,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    if values.len() > 10_000 {
        issues.push(issue(
            "REVISION_DIRTY_SET_TOO_LARGE",
            format!("revision contains more than 10,000 dirty {kind} entries"),
            path,
        ));
    }
    let mut unique = HashSet::new();
    for value in values {
        if !is_valid(value) {
            issues.push(issue(
                "REVISION_DIRTY_VALUE_INVALID",
                format!("invalid dirty {kind} value {value}"),
                path,
            ));
        } else if !unique.insert(value.as_str()) {
            issues.push(issue(
                "REVISION_DIRTY_VALUE_DUPLICATE",
                format!("duplicate dirty {kind} value {value}"),
                path,
            ));
        }
    }
}

fn validate_descriptor(descriptor: &crate::ChunkDescriptor, issues: &mut Vec<ValidationIssue>) {
    let path = &descriptor.html_href;
    if !valid_prefixed_id(&descriptor.chunk_id, "c_", 32) {
        issues.push(issue(
            "CHUNK_ID_INVALID",
            format!("invalid chunkId {}", descriptor.chunk_id),
            path,
        ));
    }
    if !matches!(
        descriptor.region.as_str(),
        "body"
            | "header"
            | "footer"
            | "footnote"
            | "endnote"
            | "comment"
            | "sheet"
            | "slide"
            | "note"
            | "page"
    ) {
        issues.push(issue(
            "CHUNK_REGION_INVALID",
            format!("unsupported chunk region {}", descriptor.region),
            path,
        ));
    }
    if !valid_sha256(&descriptor.html_hash)
        || descriptor.html_href != format!("chunks/sha256/{}.html", descriptor.html_hash)
    {
        issues.push(issue(
            "CHUNK_ADDRESS_MISMATCH",
            "HTML href is not addressed by htmlHash".to_string(),
            path,
        ));
    }
    if !valid_sha256(&descriptor.map_hash)
        || descriptor.map_href != format!("maps/sha256/{}.json", descriptor.map_hash)
    {
        issues.push(issue(
            "MAP_ADDRESS_MISMATCH",
            "map href is not addressed by mapHash".to_string(),
            &descriptor.map_href,
        ));
    }
    if !valid_sha256(&descriptor.node_bloom) {
        issues.push(issue(
            "NODE_BLOOM_INVALID",
            "nodeBloom must be a lowercase 256-bit digest".to_string(),
            path,
        ));
    }
    if let Some(grid) = &descriptor.grid {
        if descriptor.region != "sheet" {
            issues.push(issue(
                "GRID_CHUNK_REGION_INVALID",
                "grid metadata is only valid on sheet chunks".to_string(),
                path,
            ));
        }
        if !valid_prefixed_id(&grid.sheet_id, "s_", 32) {
            issues.push(issue(
                "GRID_SHEET_ID_INVALID",
                format!("invalid grid sheetId {}", grid.sheet_id),
                path,
            ));
        }
        if grid.sheet_name.is_empty() || grid.sheet_name.chars().count() > 31 {
            issues.push(issue(
                "GRID_SHEET_NAME_INVALID",
                "grid sheetName must contain 1 to 31 Unicode scalar values".to_string(),
                path,
            ));
        }
        if !matches!(
            grid.sheet_state.as_str(),
            "visible" | "hidden" | "veryHidden"
        ) {
            issues.push(issue(
                "GRID_SHEET_STATE_INVALID",
                format!("invalid grid sheetState {}", grid.sheet_state),
                path,
            ));
        }
        if matches!((grid.row_start, grid.row_end), (Some(start), Some(end)) if start > end) {
            issues.push(issue(
                "GRID_ROW_RANGE_INVALID",
                "grid rowStart must not exceed rowEnd".to_string(),
                path,
            ));
        }
        if matches!((grid.column_start, grid.column_end), (Some(start), Some(end)) if start > end) {
            issues.push(issue(
                "GRID_COLUMN_RANGE_INVALID",
                "grid columnStart must not exceed columnEnd".to_string(),
                path,
            ));
        }
        for (name, value) in [
            ("defaultColumnWidthEmu", grid.default_column_width_emu),
            ("defaultRowHeightEmu", grid.default_row_height_emu),
        ] {
            if value.is_some_and(|value| !(1..=100_000_000).contains(&value)) {
                issues.push(issue(
                    "GRID_DEFAULT_DIMENSION_INVALID",
                    format!("grid {name} must be between 1 and 100000000 EMU"),
                    path,
                ));
            }
        }
    }
    if descriptor.byte_length as usize > MAX_CHUNK_BYTES {
        issues.push(issue(
            "CHUNK_TOO_LARGE",
            format!(
                "descriptor declares {} bytes; maximum is {MAX_CHUNK_BYTES}",
                descriptor.byte_length
            ),
            path,
        ));
    }
    if descriptor.block_count == 0 {
        issues.push(issue(
            "CHUNK_BLOCK_COUNT_INVALID",
            "chunk blockCount must be at least 1".to_string(),
            path,
        ));
    }
}

fn validate_map_entry(
    entry: &crate::NodeMapEntry,
    path: &str,
    source: &crate::SourceDescriptor,
    textual_range_end: &mut u64,
    issues: &mut Vec<ValidationIssue>,
) {
    if !valid_sha256(&entry.node_hash) {
        issues.push(issue(
            "NODE_HASH_INVALID",
            format!("node {} has an invalid nodeHash", entry.node_id),
            path,
        ));
    }
    if entry.source.text_ordinal == 0 {
        issues.push(issue(
            "SOURCE_ORDINAL_INVALID",
            format!("node {} has source ordinal 0", entry.node_id),
            path,
        ));
    }
    if entry.source.node_kind.trim().is_empty() || entry.source.node_kind.len() > 128 {
        issues.push(issue(
            "SOURCE_NODE_KIND_INVALID",
            format!("node {} has an invalid source node kind", entry.node_id),
            path,
        ));
    }
    if !valid_source_part(&entry.source.part) {
        issues.push(issue(
            "SOURCE_PART_INVALID",
            format!(
                "node {} has unsafe source part {}",
                entry.node_id, entry.source.part
            ),
            path,
        ));
    }
    for (name, value) in [
        ("paragraphId", entry.source.paragraph_id.as_deref()),
        ("textId", entry.source.text_id.as_deref()),
    ] {
        if value.is_some_and(|value| value.len() > 256) {
            issues.push(issue(
                "SOURCE_ID_TOO_LARGE",
                format!("node {} has {name} longer than 256 bytes", entry.node_id),
                path,
            ));
        }
    }
    if matches!(source.format.as_str(), "html" | "md" | "txt") {
        validate_textual_source_range(entry, source, textual_range_end, path, issues);
    }
}

fn validate_textual_source_range(
    entry: &crate::NodeMapEntry,
    source: &crate::SourceDescriptor,
    previous_end: &mut u64,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let expected_part = match source.format.as_str() {
        "html" => "html/document",
        "md" => "markdown/document",
        _ => "text/document",
    };
    if entry.source.part != expected_part {
        issues.push(issue(
            "TEXTUAL_SOURCE_PART_INVALID",
            format!(
                "node {} maps to {}, expected {expected_part}",
                entry.node_id, entry.source.part
            ),
            path,
        ));
    }
    let Some(value) = entry.source.text_id.as_deref() else {
        issues.push(issue(
            "TEXTUAL_SOURCE_RANGE_MISSING",
            format!("node {} has no source byte range", entry.node_id),
            path,
        ));
        return;
    };
    let parsed = value
        .strip_prefix("bytes:")
        .and_then(|range| range.split_once(':'))
        .and_then(|(start, end)| Some((start.parse::<u64>().ok()?, end.parse::<u64>().ok()?)));
    let Some((start, end)) = parsed else {
        issues.push(issue(
            "TEXTUAL_SOURCE_RANGE_INVALID",
            format!(
                "node {} has invalid source byte range {value}",
                entry.node_id
            ),
            path,
        ));
        return;
    };
    if start > end || end > source.size_bytes || start < *previous_end {
        issues.push(issue(
            "TEXTUAL_SOURCE_RANGE_INVALID",
            format!(
                "node {} has overlapping or out-of-bounds source byte range {start}:{end}",
                entry.node_id
            ),
            path,
        ));
        return;
    }
    *previous_end = end;
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_prefixed_id(value: &str, prefix: &str, hex_length: usize) -> bool {
    value.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == hex_length
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

fn valid_index_prefix(value: &str) -> bool {
    value.strip_prefix("indexes/rev-").is_some_and(|revision| {
        revision.len() == 20 && revision.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn valid_source_part(value: &str) -> bool {
    let allowed_prefix = value.starts_with("word/")
        || value.starts_with("xl/")
        || value.starts_with("ppt/")
        || value.starts_with("pdf/")
        || value.starts_with("html/")
        || value.starts_with("markdown/")
        || value.starts_with("text/");
    allowed_prefix
        && !value.is_empty()
        && value.len() <= 1024
        && !value.starts_with('/')
        && !value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        && !value.contains('\\')
}

fn load_annotations(
    bundle: &Bundle,
    manifest: &crate::HcdManifest,
    issues: &mut Vec<ValidationIssue>,
) -> Option<AnnotationSet> {
    let Some(href) = &manifest.annotation_href else {
        let expected = crate::hash_bytes(b"[]");
        if manifest.annotation_root_hash != expected {
            issues.push(issue(
                "ANNOTATION_HASH_MISMATCH",
                format!(
                    "manifest has no annotation object but root hash is {} instead of {expected}",
                    manifest.annotation_root_hash
                ),
                "manifest.json",
            ));
        }
        return None;
    };
    let expected_href = format!("annotations/sha256/{}.json", manifest.annotation_root_hash);
    if *href != expected_href {
        issues.push(issue(
            "ANNOTATION_ADDRESS_MISMATCH",
            format!("expected {expected_href}, found {href}"),
            href,
        ));
    }
    let path = match bundle.resolve_href(href) {
        Ok(path) => path,
        Err(error) => {
            issues.push(issue("ANNOTATION_INVALID", error.to_string(), href));
            return None;
        }
    };
    match hash_file(&path) {
        Ok(hash) if hash != manifest.annotation_root_hash => issues.push(issue(
            "ANNOTATION_HASH_MISMATCH",
            format!("expected {}, actual {hash}", manifest.annotation_root_hash),
            href,
        )),
        Ok(_) => {}
        Err(error) => {
            issues.push(issue("ANNOTATION_INVALID", error.to_string(), href));
            return None;
        }
    }
    let set: AnnotationSet =
        match read_json_bounded(&path, MAX_CONTROL_PART_BYTES, "annotation set") {
            Ok(set) => set,
            Err(error) => {
                issues.push(issue("ANNOTATION_INVALID", error.to_string(), href));
                return None;
            }
        };
    if set.schema_version != HCD_SCHEMA_VERSION {
        issues.push(issue(
            "ANNOTATION_SCHEMA_VERSION",
            format!("unsupported annotation schema {}", set.schema_version),
            href,
        ));
    }
    let mut annotation_ids = HashSet::new();
    for annotation in &set.annotations {
        if annotation.annotation_id.trim().is_empty() || annotation.annotation_id.len() > 256 {
            issues.push(issue(
                "ANNOTATION_ID_INVALID",
                "annotationId must contain between 1 and 256 bytes".to_string(),
                href,
            ));
        } else if !annotation_ids.insert(annotation.annotation_id.as_str()) {
            issues.push(issue(
                "DUPLICATE_ANNOTATION_ID",
                format!("duplicate annotationId {}", annotation.annotation_id),
                href,
            ));
        }
        if decode_node_id(&annotation.node_id).is_none() {
            issues.push(issue(
                "ANNOTATION_NODE_ID_INVALID",
                format!("invalid annotation nodeId {}", annotation.node_id),
                href,
            ));
        }
        if annotation.kind.trim().is_empty() || annotation.kind.len() > 128 {
            issues.push(issue(
                "ANNOTATION_KIND_INVALID",
                "annotation kind must contain between 1 and 128 bytes".to_string(),
                href,
            ));
        }
        if annotation
            .rule_id
            .as_ref()
            .is_some_and(|rule_id| rule_id.trim().is_empty() || rule_id.len() > 256)
        {
            issues.push(issue(
                "ANNOTATION_RULE_ID_INVALID",
                "annotation ruleId must contain between 1 and 256 bytes".to_string(),
                href,
            ));
        }
        if annotation
            .confidence
            .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            issues.push(issue(
                "ANNOTATION_CONFIDENCE_INVALID",
                "annotation confidence must be finite and between 0 and 1".to_string(),
                href,
            ));
        }
    }
    Some(set)
}

fn validate_annotation_ranges(
    annotations: &AnnotationSet,
    targets: &HashMap<[u8; 16], Option<usize>>,
    issues: &mut Vec<ValidationIssue>,
) {
    for annotation in &annotations.annotations {
        let Some(node_id) = decode_node_id(&annotation.node_id) else {
            continue;
        };
        let Some(length) = targets.get(&node_id).copied().flatten() else {
            issues.push(issue(
                "ANNOTATION_NODE_MISSING",
                format!(
                    "annotation {} references missing node {}",
                    annotation.annotation_id, annotation.node_id
                ),
                "annotations",
            ));
            continue;
        };
        if annotation.start > annotation.end || annotation.end > length {
            issues.push(issue(
                "ANNOTATION_RANGE_INVALID",
                format!(
                    "annotation {} range {}..{} exceeds node length {length}",
                    annotation.annotation_id, annotation.start, annotation.end
                ),
                "annotations",
            ));
        }
    }
}

fn validate_assets(bundle: &Bundle, issues: &mut Vec<ValidationIssue>) -> HashSet<String> {
    let mut known_hashes = HashSet::new();
    let index_path = match bundle.resolve_href("assets/index.json") {
        Ok(path) => path,
        Err(error) => {
            issues.push(issue(
                "ASSET_INDEX_INVALID",
                error.to_string(),
                "assets/index.json",
            ));
            return known_hashes;
        }
    };
    let records: Vec<crate::AssetDescriptor> =
        match read_json_bounded(&index_path, MAX_CONTROL_PART_BYTES, "asset index") {
            Ok(records) => records,
            Err(error) => {
                issues.push(issue(
                    "ASSET_INDEX_INVALID",
                    error.to_string(),
                    "assets/index.json",
                ));
                return known_hashes;
            }
        };
    let mut hrefs = HashSet::new();
    let mut hashes = HashSet::new();
    for record in records {
        if !valid_source_part(&record.source_part) {
            issues.push(issue(
                "ASSET_SOURCE_PART_INVALID",
                format!("unsafe asset source part {}", record.source_part),
                &record.href,
            ));
        }
        if !valid_sha256(&record.hash) {
            issues.push(issue(
                "ASSET_HASH_INVALID",
                format!("invalid asset hash {}", record.hash),
                &record.href,
            ));
        } else {
            known_hashes.insert(record.hash.clone());
        }
        if !hrefs.insert(record.href.clone()) || !hashes.insert(record.hash.clone()) {
            issues.push(issue(
                "DUPLICATE_ASSET",
                "asset index repeats a content-addressed object".to_string(),
                &record.href,
            ));
        }
        let path = match bundle.resolve_href(&record.href) {
            Ok(path) => path,
            Err(error) => {
                issues.push(issue("UNSAFE_ASSET_HREF", error.to_string(), &record.href));
                continue;
            }
        };
        let expected_prefix = format!("assets/sha256/{}", record.hash);
        let valid_address = record.href == expected_prefix
            || record
                .href
                .strip_prefix(&expected_prefix)
                .is_some_and(|suffix| {
                    suffix.starts_with('.')
                        && suffix.len() <= 17
                        && suffix[1..].bytes().all(|byte| byte.is_ascii_alphanumeric())
                });
        if !valid_address {
            issues.push(issue(
                "ASSET_ADDRESS_MISMATCH",
                format!("asset href does not start with its hash {}", record.hash),
                &record.href,
            ));
        }
        match fs::metadata(&path) {
            Ok(metadata) if metadata.len() != record.byte_length => issues.push(issue(
                "ASSET_SIZE_MISMATCH",
                format!(
                    "index says {}, file is {} bytes",
                    record.byte_length,
                    metadata.len()
                ),
                &record.href,
            )),
            Ok(_) => {}
            Err(error) => {
                issues.push(issue("ASSET_MISSING", error.to_string(), &record.href));
                continue;
            }
        }
        match hash_file(&path) {
            Ok(actual) if actual != record.hash => issues.push(issue(
                "ASSET_HASH_MISMATCH",
                format!("expected {}, actual {actual}", record.hash),
                &record.href,
            )),
            Ok(_) => {}
            Err(error) => issues.push(issue("ASSET_READ_ERROR", error.to_string(), &record.href)),
        }
    }
    known_hashes
}

struct NodeIdTracker {
    _scratch: tempfile::TempDir,
    pending: Vec<[u8; 16]>,
    runs: Vec<PathBuf>,
    duplicate: Option<[u8; 16]>,
}

impl NodeIdTracker {
    fn new() -> Result<Self, HcdError> {
        Ok(Self {
            _scratch: tempfile::tempdir()?,
            pending: Vec::with_capacity(NODE_ID_RUN_CAPACITY),
            runs: Vec::new(),
            duplicate: None,
        })
    }

    fn push(&mut self, node_id: [u8; 16]) -> Result<(), HcdError> {
        self.pending.push(node_id);
        if self.pending.len() == NODE_ID_RUN_CAPACITY {
            self.flush_run()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Option<[u8; 16]>, HcdError> {
        self.flush_run()?;
        if self.duplicate.is_some() || self.runs.len() < 2 {
            return Ok(self.duplicate);
        }

        let mut readers = Vec::with_capacity(self.runs.len());
        let mut heap = BinaryHeap::new();
        for (index, path) in self.runs.iter().enumerate() {
            let mut reader = BufReader::new(fs::File::open(path)?);
            if let Some(node_id) = read_node_id(&mut reader)? {
                heap.push(Reverse((node_id, index)));
            }
            readers.push(reader);
        }
        let mut previous = None;
        while let Some(Reverse((node_id, run))) = heap.pop() {
            if previous == Some(node_id) {
                return Ok(Some(node_id));
            }
            previous = Some(node_id);
            if let Some(next) = read_node_id(&mut readers[run])? {
                heap.push(Reverse((next, run)));
            }
        }
        Ok(None)
    }

    fn flush_run(&mut self) -> Result<(), HcdError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.pending.sort_unstable();
        if self.duplicate.is_none() {
            self.duplicate = self
                .pending
                .windows(2)
                .find_map(|pair| (pair[0] == pair[1]).then_some(pair[0]));
        }
        let path = self
            ._scratch
            .path()
            .join(format!("node-ids-{:06}.bin", self.runs.len()));
        let mut writer = BufWriter::new(fs::File::create(&path)?);
        for node_id in self.pending.drain(..) {
            writer.write_all(&node_id)?;
        }
        writer.flush()?;
        self.runs.push(path);
        Ok(())
    }
}

fn read_node_id(reader: &mut impl Read) -> Result<Option<[u8; 16]>, HcdError> {
    let mut node_id = [0u8; 16];
    let mut filled = 0usize;
    while filled < node_id.len() {
        let count = reader.read(&mut node_id[filled..])?;
        if count == 0 {
            if filled == 0 {
                return Ok(None);
            }
            return Err(HcdError::InvalidBundle(
                "temporary node-id run is truncated".to_string(),
            ));
        }
        filled += count;
    }
    Ok(Some(node_id))
}

fn decode_node_id(value: &str) -> Option<[u8; 16]> {
    let hex = value.strip_prefix("n_")?;
    if hex.len() != 32
        || !hex
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut decoded = [0u8; 16];
    for (index, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(decoded)
}

fn encode_node_id(value: &[u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(34);
    output.push_str("n_");
    for byte in value {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn validate_html_fragment(
    path: &std::path::Path,
    asset_hashes: &HashSet<String>,
) -> Result<Vec<HcdTableFragment>, HcdError> {
    let file = fs::File::open(path)?;
    let mut reader = Reader::from_reader(BufReader::new(file));
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(16 * 1024);
    let mut depth = 0usize;
    let mut elements = 0usize;
    let mut table_fragments = Vec::new();
    let mut table_stack: Vec<Option<usize>> = Vec::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                elements = elements.saturating_add(1);
                depth = depth.saturating_add(1);
                validate_html_budget(elements, depth)?;
                let is_table = event.local_name().as_ref().eq_ignore_ascii_case(b"table");
                let is_row = event.local_name().as_ref().eq_ignore_ascii_case(b"tr");
                if let Some(fragment) = validate_html_element(&reader, &event, asset_hashes)? {
                    let index = table_fragments.len();
                    table_fragments.push(fragment);
                    table_stack.push(Some(index));
                } else if is_table {
                    table_stack.push(None);
                } else if is_row {
                    if let Some(Some(index)) = table_stack.last() {
                        let fragment = &mut table_fragments[*index];
                        fragment.actual_row_count = fragment.actual_row_count.saturating_add(1);
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                elements = elements.saturating_add(1);
                validate_html_budget(elements, depth)?;
                let is_row = event.local_name().as_ref().eq_ignore_ascii_case(b"tr");
                if let Some(fragment) = validate_html_element(&reader, &event, asset_hashes)? {
                    table_fragments.push(fragment);
                } else if is_row {
                    if let Some(Some(index)) = table_stack.last() {
                        let fragment = &mut table_fragments[*index];
                        fragment.actual_row_count = fragment.actual_row_count.saturating_add(1);
                    }
                }
            }
            Ok(Event::End(event)) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle("HTML contains an unmatched end element".to_string())
                })?;
                if event.local_name().as_ref().eq_ignore_ascii_case(b"table") {
                    table_stack.pop();
                }
            }
            Ok(Event::DocType(_)) => {
                return Err(HcdError::InvalidBundle(
                    "HTML fragments cannot contain a DOCTYPE".to_string(),
                ));
            }
            Ok(Event::Eof) => {
                if depth != 0 {
                    return Err(HcdError::InvalidBundle(format!(
                        "HTML fragment ended at depth {depth}"
                    )));
                }
                break;
            }
            Ok(_) => {}
            Err(error) => {
                return Err(HcdError::InvalidBundle(format!(
                    "HTML XML parse error: {error}"
                )))
            }
        }
        buffer.clear();
    }
    for fragment in &table_fragments {
        if fragment.actual_row_count != fragment.fragment_row_count {
            return Err(HcdError::InvalidBundle(format!(
                "HCD table {} fragment {} declares {} rows but contains {}",
                fragment.node_id,
                fragment.ordinal,
                fragment.fragment_row_count,
                fragment.actual_row_count
            )));
        }
    }
    Ok(table_fragments)
}

fn validate_html_budget(elements: usize, depth: usize) -> Result<(), HcdError> {
    if elements > MAX_HTML_ELEMENTS {
        return Err(HcdError::ResourceLimit(format!(
            "HTML fragment exceeds {MAX_HTML_ELEMENTS} elements"
        )));
    }
    if depth > MAX_HTML_DEPTH {
        return Err(HcdError::ResourceLimit(format!(
            "HTML fragment exceeds maximum depth {MAX_HTML_DEPTH}"
        )));
    }
    Ok(())
}

fn validate_html_element<B: std::io::BufRead>(
    reader: &Reader<B>,
    event: &quick_xml::events::BytesStart<'_>,
    asset_hashes: &HashSet<String>,
) -> Result<Option<HcdTableFragment>, HcdError> {
    let local = event.local_name();
    let name = std::str::from_utf8(local.as_ref()).unwrap_or("");
    if !matches!(
        name.to_ascii_lowercase().as_str(),
        "section"
            | "div"
            | "p"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "pre"
            | "code"
            | "blockquote"
            | "ul"
            | "ol"
            | "li"
            | "aside"
            | "a"
            | "span"
            | "strong"
            | "em"
            | "del"
            | "br"
            | "hr"
            | "table"
            | "colgroup"
            | "col"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "th"
            | "td"
            | "dl"
            | "dt"
            | "dd"
            | "sup"
            | "sub"
            | "mark"
            | "kbd"
            | "u"
            | "s"
            | "small"
            | "img"
    ) {
        return Err(HcdError::InvalidBundle(format!(
            "HTML element {name} is not allowed by hcd/1"
        )));
    }
    for attribute in event.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|error| HcdError::InvalidBundle(format!("invalid HTML attribute: {error}")))?;
        let key = std::str::from_utf8(attribute.key.as_ref()).unwrap_or("");
        let normalized_key = key.to_ascii_lowercase();
        if normalized_key.starts_with("on") {
            return Err(HcdError::InvalidBundle(format!(
                "forbidden event attribute {key}"
            )));
        }
        if !matches!(
            normalized_key.as_str(),
            "class"
                | "id"
                | "style"
                | "href"
                | "src"
                | "alt"
                | "title"
                | "lang"
                | "colspan"
                | "rowspan"
                | "span"
                | "start"
                | "width"
                | "height"
                | "loading"
                | "decoding"
        ) && !key.starts_with("data-hcd-")
        {
            return Err(HcdError::InvalidBundle(format!(
                "HTML attribute {key} is not allowed by hcd/1"
            )));
        }
        if normalized_key == "style" {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| {
                    HcdError::InvalidBundle(format!("invalid inline style attribute: {error}"))
                })?;
            crate::html::validate_inline_style(&value)?;
        }
        if matches!(normalized_key.as_str(), "loading" | "decoding") {
            if !name.eq_ignore_ascii_case("img") {
                return Err(HcdError::InvalidBundle(format!(
                    "HTML {normalized_key} is allowed only on img elements"
                )));
            }
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| {
                    HcdError::InvalidBundle(format!("invalid {normalized_key} attribute: {error}"))
                })?;
            let valid = match normalized_key.as_str() {
                "loading" => matches!(value.as_ref(), "lazy" | "eager"),
                "decoding" => matches!(value.as_ref(), "async" | "sync" | "auto"),
                _ => false,
            };
            if !valid {
                return Err(HcdError::InvalidBundle(format!(
                    "invalid HTML {normalized_key} value {value}"
                )));
            }
        }
        if matches!(
            normalized_key.as_str(),
            "colspan" | "rowspan" | "span" | "start"
        ) {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| {
                    HcdError::InvalidBundle(format!("invalid table span attribute: {error}"))
                })?;
            let limit = if matches!(normalized_key.as_str(), "rowspan" | "start") {
                1_048_576u64
            } else {
                16_384u64
            };
            let parsed = (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| value.parse::<u64>().ok())
                .flatten();
            if !parsed.is_some_and(|value| (1..=limit).contains(&value)) {
                return Err(HcdError::InvalidBundle(format!(
                    "HTML {normalized_key} must be an integer in 1..={limit}, got {value}"
                )));
            }
        }
        if matches!(normalized_key.as_str(), "src" | "href") {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| {
                    HcdError::InvalidBundle(format!("invalid URL attribute: {error}"))
                })?;
            let lower = value.to_ascii_lowercase();
            let asset_hash = value.strip_prefix("asset://sha256/");
            let valid_asset =
                asset_hash.is_some_and(|hash| valid_sha256(hash) && asset_hashes.contains(hash));
            let allowed = valid_asset
                || (normalized_key == "href" && value.starts_with('#'))
                || (normalized_key == "href"
                    && (lower.starts_with("https://")
                        || lower.starts_with("http://")
                        || lower.starts_with("mailto:")
                        || lower.starts_with("tel:")));
            if !allowed {
                return Err(HcdError::InvalidBundle(format!(
                    "external URL is forbidden: {value}"
                )));
            }
        }
    }
    if name.eq_ignore_ascii_case("table") {
        parse_hcd_table_fragment(reader, event)
    } else {
        Ok(None)
    }
}

fn parse_hcd_table_fragment<B: std::io::BufRead>(
    reader: &Reader<B>,
    event: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<HcdTableFragment>, HcdError> {
    const MARKERS: [&str; 9] = [
        "data-hcd-table-node-id",
        "data-hcd-table-fragment",
        "data-hcd-row-start",
        "data-hcd-row-end",
        "data-hcd-fragment-row-count",
        "data-hcd-column-count",
        "data-hcd-table-continuation",
        "data-hcd-table-final",
        "data-hcd-row-count",
    ];
    let mut attributes = HashMap::new();
    for attribute in event.attributes().with_checks(true) {
        let attribute = attribute
            .map_err(|error| HcdError::InvalidBundle(format!("invalid HTML attribute: {error}")))?;
        let name = std::str::from_utf8(attribute.key.as_ref())
            .unwrap_or("")
            .to_ascii_lowercase();
        if MARKERS.contains(&name.as_str()) {
            let value = attribute
                .decode_and_unescape_value(reader.decoder())
                .map_err(|error| {
                    HcdError::InvalidBundle(format!(
                        "invalid HCD table fragment attribute {name}: {error}"
                    ))
                })?
                .into_owned();
            attributes.insert(name, value);
        }
    }
    if attributes.is_empty() {
        return Ok(None);
    }
    let node_id = attributes
        .get("data-hcd-table-node-id")
        .filter(|value| decode_node_id(value).is_some())
        .cloned()
        .ok_or_else(|| {
            HcdError::InvalidBundle(
                "HCD table fragment requires a canonical data-hcd-table-node-id".to_string(),
            )
        })?;
    let ordinal = hcd_table_usize(
        &attributes,
        "data-hcd-table-fragment",
        MAX_HCD_TABLE_FRAGMENTS.saturating_sub(1),
    )?;
    let row_start = hcd_table_usize(&attributes, "data-hcd-row-start", MAX_HCD_TABLE_ROWS)?;
    let row_end = hcd_table_usize(&attributes, "data-hcd-row-end", MAX_HCD_TABLE_ROWS)?;
    let fragment_row_count = hcd_table_usize(
        &attributes,
        "data-hcd-fragment-row-count",
        MAX_HCD_TABLE_ROWS,
    )?;
    let column_count =
        hcd_table_usize(&attributes, "data-hcd-column-count", MAX_HCD_TABLE_COLUMNS)?;
    let continuation = hcd_table_true(&attributes, "data-hcd-table-continuation")?;
    let final_fragment = hcd_table_true(&attributes, "data-hcd-table-final")?;
    let total_row_count = attributes
        .contains_key("data-hcd-row-count")
        .then(|| hcd_table_usize(&attributes, "data-hcd-row-count", MAX_HCD_TABLE_ROWS))
        .transpose()?;
    if continuation != (ordinal > 0) {
        return Err(HcdError::InvalidBundle(format!(
            "HCD table {node_id} fragment {ordinal} has inconsistent continuation metadata"
        )));
    }
    if final_fragment != total_row_count.is_some() {
        return Err(HcdError::InvalidBundle(format!(
            "HCD table {node_id} final marker and row count must appear together"
        )));
    }
    let expected_rows = if row_start == 0 && row_end == 0 {
        0
    } else if row_start == 0 || row_end < row_start {
        return Err(HcdError::InvalidBundle(format!(
            "HCD table {node_id} fragment {ordinal} has invalid row range {row_start}..={row_end}"
        )));
    } else {
        row_end - row_start + 1
    };
    if fragment_row_count != expected_rows {
        return Err(HcdError::InvalidBundle(format!(
            "HCD table {node_id} fragment {ordinal} declares {fragment_row_count} rows for range {row_start}..={row_end}"
        )));
    }
    if fragment_row_count > 0 && column_count == 0 {
        return Err(HcdError::InvalidBundle(format!(
            "HCD table {node_id} has rows but declares zero columns"
        )));
    }
    Ok(Some(HcdTableFragment {
        node_id,
        ordinal,
        row_start,
        row_end,
        fragment_row_count,
        column_count,
        final_fragment,
        total_row_count,
        actual_row_count: 0,
    }))
}

fn hcd_table_usize(
    attributes: &HashMap<String, String>,
    name: &str,
    maximum: usize,
) -> Result<usize, HcdError> {
    let value = attributes
        .get(name)
        .ok_or_else(|| HcdError::InvalidBundle(format!("HCD table fragment is missing {name}")))?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(HcdError::InvalidBundle(format!(
            "HCD table {name} must be a canonical non-negative integer"
        )));
    }
    let parsed = value
        .parse::<usize>()
        .map_err(|_| HcdError::InvalidBundle(format!("HCD table {name} is too large")))?;
    if parsed > maximum {
        return Err(HcdError::ResourceLimit(format!(
            "HCD table {name} exceeds {maximum}"
        )));
    }
    Ok(parsed)
}

fn hcd_table_true(attributes: &HashMap<String, String>, name: &str) -> Result<bool, HcdError> {
    match attributes.get(name).map(String::as_str) {
        None => Ok(false),
        Some("true") => Ok(true),
        Some(value) => Err(HcdError::InvalidBundle(format!(
            "HCD table {name} must be omitted or true, got {value}"
        ))),
    }
}

fn issue(code: &str, message: String, path: &str) -> ValidationIssue {
    ValidationIssue {
        code: code.to_string(),
        message,
        path: Some(path.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dangerous_html_elements_attributes_and_urls_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("chunk.html");
        let asset_hash = "a".repeat(64);
        let assets = HashSet::from([asset_hash.clone()]);
        for html in [
            "<section><script>bad()</script></section>",
            "<section><span onclick=\"bad()\">x</span></section>",
            "<section><span style=\"background:url(javascript:x)\">x</span></section>",
            "<section><img src=\"https://example.test/tracker\"/></section>",
            "<section><img srcset=\"https://example.test/tracker 1x\"/></section>",
            "<section><img loading=\"later\"/></section>",
            "<section><img decoding=\"parallel\"/></section>",
            "<section loading=\"lazy\">bad placement</section>",
            "<section><img SRC=\"https://example.test/tracker\"/></section>",
            "<table><tbody><tr><td colspan=\"16385\">x</td></tr></tbody></table>",
            "<table><tbody><tr><td rowspan=\"1048577\">x</td></tr></tbody></table>",
            "<table><colgroup><col span=\"0\"/></colgroup><tbody></tbody></table>",
            "<table><tbody><tr><td colspan=\"1e3\">x</td></tr></tbody></table>",
            "<ol start=\"0\"><li>x</li></ol>",
            "<ol start=\"1048577\"><li>x</li></ol>",
            "<!DOCTYPE section><section></section>",
        ] {
            fs::write(&path, html).unwrap();
            assert!(
                validate_html_fragment(&path, &assets).is_err(),
                "accepted {html}"
            );
        }
        fs::write(
            &path,
            format!(
                "<section><a href=\"https://example.test\" style=\"color:#123456;text-decoration:underline\">safe</a><img src=\"asset://sha256/{asset_hash}\" alt=\"\" loading=\"lazy\" decoding=\"async\"/></section>"
            ),
        )
        .unwrap();
        assert!(validate_html_fragment(&path, &assets).is_ok());
        fs::write(
            &path,
            "<table><colgroup><col span=\"0002\"/></colgroup><tbody><tr><td colspan=\"2\" rowspan=\"0000000002\">safe</td></tr></tbody></table>",
        )
        .unwrap();
        assert!(validate_html_fragment(&path, &assets).is_ok());
        fs::write(
            &path,
            "<section><h1>Heading</h1><blockquote>Quote</blockquote><pre>Pre</pre><ol start=\"2\"><li>Item</li></ol><table><tbody><tr><th>Head</th></tr></tbody></table></section>",
        )
        .unwrap();
        assert!(validate_html_fragment(&path, &assets).is_ok());
        fs::write(
            &path,
            "<section><h2 id=\"setext\">Setext</h2><dl><dt>Term</dt><dd>Definition</dd></dl><table><thead><tr><th>Head</th></tr></thead><tbody><tr><td><mark><kbd>Ctrl</kbd> <u>U</u> <s>S</s> H<sub>2</sub>O x<sup>2</sup> <small>note</small></mark></td></tr></tbody><tfoot><tr><td>Foot</td></tr></tfoot></table></section>",
        )
        .unwrap();
        assert!(validate_html_fragment(&path, &assets).is_ok());

        let deep = format!(
            "{}x{}",
            "<div>".repeat(MAX_HTML_DEPTH + 1),
            "</div>".repeat(MAX_HTML_DEPTH + 1)
        );
        fs::write(&path, deep).unwrap();
        assert!(validate_html_fragment(&path, &assets).is_err());
    }

    #[test]
    fn node_id_tracker_detects_duplicates_after_spilling_to_disk() {
        let mut tracker = NodeIdTracker::new().unwrap();
        for value in 0..NODE_ID_RUN_CAPACITY as u32 {
            let mut node_id = [0u8; 16];
            node_id[12..].copy_from_slice(&value.to_be_bytes());
            tracker.push(node_id).unwrap();
        }
        let duplicate = [0u8; 16];
        tracker.push(duplicate).unwrap();
        assert_eq!(tracker.finish().unwrap(), Some(duplicate));
    }

    #[test]
    fn hcd_table_fragment_metadata_is_strict_and_contiguous() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("table-fragment.html");
        let assets = HashSet::new();
        let table_id = "n_0123456789abcdef0123456789abcdef";
        fs::write(
            &path,
            format!(
                r#"<table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="0" data-hcd-row-start="1" data-hcd-row-end="2" data-hcd-fragment-row-count="2" data-hcd-column-count="1"><tbody><tr><td>A</td></tr><tr><td>B</td></tr></tbody></table>"#
            ),
        )
        .unwrap();
        let first = validate_html_fragment(&path, &assets).unwrap().remove(0);
        fs::write(
            &path,
            format!(
                r#"<table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="1" data-hcd-row-start="3" data-hcd-row-end="3" data-hcd-fragment-row-count="1" data-hcd-column-count="1" data-hcd-table-continuation="true" data-hcd-table-final="true" data-hcd-row-count="3"><tbody><tr><td>C</td></tr></tbody></table>"#
            ),
        )
        .unwrap();
        let second = validate_html_fragment(&path, &assets).unwrap().remove(0);
        let mut tracker = HcdTableFragmentTracker::default();
        assert!(tracker.observe(&first).unwrap());
        assert!(!tracker.observe(&second).unwrap());
        tracker.finish().unwrap();

        let mut incomplete = HcdTableFragmentTracker::default();
        incomplete.observe(&first).unwrap();
        assert!(incomplete.finish().is_err());

        fs::write(
            &path,
            format!(
                r#"<table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="2" data-hcd-row-start="3" data-hcd-row-end="3" data-hcd-fragment-row-count="1" data-hcd-column-count="1" data-hcd-table-continuation="true" data-hcd-table-final="true" data-hcd-row-count="3"><tbody><tr><td>C</td></tr></tbody></table>"#
            ),
        )
        .unwrap();
        let gap = validate_html_fragment(&path, &assets).unwrap().remove(0);
        let mut tracker = HcdTableFragmentTracker::default();
        tracker.observe(&first).unwrap();
        assert!(tracker.observe(&gap).is_err());

        fs::write(
            &path,
            format!(
                r#"<table data-hcd-table-node-id="{table_id}" data-hcd-table-fragment="0" data-hcd-row-start="1" data-hcd-row-end="2" data-hcd-fragment-row-count="2" data-hcd-column-count="1"><tbody><tr><td>A</td></tr></tbody></table>"#
            ),
        )
        .unwrap();
        assert!(validate_html_fragment(&path, &assets).is_err());
    }

    #[test]
    fn canonical_identifier_decoding_is_strict_and_repeatable() {
        let value = "n_0123456789abcdef0123456789abcdef";
        let decoded = decode_node_id(value).unwrap();
        assert_eq!(encode_node_id(&decoded), value);
        assert!(decode_node_id("n_0123456789ABCDEF0123456789abcdef").is_none());
        assert!(decode_node_id("../manifest.json").is_none());
    }
}
