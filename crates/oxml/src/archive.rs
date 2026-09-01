use crate::package::PackageError;
use crate::package::{
    DEFAULT_MAX_DOM_ELEMENTS, MAX_COMPRESSION_RATIO, MAX_RECURSION_DEPTH, MAX_UNCOMPRESSED_BYTES,
    MAX_ZIP_ENTRIES,
};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use zip::read::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

const RATIO_MIN_COMPRESSED_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub name: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub is_dir: bool,
}

pub struct StreamingOxmlArchive {
    archive: ZipArchive<File>,
    entries: Vec<ArchiveEntry>,
    names: HashSet<String>,
}

impl StreamingOxmlArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PackageError> {
        let file = File::open(path)?;
        let mut archive = ZipArchive::new(file)?;
        let entries = inspect_entries(&mut archive)?;
        let names = entries.iter().map(|entry| entry.name.clone()).collect();
        Ok(Self {
            archive,
            entries,
            names,
        })
    }

    pub fn entries(&self) -> &[ArchiveEntry] {
        &self.entries
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    pub fn entry(&self, name: &str) -> Option<&ArchiveEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    pub fn with_part<T>(
        &mut self,
        name: &str,
        callback: impl FnOnce(&mut dyn Read) -> Result<T, PackageError>,
    ) -> Result<T, PackageError> {
        let mut part = self
            .archive
            .by_name(name)
            .map_err(|error| PackageError::ReadPartError(format!("{name}: {error}")))?;
        callback(&mut part)
    }

    pub fn read_control_part(
        &mut self,
        name: &str,
        maximum_bytes: u64,
    ) -> Result<Vec<u8>, PackageError> {
        let entry_size = self
            .entry(name)
            .ok_or_else(|| PackageError::PartNotFound(name.to_string()))?
            .uncompressed_size;
        if entry_size > maximum_bytes {
            return Err(PackageError::ResourceLimit(format!(
                "control part {name} is {} bytes; maximum is {maximum_bytes}",
                entry_size
            )));
        }
        self.with_part(name, |reader| {
            let mut bytes = Vec::with_capacity(entry_size as usize);
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
    }

    /// Validate an exported OOXML package without materializing its XML parts.
    ///
    /// The ZIP central directory limits are enforced by [`Self::open`]. This
    /// second pass verifies required OPC parts and streams every XML/.rels
    /// entry through `quick_xml`, with the same depth/element budgets used by
    /// the in-memory package guard. DTDs are rejected because OOXML does not
    /// need them and accepting them would broaden the parser attack surface.
    pub fn validate_structure(&mut self, required_main_part: &str) -> Result<(), PackageError> {
        for required in ["[Content_Types].xml", "_rels/.rels", required_main_part] {
            if !self.contains(required) {
                return Err(PackageError::ReadPartError(format!(
                    "exported package is missing required part {required}"
                )));
            }
        }
        let xml_parts: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| {
                !entry.is_dir && (entry.name.ends_with(".xml") || entry.name.ends_with(".rels"))
            })
            .map(|entry| entry.name.clone())
            .collect();
        for part in xml_parts {
            self.with_part(&part, |source| validate_xml_part(&part, source))?;
        }
        Ok(())
    }
}

pub struct StreamingOxmlRewriter;

impl StreamingOxmlRewriter {
    pub fn rewrite(
        source: impl AsRef<Path>,
        target: impl AsRef<Path>,
        replacements: &HashMap<String, PathBuf>,
        required_main_part: &str,
    ) -> Result<Vec<String>, PackageError> {
        let source = source.as_ref();
        let target = target.as_ref();
        if target.exists() {
            return Err(PackageError::SaveError(format!(
                "target already exists: {}",
                target.display()
            )));
        }
        let file = File::open(source)?;
        let mut archive = ZipArchive::new(file)?;
        let entries = inspect_entries(&mut archive)?;
        let names: HashSet<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        for replacement in replacements.keys() {
            if !names.contains(replacement.as_str()) {
                return Err(PackageError::PartNotFound(replacement.clone()));
            }
        }

        let parent = target
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let temp = tempfile::Builder::new()
            .prefix(".officecli-hcd-")
            .suffix(".tmp")
            .tempfile_in(parent)?;
        let output = temp.reopen()?;
        let mut writer = ZipWriter::new(output);
        let mut changed = Vec::new();

        for (index, entry) in entries.iter().enumerate() {
            let name = entry.name.clone();
            if let Some(replacement) = replacements.get(&name) {
                let original = archive.by_index(index)?;
                let options = SimpleFileOptions::default()
                    .compression_method(original.compression())
                    .last_modified_time(original.last_modified().unwrap_or_default())
                    .unix_permissions(original.unix_mode().unwrap_or(0o644));
                drop(original);
                writer.start_file(&name, options)?;
                let mut replacement_file = File::open(replacement)?;
                std::io::copy(&mut replacement_file, &mut writer)?;
                changed.push(name);
            } else {
                let original = archive.by_index_raw(index)?;
                writer.raw_copy_file(original)?;
            }
        }
        writer.finish()?.sync_all()?;
        let mut candidate = StreamingOxmlArchive::open(temp.path())?;
        candidate.validate_structure(required_main_part)?;
        temp.persist(target)
            .map_err(|error| PackageError::SaveError(error.error.to_string()))?;
        Ok(changed)
    }
}

fn inspect_entries(archive: &mut ZipArchive<File>) -> Result<Vec<ArchiveEntry>, PackageError> {
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(PackageError::ResourceLimit(format!(
            "{} entries exceeds the {} entry limit",
            archive.len(),
            MAX_ZIP_ENTRIES
        )));
    }
    let mut uncompressed = 0u64;
    let mut compressed = 0u64;
    let mut entries = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        validate_entry_name(entry.name())?;
        uncompressed = uncompressed.saturating_add(entry.size());
        compressed = compressed.saturating_add(entry.compressed_size());
        entries.push(ArchiveEntry {
            name: entry.name().to_string(),
            compressed_size: entry.compressed_size(),
            uncompressed_size: entry.size(),
            is_dir: entry.is_dir(),
        });
    }
    if uncompressed > MAX_UNCOMPRESSED_BYTES {
        return Err(PackageError::ResourceLimit(format!(
            "uncompressed size exceeds {} bytes",
            MAX_UNCOMPRESSED_BYTES
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
    Ok(entries)
}

fn validate_entry_name(name: &str) -> Result<(), PackageError> {
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(PackageError::ResourceLimit(format!(
            "unsafe ZIP entry path: {name}"
        )));
    }
    Ok(())
}

fn validate_xml_part(part: &str, source: &mut dyn Read) -> Result<(), PackageError> {
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut depth = 0usize;
    let mut elements = 0usize;
    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            PackageError::ReadPartError(format!("exported XML part {part} is invalid: {error}"))
        })?;
        match event {
            Event::Start(_) => {
                elements = elements.saturating_add(1);
                depth = depth.saturating_add(1);
                check_xml_budget(part, elements, depth)?;
            }
            Event::Empty(_) => {
                elements = elements.saturating_add(1);
                check_xml_budget(part, elements, depth)?;
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| {
                    PackageError::ReadPartError(format!(
                        "exported XML part {part} has an unmatched end element"
                    ))
                })?;
            }
            Event::DocType(_) => {
                return Err(PackageError::ReadPartError(format!(
                    "exported XML part {part} contains a forbidden DOCTYPE"
                )));
            }
            Event::Eof => {
                if depth != 0 {
                    return Err(PackageError::ReadPartError(format!(
                        "exported XML part {part} ended at depth {depth}"
                    )));
                }
                break;
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(())
}

fn check_xml_budget(part: &str, elements: usize, depth: usize) -> Result<(), PackageError> {
    if elements > DEFAULT_MAX_DOM_ELEMENTS {
        return Err(PackageError::ResourceLimit(format!(
            "XML part {part} exceeds {DEFAULT_MAX_DOM_ELEMENTS} elements"
        )));
    }
    if depth > MAX_RECURSION_DEPTH {
        return Err(PackageError::ResourceLimit(format!(
            "XML part {part} exceeds maximum depth {MAX_RECURSION_DEPTH}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::{SimpleFileOptions, ZipWriter};

    #[test]
    fn unsafe_entry_names_are_rejected() {
        assert!(validate_entry_name("word/document.xml").is_ok());
        assert!(validate_entry_name("../document.xml").is_err());
        assert!(validate_entry_name("/absolute.xml").is_err());
    }

    #[test]
    fn streaming_structure_validation_rejects_malformed_xml_and_doctype() {
        for (name, document) in [
            ("malformed", b"<w:document><w:body></w:document>".as_slice()),
            (
                "doctype",
                b"<!DOCTYPE document><w:document xmlns:w=\"urn:test\"/>".as_slice(),
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join(format!("{name}.docx"));
            write_test_package(&path, document);
            let mut archive = StreamingOxmlArchive::open(&path).unwrap();
            assert!(archive.validate_structure("word/document.xml").is_err());
        }
    }

    #[test]
    fn streaming_structure_validation_accepts_a_minimal_package() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("valid.docx");
        write_test_package(
            &path,
            b"<w:document xmlns:w=\"urn:test\"><w:body/></w:document>",
        );
        let mut archive = StreamingOxmlArchive::open(&path).unwrap();
        archive.validate_structure("word/document.xml").unwrap();
    }

    #[test]
    fn validated_rewrite_does_not_publish_a_malformed_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.docx");
        let target = temp.path().join("target.docx");
        let replacement = temp.path().join("document.xml");
        write_test_package(
            &source,
            b"<w:document xmlns:w=\"urn:test\"><w:body/></w:document>",
        );
        std::fs::write(&replacement, b"<w:document><w:body></w:document>").unwrap();
        let replacements = HashMap::from([("word/document.xml".to_string(), replacement)]);

        let error =
            StreamingOxmlRewriter::rewrite(&source, &target, &replacements, "word/document.xml")
                .unwrap_err();

        assert!(error.to_string().contains("invalid"));
        assert!(!target.exists());
    }

    #[test]
    fn validated_rewrite_preserves_unmodified_compressed_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.docx");
        let target = temp.path().join("target.docx");
        let replacement = temp.path().join("document.xml");
        write_test_package(
            &source,
            b"<w:document xmlns:w=\"urn:test\"><w:body/></w:document>",
        );
        std::fs::write(
            &replacement,
            b"<w:document xmlns:w=\"urn:test\"><w:body><w:p/></w:body></w:document>",
        )
        .unwrap();
        let replacements = HashMap::from([("word/document.xml".to_string(), replacement)]);

        StreamingOxmlRewriter::rewrite(&source, &target, &replacements, "word/document.xml")
            .unwrap();

        for unchanged in ["[Content_Types].xml", "_rels/.rels"] {
            assert_eq!(
                raw_entry(&source, unchanged),
                raw_entry(&target, unchanged),
                "raw compressed payload changed for {unchanged}"
            );
        }
        assert_ne!(
            raw_entry(&source, "word/document.xml"),
            raw_entry(&target, "word/document.xml")
        );
    }

    fn write_test_package(path: &Path, document: &[u8]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", options).unwrap();
        zip.write_all(b"<Types/>").unwrap();
        zip.start_file("_rels/.rels", options).unwrap();
        zip.write_all(b"<Relationships/>").unwrap();
        zip.start_file("word/document.xml", options).unwrap();
        zip.write_all(document).unwrap();
        zip.finish().unwrap();
    }

    fn raw_entry(path: &Path, name: &str) -> Vec<u8> {
        let file = File::open(path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        let index = archive
            .file_names()
            .position(|candidate| candidate == name)
            .unwrap();
        let mut entry = archive.by_index_raw(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        bytes
    }
}
