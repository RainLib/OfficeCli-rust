use hcd_core::{
    extract_html_text_nodes, hash_bytes, hash_file, Bundle, FidelityReport, HcdError, HcdManifest,
    ImportEvent, SourceDescriptor, TextExtractEntry, DEFAULT_CHUNK_BLOCKS,
    DEFAULT_CHUNK_SOFT_BYTES, HCD_SCHEMA_VERSION, MAX_REVISION,
};
use quick_xml::events::Event;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_XML_ELEMENTS: usize = 3_000_000;
const MAX_XML_DEPTH: usize = 256;

#[derive(Default)]
pub(crate) struct XmlBudget {
    elements: usize,
    depth: usize,
}

impl XmlBudget {
    pub(crate) fn observe(&mut self, event: &Event<'_>, part: &str) -> Result<(), HcdError> {
        match event {
            Event::Start(_) => {
                self.elements = self.elements.saturating_add(1);
                self.depth = self.depth.saturating_add(1);
                self.check(part)?;
            }
            Event::Empty(_) => {
                self.elements = self.elements.saturating_add(1);
                self.check(part)?;
            }
            Event::End(_) => {
                self.depth = self.depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle(format!("unbalanced XML depth in {part}"))
                })?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn finish(self, part: &str) -> Result<(), HcdError> {
        if self.depth != 0 {
            return Err(HcdError::InvalidBundle(format!(
                "unbalanced XML depth in {part}"
            )));
        }
        Ok(())
    }

    fn check(&self, part: &str) -> Result<(), HcdError> {
        if self.elements > MAX_XML_ELEMENTS {
            return Err(HcdError::ResourceLimit(format!(
                "{part} exceeds {MAX_XML_ELEMENTS} XML elements"
            )));
        }
        if self.depth > MAX_XML_DEPTH {
            return Err(HcdError::ResourceLimit(format!(
                "{part} exceeds the maximum XML depth {MAX_XML_DEPTH}"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub document_id: String,
    pub chunk_soft_bytes: usize,
    pub chunk_blocks: usize,
}

impl ImportOptions {
    pub fn new(document_id: impl Into<String>) -> Self {
        Self {
            document_id: document_id.into(),
            chunk_soft_bytes: DEFAULT_CHUNK_SOFT_BYTES,
            chunk_blocks: DEFAULT_CHUNK_BLOCKS,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub revision: Option<u64>,
    pub fidelity_report: Option<PathBuf>,
}

pub(crate) fn source_identity(
    source: &Path,
    expected_format: &str,
) -> Result<(String, u64), HcdError> {
    source_identity_with_extensions(source, &[expected_format])
}

pub(crate) fn source_identity_with_extensions(
    source: &Path,
    expected_extensions: &[&str],
) -> Result<(String, u64), HcdError> {
    let metadata = std::fs::metadata(source)?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "source {} is {} bytes; maximum is {MAX_SOURCE_BYTES}",
            source.display(),
            metadata.len()
        )));
    }
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !expected_extensions.contains(&extension.as_str()) {
        return Err(HcdError::Unsupported(format!(
            "expected {}, found .{extension}",
            expected_extensions
                .iter()
                .map(|value| format!(".{value}"))
                .collect::<Vec<_>>()
                .join(" or ")
        )));
    }
    Ok((hash_file(source)?, metadata.len()))
}

pub(crate) fn base_manifest(
    options: &ImportOptions,
    format: &str,
    profile: &str,
    source_hash: String,
    source_size: u64,
) -> HcdManifest {
    HcdManifest {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        document_id: options.document_id.clone(),
        profile: profile.to_string(),
        revision: 0,
        source: SourceDescriptor {
            format: format.to_string(),
            sha256: source_hash,
            size_bytes: source_size,
        },
        root_hash: String::new(),
        annotation_root_hash: String::new(),
        annotation_href: None,
        index_prefix: String::new(),
        index_page_count: 0,
        chunk_count: 0,
        styles_href: "styles.css".to_string(),
        capabilities: hcd_core::HcdCapabilities::default(),
        fidelity: None,
        state: "IMPORTING".to_string(),
        warnings: Vec::new(),
    }
}

pub(crate) fn emit_started<F>(
    emit: &mut F,
    options: &ImportOptions,
    source_hash: &str,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    if options.document_id.trim().is_empty() || options.document_id.len() > 256 {
        return Err(HcdError::InvalidBundle(
            "documentId must contain between 1 and 256 bytes".to_string(),
        ));
    }
    emit(&ImportEvent::ImportStarted {
        document_id: options.document_id.clone(),
        source_sha256: source_hash.to_string(),
    })
}

pub(crate) fn emit_failed<F>(emit: &mut F, options: &ImportOptions, error: &HcdError)
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let _ = emit(&ImportEvent::Failed {
        document_id: options.document_id.clone(),
        error: error.to_string(),
    });
}

pub(crate) fn finish_import<F>(
    writer: hcd_core::BundleWriter,
    manifest: HcdManifest,
    emit: &mut F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let manifest = writer.finish(manifest)?;
    let _ = emit(&ImportEvent::Completed {
        manifest: manifest.clone(),
    });
    Ok(manifest)
}

pub(crate) fn manifest_at_revision(
    bundle: &Bundle,
    head: &HcdManifest,
    revision: u64,
) -> Result<HcdManifest, HcdError> {
    if revision == head.revision {
        return Ok(head.clone());
    }
    let record = bundle.revision(revision)?;
    let mut manifest = head.clone();
    manifest.revision = revision;
    manifest.root_hash = record.root_hash;
    manifest.annotation_root_hash = record.annotation_root_hash;
    manifest.annotation_href = (manifest.annotation_root_hash != hash_bytes(b"[]"))
        .then(|| format!("annotations/sha256/{}.json", manifest.annotation_root_hash));
    manifest.index_prefix = record.index_prefix;
    Ok(manifest)
}

pub(crate) fn dirty_state_through(
    bundle: &Bundle,
    revision: u64,
) -> Result<(HashSet<String>, HashSet<String>), HcdError> {
    let mut dirty_parts = HashSet::new();
    let mut dirty_nodes = HashSet::new();
    for current in 1..=revision {
        let record = bundle.revision(current)?;
        dirty_parts.extend(record.dirty_source_parts);
        dirty_nodes.extend(record.dirty_node_ids);
    }
    Ok((dirty_parts, dirty_nodes))
}

pub(crate) fn collect_dirty_nodes(
    bundle: &Bundle,
    manifest: &HcdManifest,
    parts: &HashSet<String>,
    node_ids: &HashSet<String>,
) -> Result<Vec<TextExtractEntry>, HcdError> {
    if parts.is_empty() && node_ids.is_empty() {
        return Ok(Vec::new());
    }
    if parts.is_empty() || node_ids.is_empty() {
        return Err(HcdError::InvalidBundle(
            "revision dirty source parts and dirty node ids must either both be empty or both be present"
                .to_string(),
        ));
    }
    let mut output = Vec::new();
    let mut found = HashSet::with_capacity(node_ids.len());
    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(manifest, page_number)?;
        for descriptor in page.chunks {
            let source_map = bundle.read_map(&descriptor)?;
            if !source_map
                .entries
                .iter()
                .any(|entry| node_ids.contains(&entry.node_id))
            {
                continue;
            }
            let html = bundle.read_chunk(&descriptor)?;
            if hash_bytes(html.as_bytes()) != descriptor.html_hash {
                return Err(HcdError::InvalidBundle(format!(
                    "chunk {} hash mismatch",
                    descriptor.chunk_id
                )));
            }
            let html_nodes = extract_html_text_nodes(&html)?;
            for entry in source_map.entries {
                if !node_ids.contains(&entry.node_id) {
                    continue;
                }
                if !parts.contains(&entry.source.part) {
                    return Err(HcdError::InvalidBundle(format!(
                        "dirty node {} maps to source part {} that is absent from the revision dirty part set",
                        entry.node_id, entry.source.part
                    )));
                }
                let text = html_nodes.get(&entry.node_id).ok_or_else(|| {
                    HcdError::InvalidBundle(format!(
                        "mapped node {} is missing from canonical HTML",
                        entry.node_id
                    ))
                })?;
                let actual = hash_bytes(text.as_bytes());
                if actual != entry.node_hash {
                    return Err(HcdError::InvalidBundle(format!(
                        "node {} expected hash {}, actual {actual}",
                        entry.node_id, entry.node_hash
                    )));
                }
                found.insert(entry.node_id.clone());
                output.push(TextExtractEntry {
                    chunk_id: descriptor.chunk_id.clone(),
                    node_id: entry.node_id,
                    text: text.clone(),
                    node_hash: entry.node_hash,
                    source: entry.source,
                });
            }
        }
    }
    if found.len() != node_ids.len() {
        let mut missing: Vec<&str> = node_ids
            .iter()
            .filter(|node_id| !found.contains(*node_id))
            .map(String::as_str)
            .collect();
        missing.sort_unstable();
        return Err(HcdError::InvalidBundle(format!(
            "revision references dirty HCD nodes that are missing from its index: {missing:?}"
        )));
    }
    Ok(output)
}

pub(crate) fn checked_export_state(
    bundle: &Bundle,
    source: &Path,
    options: &ExportOptions,
) -> Result<(HcdManifest, u64, HashSet<String>, HashSet<String>), HcdError> {
    let head = bundle.manifest()?;
    if head.revision > MAX_REVISION {
        return Err(HcdError::ResourceLimit(format!(
            "manifest revision {} exceeds the maximum {MAX_REVISION}",
            head.revision
        )));
    }
    let revision = options.revision.unwrap_or(head.revision);
    if revision > head.revision {
        return Err(HcdError::RevisionConflict(format!(
            "requested revision {revision} is ahead of head {}",
            head.revision
        )));
    }
    let actual = hash_file(source)?;
    if actual != head.source.sha256 {
        return Err(HcdError::SourceMismatch(format!(
            "expected source {}, actual {actual}",
            head.source.sha256
        )));
    }
    let manifest = manifest_at_revision(bundle, &head, revision)?;
    let (dirty_parts, dirty_nodes) = dirty_state_through(bundle, revision)?;
    reject_presentation_style_source_export(bundle, &manifest, &dirty_nodes)?;
    reject_image_source_export(bundle, &manifest, &dirty_nodes)?;
    Ok((manifest, revision, dirty_parts, dirty_nodes))
}

fn reject_image_source_export(
    bundle: &Bundle,
    manifest: &HcdManifest,
    dirty_nodes: &HashSet<String>,
) -> Result<(), HcdError> {
    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(manifest, page_number)?;
        for descriptor in page.chunks {
            let source_map = bundle.read_map(&descriptor)?;
            if let Some(entry) = source_map.entries.iter().find(|entry| {
                dirty_nodes.contains(&entry.node_id) && entry.source.node_kind == "image"
            }) {
                return Err(HcdError::Unsupported(format!(
                    "revision changes image node {}; source-backed {} image rewrite is not implemented and export was stopped before writing output; omit --source to use the pure-Rust semantic rebuild",
                    entry.node_id, manifest.source.format
                )));
            }
        }
    }
    Ok(())
}

fn reject_presentation_style_source_export(
    bundle: &Bundle,
    manifest: &HcdManifest,
    dirty_nodes: &HashSet<String>,
) -> Result<(), HcdError> {
    if dirty_nodes.is_empty() {
        return Ok(());
    }
    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(manifest, page_number)?;
        for descriptor in page.chunks {
            let source_map = bundle.read_map(&descriptor)?;
            if !source_map
                .entries
                .iter()
                .any(|entry| dirty_nodes.contains(&entry.node_id))
            {
                continue;
            }
            let html = bundle.read_chunk(&descriptor)?;
            if html.contains("data-hcd-style-patched=\"true\"") {
                return Err(HcdError::Unsupported(
                    "this revision contains node.style presentation changes; source-backed export cannot preserve those changes and was stopped before writing output"
                        .to_string(),
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn write_fidelity_report(
    options: &ExportOptions,
    report: &FidelityReport,
) -> Result<(), HcdError> {
    if let Some(path) = &options.fidelity_report {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(file, report)?;
    }
    Ok(())
}

pub(crate) fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(crate) fn escape_attribute(text: &str) -> String {
    escape_text(text)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use hcd_core::{
        hash_bytes, BundleWriter, ChunkSourceMap, NodeMapEntry, SourceAnchor, HCD_SCHEMA_VERSION,
    };
    use quick_xml::events::BytesStart;

    #[test]
    fn xml_budget_rejects_excessive_depth() {
        let mut budget = XmlBudget::default();
        let start = Event::Start(BytesStart::new("nested"));
        for _ in 0..MAX_XML_DEPTH {
            budget.observe(&start, "test.xml").unwrap();
        }
        let error = budget.observe(&start, "test.xml").unwrap_err();
        assert!(error.to_string().contains("maximum XML depth"));
    }

    #[test]
    fn export_collection_materializes_only_dirty_nodes_not_the_whole_part() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        writer.write_styles("").unwrap();
        let first_id = "n_00000000000000000000000000000001";
        let second_id = "n_00000000000000000000000000000002";
        let first_hash = hash_bytes(b"unchanged");
        let second_hash = hash_bytes(b"changed");
        let html = format!(
            "<table><tr><td><span data-hcd-id=\"{first_id}\" data-hcd-node-hash=\"{first_hash}\">unchanged</span></td><td><span data-hcd-id=\"{second_id}\" data-hcd-node-hash=\"{second_hash}\">changed</span></td></tr></table>"
        );
        let part = "xl/worksheets/sheet1.xml";
        let map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: "c_00000000000000000000000000000001".to_string(),
            entries: vec![
                NodeMapEntry {
                    node_id: first_id.to_string(),
                    node_hash: first_hash,
                    source: test_anchor(part, 1, "A1"),
                },
                NodeMapEntry {
                    node_id: second_id.to_string(),
                    node_hash: second_hash,
                    source: test_anchor(part, 2, "B1"),
                },
            ],
        };
        writer
            .write_chunk(
                map.chunk_id.clone(),
                "sheet".to_string(),
                html,
                map,
                1,
                false,
            )
            .unwrap();
        let options = ImportOptions::new("test-document");
        let manifest = writer
            .finish(base_manifest(&options, "xlsx", "grid", "0".repeat(64), 1))
            .unwrap();
        let bundle = Bundle::open(bundle_path).unwrap();
        let parts = HashSet::from([part.to_string()]);
        let dirty = HashSet::from([second_id.to_string()]);

        let nodes = collect_dirty_nodes(&bundle, &manifest, &parts, &dirty).unwrap();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, second_id);
        assert_eq!(nodes[0].text, "changed");
    }

    fn test_anchor(part: &str, ordinal: u64, cell: &str) -> SourceAnchor {
        SourceAnchor {
            part: part.to_string(),
            text_ordinal: ordinal,
            paragraph_id: Some(cell.to_string()),
            text_id: None,
            node_kind: "cell".to_string(),
            editable: true,
        }
    }
}
