use crate::content_types::{ContentTypes, ContentTypesError};
use crate::rels::{Relationships, RelsError};
use std::collections::HashMap;
use std::io::{Read, Write};
use thiserror::Error;
use zip::read::ZipArchive;
use zip::write::{SimpleFileOptions, ZipWriter};

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

        let mut parts = HashMap::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let entry_path = entry.name().to_string();
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            parts.insert(entry_path, content);
        }

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

    /// Check if a part exists.
    pub fn has_part(&self, part_path: &str) -> bool {
        self.parts.contains_key(part_path)
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

        // Write all parts to a new ZIP file
        let tmp_path = format!("{}.new", self.file_path);
        let file = std::fs::File::create(&tmp_path)?;
        let mut writer = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        for (path, content) in &self.parts {
            writer.start_file(path, options)?;
            writer.write_all(content)?;
        }

        writer.finish()?;

        // Atomic replacement: rename temp to original
        std::fs::rename(&tmp_path, &self.file_path)?;

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
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

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
