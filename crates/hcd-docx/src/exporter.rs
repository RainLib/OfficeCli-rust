use hcd_core::{
    extract_html_text_nodes, hash_file, Bundle, FidelityLevel, FidelityReport, HcdError,
    HcdManifest, NodeMapEntry, HCD_SCHEMA_VERSION, MAX_REVISION,
};
use oxml::{PackageError, StreamingOxmlArchive, StreamingOxmlRewriter};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub revision: Option<u64>,
    pub fidelity_report: Option<PathBuf>,
}

pub fn export_docx(
    bundle_path: impl AsRef<Path>,
    source: impl AsRef<Path>,
    target: impl AsRef<Path>,
    options: &ExportOptions,
) -> Result<FidelityReport, HcdError> {
    let bundle = Bundle::open(bundle_path)?;
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
    let source = source.as_ref();
    let target = target.as_ref();
    let actual_source_hash = hash_file(source)?;
    if actual_source_hash != head.source.sha256 {
        return Err(HcdError::SourceMismatch(format!(
            "expected source {}, actual {actual_source_hash}",
            head.source.sha256
        )));
    }

    let revision_manifest = manifest_at_revision(&bundle, &head, revision)?;
    let (dirty_parts, dirty_node_ids) = dirty_state_through(&bundle, revision)?;
    reject_image_source_export(&bundle, &revision_manifest, &dirty_node_ids)?;
    let text_by_part =
        collect_text_by_part(&bundle, &revision_manifest, &dirty_parts, &dirty_node_ids)?;

    let temp_dir = tempfile::tempdir()?;
    let mut replacement_paths = HashMap::new();
    if !text_by_part.is_empty() {
        let mut archive = StreamingOxmlArchive::open(source).map_err(package_error)?;
        for (part, replacements) in &text_by_part {
            if !archive.contains(part) {
                return Err(HcdError::InvalidBundle(format!(
                    "source part {part} referenced by HCD is missing"
                )));
            }
            let replacement_path = temp_dir.path().join(safe_temp_name(part));
            let output = std::fs::File::create(&replacement_path)?;
            archive
                .with_part(part, |reader| {
                    rewrite_text_part(reader, BufWriter::new(output), replacements)
                        .map_err(|error| PackageError::ReadPartError(error.to_string()))
                })
                .map_err(package_error)?;
            replacement_paths.insert(part.clone(), replacement_path);
        }
    }

    let changed =
        StreamingOxmlRewriter::rewrite(source, target, &replacement_paths, "word/document.xml")
            .map_err(package_error)?;

    let report = FidelityReport {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        level: if changed.is_empty() {
            FidelityLevel::Exact
        } else {
            FidelityLevel::High
        },
        preserved: vec![
            "unmodified OOXML entries copied as raw compressed payloads".to_string(),
            "paragraph and run formatting".to_string(),
            "relationships, media, numbering, headers and footers".to_string(),
        ],
        flattened: Vec::new(),
        dropped: vec!["HCD recognition annotations are not exported to DOCX".to_string()],
        warnings: head.warnings.clone(),
    };
    if let Some(path) = &options.fidelity_report {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(file, &report)?;
    }
    Ok(report)
}

fn reject_image_source_export(
    bundle: &Bundle,
    manifest: &HcdManifest,
    dirty_node_ids: &HashSet<String>,
) -> Result<(), HcdError> {
    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(manifest, page_number)?;
        for descriptor in page.chunks {
            let source_map = bundle.read_map(&descriptor)?;
            if let Some(entry) = source_map.entries.iter().find(|entry| {
                dirty_node_ids.contains(&entry.node_id) && entry.source.node_kind == "image"
            }) {
                return Err(HcdError::Unsupported(format!(
                    "revision changes image node {}; source-backed DOCX image rewrite is not implemented and export was stopped before writing output; omit --source to use the pure-Rust semantic rebuild",
                    entry.node_id
                )));
            }
        }
    }
    Ok(())
}

fn manifest_at_revision(
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
    manifest.annotation_href = (manifest.annotation_root_hash != hcd_core::hash_bytes(b"[]"))
        .then(|| format!("annotations/sha256/{}.json", manifest.annotation_root_hash));
    manifest.index_prefix = record.index_prefix;
    Ok(manifest)
}

fn dirty_state_through(
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

fn collect_text_by_part(
    bundle: &Bundle,
    manifest: &HcdManifest,
    dirty_parts: &HashSet<String>,
    dirty_node_ids: &HashSet<String>,
) -> Result<HashMap<String, BTreeMap<u64, String>>, HcdError> {
    let mut output: HashMap<String, BTreeMap<u64, String>> = HashMap::new();
    if dirty_parts.is_empty() && dirty_node_ids.is_empty() {
        return Ok(output);
    }
    if dirty_parts.is_empty() || dirty_node_ids.is_empty() {
        return Err(HcdError::InvalidBundle(
            "revision dirty source parts and dirty node ids must either both be empty or both be present"
                .to_string(),
        ));
    }
    let mut found = HashSet::with_capacity(dirty_node_ids.len());
    for page_number in 0..manifest.index_page_count {
        let page = bundle.read_index_page(manifest, page_number)?;
        for descriptor in page.chunks {
            let source_map = bundle.read_map(&descriptor)?;
            if !source_map
                .entries
                .iter()
                .any(|entry| dirty_node_ids.contains(&entry.node_id))
            {
                continue;
            }
            let html = bundle.read_chunk(&descriptor)?;
            let html_nodes = extract_html_text_nodes(&html)?;
            for entry in source_map.entries {
                if !dirty_node_ids.contains(&entry.node_id) {
                    continue;
                }
                if !dirty_parts.contains(&entry.source.part) {
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
                let actual_hash = hcd_core::hash_bytes(text.as_bytes());
                if actual_hash != entry.node_hash {
                    return Err(HcdError::InvalidBundle(format!(
                        "node {} HTML hash {} does not match source-map hash {}",
                        entry.node_id, actual_hash, entry.node_hash
                    )));
                }
                found.insert(entry.node_id.clone());
                insert_entry(&mut output, entry, text)?;
            }
        }
    }
    if found.len() != dirty_node_ids.len() {
        let mut missing: Vec<&str> = dirty_node_ids
            .iter()
            .filter(|node_id| !found.contains(*node_id))
            .map(String::as_str)
            .collect();
        missing.sort_unstable();
        return Err(HcdError::InvalidBundle(format!(
            "revision references dirty HCD nodes that are missing from its index: {missing:?}"
        )));
    }
    for part in dirty_parts {
        if !output.contains_key(part) {
            return Err(HcdError::InvalidBundle(format!(
                "dirty source part {part} has no HCD text map"
            )));
        }
    }
    Ok(output)
}

fn insert_entry(
    output: &mut HashMap<String, BTreeMap<u64, String>>,
    entry: NodeMapEntry,
    text: &str,
) -> Result<(), HcdError> {
    let part = output.entry(entry.source.part.clone()).or_default();
    if let Some(previous) = part.insert(entry.source.text_ordinal, text.to_string()) {
        if previous != text {
            return Err(HcdError::InvalidBundle(format!(
                "source ordinal {} in {} maps to multiple values",
                entry.source.text_ordinal, entry.source.part
            )));
        }
    }
    Ok(())
}

fn rewrite_text_part(
    source: &mut dyn Read,
    output: impl Write,
    replacements: &BTreeMap<u64, String>,
) -> Result<(), HcdError> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut writer = Writer::new(output);
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut ordinal = 0u64;
    let mut replacing = false;
    let mut seen = BTreeSet::new();

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("source XML parse error: {error}")))?;
        match event {
            Event::Start(ref start) if local_name(start.name().as_ref()) == "t" => {
                ordinal += 1;
                if let Some(text) = replacements.get(&ordinal) {
                    let owned = start_with_space_preservation(start, text);
                    writer.write_event(Event::Start(owned))?;
                    if !text.is_empty() {
                        writer.write_event(Event::Text(BytesText::new(text)))?;
                    }
                    replacing = true;
                    seen.insert(ordinal);
                } else {
                    writer.write_event(event.into_owned())?;
                }
            }
            Event::Empty(ref empty) if local_name(empty.name().as_ref()) == "t" => {
                ordinal += 1;
                if let Some(text) = replacements.get(&ordinal) {
                    let owned = start_with_space_preservation(empty, text);
                    writer.write_event(Event::Start(owned))?;
                    if !text.is_empty() {
                        writer.write_event(Event::Text(BytesText::new(text)))?;
                    }
                    writer.write_event(Event::End(BytesEnd::new(String::from_utf8_lossy(
                        empty.name().as_ref(),
                    ))))?;
                    seen.insert(ordinal);
                } else {
                    writer.write_event(event.into_owned())?;
                }
            }
            Event::Text(_) | Event::CData(_) if replacing => {}
            Event::End(ref end) if local_name(end.name().as_ref()) == "t" && replacing => {
                writer.write_event(event.into_owned())?;
                replacing = false;
            }
            Event::Eof => break,
            _ => writer.write_event(event.into_owned())?,
        }
        buffer.clear();
    }
    if seen.len() != replacements.len() {
        let missing: Vec<u64> = replacements
            .keys()
            .filter(|ordinal| !seen.contains(ordinal))
            .copied()
            .collect();
        return Err(HcdError::InvalidBundle(format!(
            "source XML is missing mapped text ordinals {missing:?}"
        )));
    }
    Ok(())
}

fn start_with_space_preservation(start: &BytesStart<'_>, text: &str) -> BytesStart<'static> {
    let mut owned = start.to_owned();
    let preserve = text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace);
    let has_space = start
        .attributes()
        .with_checks(false)
        .filter_map(Result::ok)
        .any(|attribute| attribute.key.as_ref() == b"xml:space");
    if preserve && !has_space {
        owned.push_attribute(("xml:space", "preserve"));
    }
    owned
}

fn local_name(name: &[u8]) -> &str {
    let local = name
        .iter()
        .rposition(|byte| *byte == b':')
        .map(|index| &name[index + 1..])
        .unwrap_or(name);
    std::str::from_utf8(local).unwrap_or("")
}

fn safe_temp_name(part: &str) -> String {
    part.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn package_error(error: PackageError) -> HcdError {
    match error {
        PackageError::ResourceLimit(message) => HcdError::ResourceLimit(message),
        other => HcdError::InvalidBundle(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hcd_core::{
        hash_bytes, BundleWriter, ChunkSourceMap, HcdCapabilities, NodeMapEntry, SourceAnchor,
        SourceDescriptor,
    };

    #[test]
    fn text_rewriter_uses_unicode_text_and_preserves_spaces() {
        let source =
            br#"<?xml version="1.0"?><w:p xmlns:w="x"><w:r><w:t>old</w:t><w:t/></w:r></w:p>"#;
        let replacements = BTreeMap::from([(1, "甲😀乙".to_string()), (2, " padded ".to_string())]);
        let mut output = Vec::new();
        let mut input = source.as_slice();
        rewrite_text_part(&mut input, &mut output, &replacements).unwrap();
        let xml = String::from_utf8(output).unwrap();
        assert!(xml.contains("甲😀乙"));
        assert!(xml.contains("xml:space=\"preserve\""));
        assert!(xml.contains(" padded "));
    }

    #[test]
    fn export_collection_keeps_only_changed_nodes_from_a_dirty_part() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        writer.write_styles("").unwrap();
        let first_id = "n_00000000000000000000000000000001";
        let second_id = "n_00000000000000000000000000000002";
        let first_hash = hash_bytes(b"unchanged");
        let second_hash = hash_bytes(b"changed");
        let html = format!(
            "<p><span data-hcd-id=\"{first_id}\" data-hcd-node-hash=\"{first_hash}\">unchanged</span><span data-hcd-id=\"{second_id}\" data-hcd-node-hash=\"{second_hash}\">changed</span></p>"
        );
        let part = "word/document.xml";
        let map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: "c_00000000000000000000000000000001".to_string(),
            entries: vec![
                NodeMapEntry {
                    node_id: first_id.to_string(),
                    node_hash: first_hash,
                    source: test_anchor(part, 1),
                },
                NodeMapEntry {
                    node_id: second_id.to_string(),
                    node_hash: second_hash,
                    source: test_anchor(part, 2),
                },
            ],
        };
        writer
            .write_chunk(
                map.chunk_id.clone(),
                "body".to_string(),
                html,
                map,
                1,
                false,
            )
            .unwrap();
        let manifest = writer.finish(test_manifest()).unwrap();
        let bundle = Bundle::open(bundle_path).unwrap();
        let dirty_parts = HashSet::from([part.to_string()]);
        let dirty_nodes = HashSet::from([second_id.to_string()]);

        let replacements =
            collect_text_by_part(&bundle, &manifest, &dirty_parts, &dirty_nodes).unwrap();

        assert_eq!(replacements[part].len(), 1);
        assert_eq!(replacements[part][&2], "changed");
        assert!(!replacements[part].contains_key(&1));
    }

    fn test_anchor(part: &str, ordinal: u64) -> SourceAnchor {
        SourceAnchor {
            part: part.to_string(),
            text_ordinal: ordinal,
            paragraph_id: None,
            text_id: None,
            node_kind: "text".to_string(),
            editable: true,
        }
    }

    fn test_manifest() -> HcdManifest {
        HcdManifest {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            document_id: "test-document".to_string(),
            profile: "semantic-flow".to_string(),
            revision: 0,
            source: SourceDescriptor {
                format: "docx".to_string(),
                sha256: "0".repeat(64),
                size_bytes: 1,
            },
            root_hash: String::new(),
            annotation_root_hash: String::new(),
            annotation_href: None,
            index_prefix: String::new(),
            index_page_count: 0,
            chunk_count: 0,
            styles_href: "styles.css".to_string(),
            capabilities: HcdCapabilities::default(),
            fidelity: None,
            state: "IMPORTING".to_string(),
            warnings: Vec::new(),
        }
    }
}
