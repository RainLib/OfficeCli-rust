use crate::content_types::{ContentTypes, ContentTypesError};
use crate::rels::{Relationships, RelsError};
use std::collections::HashMap;
use std::io::{Read, Write};
use thiserror::Error;
use zip::read::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

/// Generous package limits mirrored from the C# `DocumentLimits` guard. They
/// are evaluated from the ZIP central directory before any entry is inflated.
pub const MAX_ZIP_ENTRIES: usize = 100_000;
pub const MAX_UNCOMPRESSED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_COMPRESSION_RATIO: u64 = 1_000;
pub const MAX_RECURSION_DEPTH: usize = 256;
pub const DEFAULT_MAX_DOM_ELEMENTS: usize = 3_000_000;
pub const ELEMENT_SCAN_PART_THRESHOLD: usize = 8 * 1024 * 1024;
const RATIO_MIN_COMPRESSED_BYTES: u64 = 64 * 1024;

#[derive(Debug, Error)]
pub enum PackageError {
    #[error("failed to open package: {0}")]
    OpenError(String),
    #[error("failed to read part: {0}")]
    ReadPartError(String),
    #[error("failed to write part: {0}")]
    WritePartError(String),
    #[error("part not found: {0}")]
    PartNotFound(String),
    #[error("failed to save package: {0}")]
    SaveError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("content types error: {0}")]
    ContentTypes(#[from] ContentTypesError),
    #[error("relationships error: {0}")]
    Rels(#[from] RelsError),
    #[error("package rejected by resource limit: {0}")]
    ResourceLimit(String),
}

/// Represents an OOXML package (ZIP file with XML parts).
pub struct OxmlPackage {
    /// Path to the original file
    file_path: String,
    /// Whether opened in editable mode
    editable: bool,
    /// Parts stored as (path -> XML/binary content)
    parts: HashMap<String, Vec<u8>>,
    /// Content types from [Content_Types].xml
    content_types: ContentTypes,
    /// Relationships from _rels/.rels
    root_rels: Relationships,
    /// Modified parts (for dirty tracking)
    dirty_parts: Vec<String>,
}

impl OxmlPackage {
    /// Create a new empty OOXML package for a given file path.
    pub fn create(path: &str) -> Self {
        Self {
            file_path: path.to_string(),
            editable: true,
            parts: HashMap::new(),
            content_types: ContentTypes::empty(),
            root_rels: Relationships::empty(),
            dirty_parts: Vec::new(),
        }
    }

    /// Open an OOXML package from a file path.
    pub fn open(path: &str, editable: bool) -> Result<Self, PackageError> {
        let file = std::fs::File::open(path)?;
        let mut archive = ZipArchive::new(file)?;

        guard_zip_resources(&mut archive)?;

        let mut parts = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let entry_path = entry.name().to_string();
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            parts.insert(entry_path, content);
        }

        guard_large_xml_structures(&parts)?;

        // Parse content types
        let content_types_xml = parts
            .get("[Content_Types].xml")
            .cloned()
            .unwrap_or_default();
        let content_types = ContentTypes::parse(&content_types_xml)?;

        // Parse root relationships
        let root_rels_xml = parts.get("_rels/.rels").cloned().unwrap_or_default();
        let root_rels = Relationships::parse(&root_rels_xml)?;

        Ok(Self {
            file_path: path.to_string(),
            editable,
            parts,
            content_types,
            root_rels,
            dirty_parts: Vec::new(),
        })
    }

    /// Read a part's content as raw bytes.
    pub fn read_part_bytes(&self, part_path: &str) -> Result<&Vec<u8>, PackageError> {
        self.parts
            .get(part_path)
            .ok_or_else(|| PackageError::PartNotFound(part_path.to_string()))
    }

    /// Read a part's content as a UTF-8 string (XML part).
    pub fn read_part_xml(&self, part_path: &str) -> Result<String, PackageError> {
        let bytes = self.read_part_bytes(part_path)?;
        Ok(String::from_utf8_lossy(bytes).to_string())
    }

    /// Write/update a part's content (marks it dirty for save).
    pub fn write_part(&mut self, part_path: &str, content: Vec<u8>) -> Result<(), PackageError> {
        if !self.editable {
            return Err(PackageError::WritePartError(
                "package opened in read-only mode".to_string(),
            ));
        }
        self.parts.insert(part_path.to_string(), content);
        if !self.dirty_parts.contains(&part_path.to_string()) {
            self.dirty_parts.push(part_path.to_string());
        }
        Ok(())
    }

    /// Write/update an XML part's content.
    pub fn write_part_xml(&mut self, part_path: &str, xml: &str) -> Result<(), PackageError> {
        self.write_part(part_path, xml.as_bytes().to_vec())
    }

    /// List all part paths in the package.
    pub fn list_parts(&self) -> Vec<&String> {
        self.parts.keys().collect()
    }

    /// Validate package-level invariants shared by all OOXML formats.
    pub fn validate(&self) -> Vec<handler_common::ValidationError> {
        crate::validate::validate_package(&self.parts)
    }

    /// Check if a part exists.
    pub fn has_part(&self, part_path: &str) -> bool {
        self.parts.contains_key(part_path)
    }

    /// Remove a part from the package.
    ///
    /// Callers are responsible for removing relationships and content-type
    /// overrides that reference the part.
    pub fn remove_part(&mut self, part_path: &str) -> Result<bool, PackageError> {
        if !self.editable {
            return Err(PackageError::WritePartError(
                "package opened in read-only mode".to_string(),
            ));
        }

        let removed = self.parts.remove(part_path).is_some();
        if removed && !self.dirty_parts.iter().any(|path| path == part_path) {
            self.dirty_parts.push(part_path.to_string());
        }
        Ok(removed)
    }

    /// Get the content types.
    pub fn content_types(&self) -> &ContentTypes {
        &self.content_types
    }

    /// Get the root relationships.
    pub fn root_rels(&self) -> &Relationships {
        &self.root_rels
    }

    /// Get relationship for a specific part.
    pub fn part_rels(&self, part_path: &str) -> Result<Relationships, PackageError> {
        // e.g. "word/document.xml" -> "word/_rels/document.xml.rels"
        let rels_path = if part_path.contains('/') {
            let last_slash = part_path.rfind('/').unwrap();
            format!(
                "{}_rels/{}.rels",
                &part_path[..last_slash + 1],
                &part_path[last_slash + 1..]
            )
        } else {
            format!("_rels/{}.rels", part_path)
        };
        if let Some(xml) = self.parts.get(&rels_path) {
            Relationships::parse(xml).map_err(|e| PackageError::ReadPartError(e.to_string()))
        } else {
            Ok(Relationships::empty())
        }
    }

    /// Resolve a relationship target to a part path.
    pub fn resolve_rel_target(&self, source_part: &str, target: &str) -> String {
        let raw = if let Some(stripped) = target.strip_prefix('/') {
            // Absolute target - strip leading slash
            stripped.to_string()
        } else {
            // Relative target - resolve against source part
            if source_part.contains('/') {
                let last_slash = source_part.rfind('/').unwrap();
                format!("{}{}", &source_part[..last_slash + 1], target)
            } else {
                target.to_string()
            }
        };

        // Normalize path (collapse '.' and '..')
        let mut parts = Vec::new();
        for component in raw.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                c => {
                    parts.push(c);
                }
            }
        }
        parts.join("/")
    }

    /// Save the package back to disk (all modified parts written).
    pub fn save(&mut self) -> Result<(), PackageError> {
        if !self.editable {
            return Err(PackageError::SaveError(
                "package opened in read-only mode".to_string(),
            ));
        }

        // Use a unique sibling temp file so concurrent commands cannot race on
        // the old predictable `<file>.new` name. Keeping it in the target
        // directory makes the final replacement a same-filesystem operation.
        let target = std::path::Path::new(&self.file_path);
        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let temp = tempfile::Builder::new()
            .prefix(".officecli-")
            .suffix(".tmp")
            .tempfile_in(parent)?;
        let file = temp.reopen()?;
        let mut writer = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (path, content) in &self.parts {
            writer.start_file(path, options)?;
            writer.write_all(content)?;
        }

        writer.finish()?;

        temp.persist(&self.file_path)
            .map_err(|error| PackageError::SaveError(error.error.to_string()))?;

        self.dirty_parts.clear();
        Ok(())
    }

    /// Get the file path.
    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    /// Add a new part to the package.
    pub fn add_part(&mut self, part_path: &str, content: &[u8]) {
        self.parts.insert(part_path.to_string(), content.to_vec());
        self.dirty_parts.push(part_path.to_string());
    }

    /// Save the package to a different file path (for create operations).
    pub fn save_as(&mut self, path: &str) -> Result<(), PackageError> {
        let file = std::fs::File::create(path)?;
        let mut writer = ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (part_path, content) in &self.parts {
            writer.start_file(part_path, options)?;
            writer.write_all(content)?;
        }

        writer.finish()?;
        self.file_path = path.to_string();
        self.dirty_parts.clear();
        Ok(())
    }
}

/// Reject entry-count, decompressed-size and compression-ratio bombs before
/// `read_to_end` can allocate attacker-controlled amounts of memory.
fn guard_zip_resources(archive: &mut ZipArchive<std::fs::File>) -> Result<(), PackageError> {
    let mut uncompressed = 0u64;
    let mut compressed = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        uncompressed = uncompressed.saturating_add(entry.size());
        compressed = compressed.saturating_add(entry.compressed_size());
    }

    validate_zip_resource_totals(archive.len(), uncompressed, compressed)
}

fn validate_zip_resource_totals(
    entries: usize,
    uncompressed: u64,
    compressed: u64,
) -> Result<(), PackageError> {
    if entries > MAX_ZIP_ENTRIES {
        return Err(PackageError::ResourceLimit(format!(
            "{} entries exceeds the {} entry limit",
            entries, MAX_ZIP_ENTRIES
        )));
    }
    if uncompressed > MAX_UNCOMPRESSED_BYTES {
        return Err(PackageError::ResourceLimit(format!(
            "uncompressed size exceeds the {} GiB limit",
            MAX_UNCOMPRESSED_BYTES / (1024 * 1024 * 1024)
        )));
    }
    if compressed > RATIO_MIN_COMPRESSED_BYTES
        && uncompressed / compressed.max(1) > MAX_COMPRESSION_RATIO
    {
        return Err(PackageError::ResourceLimit(format!(
            "compression ratio {}x exceeds the {}x limit",
            uncompressed / compressed.max(1),
            MAX_COMPRESSION_RATIO
        )));
    }
    Ok(())
}

/// Count XML start tags without constructing a DOM for suspiciously large
/// XML parts.  ZIP byte limits alone cannot prevent a dense worksheet from
/// expanding into millions of DOM nodes.  The environment override matches
/// the C# `OFFICECLI_MAX_DOM_ELEMENTS` escape hatch for exceptional workbooks.
fn guard_large_xml_structures(parts: &HashMap<String, Vec<u8>>) -> Result<(), PackageError> {
    let max_elements = std::env::var("OFFICECLI_MAX_DOM_ELEMENTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_DOM_ELEMENTS);

    for (part_path, content) in parts {
        if !is_xml_part(part_path) {
            continue;
        }
        // Dense element counting is necessary only for large XML parts. Depth
        // is cheap to scan and must be capped for every part: a tiny, deeply
        // nested document can still overflow recursive DOM walkers/renderers.
        let element_limit = if content.len() >= ELEMENT_SCAN_PART_THRESHOLD {
            max_elements
        } else {
            usize::MAX
        };
        validate_xml_structure_limits(content, element_limit, MAX_RECURSION_DEPTH).map_err(
            |reason| {
                PackageError::ResourceLimit(format!(
                    "XML part '{}' exceeds a structural limit: {}",
                    part_path, reason
                ))
            },
        )?;
    }
    Ok(())
}

fn is_xml_part(part_path: &str) -> bool {
    part_path.ends_with(".xml")
        || part_path.ends_with(".rels")
        || part_path == "[Content_Types].xml"
}

/// Lightweight XML tag scan.  It intentionally does not validate XML (the
/// normal XML parsers provide precise syntax errors); it only establishes
/// resource bounds before a format handler creates a recursive tree.
fn validate_xml_structure_limits(
    xml: &[u8],
    max_elements: usize,
    max_depth: usize,
) -> Result<(), String> {
    let mut index = 0;
    let mut elements = 0usize;
    let mut depth = 0usize;

    while index < xml.len() {
        let Some(relative) = xml[index..].iter().position(|byte| *byte == b'<') else {
            break;
        };
        index += relative;
        let next = *xml.get(index + 1).unwrap_or(&0);
        if next == b'!' {
            if xml[index..].starts_with(b"<!--") {
                let Some(end) = find_bytes(&xml[index + 4..], b"-->") else {
                    break;
                };
                index += end + 7;
                continue;
            }
            if xml[index..].starts_with(b"<![CDATA[") {
                let Some(end) = find_bytes(&xml[index + 9..], b"]]>") else {
                    break;
                };
                index += end + 12;
                continue;
            }
        }
        let Some(tag_end_relative) = find_tag_end(&xml[index + 1..]) else {
            break;
        };
        let tag_end = index + tag_end_relative + 1;

        if next == b'/' {
            depth = depth.saturating_sub(1);
        } else if next != b'?' && next != b'!' {
            elements += 1;
            if elements > max_elements {
                return Err(format!(
                    "{} elements exceeds the {} element limit",
                    elements, max_elements
                ));
            }
            let self_closing = xml[index..=tag_end]
                .iter()
                .rev()
                .skip(1)
                .find(|byte| !byte.is_ascii_whitespace())
                == Some(&b'/');
            if !self_closing {
                depth += 1;
                if depth > max_depth {
                    return Err(format!(
                        "nesting depth {} exceeds the {} depth limit",
                        depth, max_depth
                    ));
                }
            }
        }
        index = tag_end + 1;
    }
    Ok(())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Find the end of a tag while respecting quoted attribute values.
fn find_tag_end(bytes: &[u8]) -> Option<usize> {
    let mut quote = None;
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(byte, b'\'' | b'\"') {
            if quote == Some(*byte) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(*byte);
            }
        } else if *byte == b'>' && quote.is_none() {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod resource_limit_tests {
    use super::*;

    #[test]
    fn accepts_normal_zip_metadata() {
        assert!(validate_zip_resource_totals(3, 50_000, 10_000).is_ok());
    }

    #[test]
    fn rejects_excessive_entry_count() {
        assert!(matches!(
            validate_zip_resource_totals(MAX_ZIP_ENTRIES + 1, 0, 0),
            Err(PackageError::ResourceLimit(_))
        ));
    }

    #[test]
    fn rejects_excessive_uncompressed_size_and_ratio() {
        assert!(matches!(
            validate_zip_resource_totals(1, MAX_UNCOMPRESSED_BYTES + 1, 1),
            Err(PackageError::ResourceLimit(_))
        ));
        assert!(matches!(
            validate_zip_resource_totals(1, 1001 * 65_537, 65_537),
            Err(PackageError::ResourceLimit(_))
        ));
    }

    #[test]
    fn rejects_excessive_xml_elements_and_depth() {
        assert!(validate_xml_structure_limits(b"<root><a/><b/></root>", 2, 10).is_err());
        assert!(validate_xml_structure_limits(b"<a><b><c><d/></c></b></a>", 10, 2).is_err());
        assert!(validate_xml_structure_limits(b"<a note=\"1 > 0\"><b/></a>", 10, 2).is_ok());
    }

    #[test]
    fn rejects_deep_small_xml_parts_before_handler_recursion() {
        let xml = format!(
            "{}{}",
            "<n>".repeat(MAX_RECURSION_DEPTH + 1),
            "</n>".repeat(MAX_RECURSION_DEPTH + 1)
        );
        let parts = HashMap::from([("word/document.xml".to_string(), xml.into_bytes())]);
        assert!(matches!(
            guard_large_xml_structures(&parts),
            Err(PackageError::ResourceLimit(_))
        ));
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn save_replaces_existing_package_with_a_unique_sibling_tempfile() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("document.docx");
        let path_text = path.to_string_lossy().to_string();
        let mut created = OxmlPackage::create(&path_text);
        created.add_part(
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
        );
        created.add_part(
            "_rels/.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#,
        );
        created.add_part("word/document.xml", b"<document>before</document>");
        created.save_as(&path_text).unwrap();

        let mut opened = OxmlPackage::open(&path_text, true).unwrap();
        opened
            .write_part_xml("word/document.xml", "<document>after</document>")
            .unwrap();
        opened.save().unwrap();

        let reopened = OxmlPackage::open(&path_text, false).unwrap();
        assert_eq!(
            reopened.read_part_xml("word/document.xml").unwrap(),
            "<document>after</document>"
        );
        assert!(!directory.path().join("document.docx.new").exists());
    }
}
