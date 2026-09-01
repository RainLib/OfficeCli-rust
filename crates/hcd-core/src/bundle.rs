use crate::hash::{hash_bytes, node_bloom};
use crate::{
    AssetDescriptor, ChunkDescriptor, ChunkIndexPage, ChunkSourceMap, HcdError, HcdManifest,
    RevisionRecord, HCD_SCHEMA_VERSION, MAX_CHUNK_BYTES, MAX_CONTROL_PART_BYTES,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const INDEX_PAGE_SIZE: usize = 128;

#[derive(Debug, Clone)]
pub struct Bundle {
    root: PathBuf,
}

impl Bundle {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, HcdError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_dir() {
            return Err(HcdError::InvalidBundle(format!(
                "bundle directory does not exist: {}",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> Result<HcdManifest, HcdError> {
        read_json_bounded(
            &self.resolve_href("manifest.json")?,
            MAX_CONTROL_PART_BYTES,
            "manifest",
        )
    }

    pub fn read_index_page(
        &self,
        manifest: &HcdManifest,
        page: usize,
    ) -> Result<ChunkIndexPage, HcdError> {
        let href = format!("{}/{page:06}.json", manifest.index_prefix);
        read_json_bounded(
            &self.resolve_href(&href)?,
            MAX_CONTROL_PART_BYTES,
            "index page",
        )
    }

    pub fn read_map(&self, descriptor: &ChunkDescriptor) -> Result<ChunkSourceMap, HcdError> {
        read_json_bounded(
            &self.resolve_href(&descriptor.map_href)?,
            MAX_CONTROL_PART_BYTES,
            "source map",
        )
    }

    pub fn read_asset_index(&self) -> Result<Vec<AssetDescriptor>, HcdError> {
        read_json_bounded(
            &self.resolve_href("assets/index.json")?,
            MAX_CONTROL_PART_BYTES,
            "asset index",
        )
    }

    pub fn read_chunk(&self, descriptor: &ChunkDescriptor) -> Result<String, HcdError> {
        read_text_bounded(
            &self.resolve_href(&descriptor.html_href)?,
            MAX_CHUNK_BYTES as u64,
            "HTML chunk",
        )
    }

    pub fn resolve_href(&self, href: &str) -> Result<PathBuf, HcdError> {
        let relative = safe_relative_path(href)?;
        let mut resolved = self.root.clone();
        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(HcdError::InvalidBundle(format!(
                    "unsafe relative path: {href}"
                )));
            };
            resolved.push(component);
            match fs::symlink_metadata(&resolved) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(HcdError::InvalidBundle(format!(
                        "bundle href traverses a symbolic link: {href}"
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(HcdError::Io(error)),
            }
        }
        Ok(resolved)
    }

    pub fn revision(&self, revision: u64) -> Result<RevisionRecord, HcdError> {
        if revision > crate::MAX_REVISION {
            return Err(HcdError::ResourceLimit(format!(
                "revision {revision} exceeds the maximum {}",
                crate::MAX_REVISION
            )));
        }
        read_json_bounded(
            &self.resolve_href(&format!("revisions/{revision:020}.json"))?,
            MAX_CONTROL_PART_BYTES,
            "revision record",
        )
    }

    pub fn write_json_object<T: serde::Serialize>(
        &self,
        directory: &str,
        value: &T,
    ) -> Result<(String, String), HcdError> {
        let bytes = serde_json::to_vec(value)?;
        let hash = hash_bytes(&bytes);
        let href = format!("{directory}/sha256/{hash}.json");
        write_content_addressed(&self.root, &href, &bytes)?;
        Ok((href, hash))
    }

    pub fn write_chunk_object(&self, html: &str) -> Result<(String, String), HcdError> {
        if html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "HTML chunk is {} bytes; maximum is {}",
                html.len(),
                MAX_CHUNK_BYTES
            )));
        }
        let hash = hash_bytes(html.as_bytes());
        let href = format!("chunks/sha256/{hash}.html");
        write_content_addressed(&self.root, &href, html.as_bytes())?;
        Ok((href, hash))
    }

    pub fn write_revision(&self, record: &RevisionRecord) -> Result<(), HcdError> {
        if record.revision > crate::MAX_REVISION {
            return Err(HcdError::ResourceLimit(format!(
                "revision {} exceeds the maximum {}",
                record.revision,
                crate::MAX_REVISION
            )));
        }
        atomic_write_json(
            &self
                .root
                .join("revisions")
                .join(format!("{:020}.json", record.revision)),
            record,
        )
    }

    pub fn write_manifest(&self, manifest: &HcdManifest) -> Result<(), HcdError> {
        atomic_write_json(&self.root.join("manifest.json"), manifest)
    }

    pub(crate) fn acquire_write_lock(&self) -> Result<BundleWriteGuard, HcdError> {
        let path = self.root.join(".hcd-write-lock");
        let mut lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(HcdError::Io)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(HcdError::RevisionConflict(
                    "another local HCD mutation is already in progress".to_string(),
                ));
            }
            Err(fs::TryLockError::Error(error)) => return Err(HcdError::Io(error)),
        }
        lock.set_len(0)?;
        writeln!(
            lock,
            "pid={} epochMs={}",
            std::process::id(),
            now_epoch_ms()
        )?;
        lock.sync_all()?;
        Ok(BundleWriteGuard { _lock: lock })
    }
}

pub(crate) struct BundleWriteGuard {
    _lock: fs::File,
}

pub struct BundleWriter {
    root: PathBuf,
    pending: Vec<ChunkDescriptor>,
    page_count: usize,
    chunk_count: usize,
    root_hasher: Sha256,
    finished: bool,
}

impl BundleWriter {
    pub fn create(root: impl AsRef<Path>) -> Result<Self, HcdError> {
        let root = root.as_ref().to_path_buf();
        if root.exists() {
            return Err(HcdError::InvalidBundle(format!(
                "output already exists: {}",
                root.display()
            )));
        }
        fs::create_dir_all(&root)?;
        for directory in [
            "indexes/rev-00000000000000000000",
            "chunks/sha256",
            "maps/sha256",
            "annotations/sha256",
            "assets/sha256",
            "revisions",
        ] {
            fs::create_dir_all(root.join(directory))?;
        }
        fs::write(root.join("assets/index.json"), b"[]")?;
        fs::write(root.join(".importing"), b"hcd/1\n")?;
        Ok(Self {
            root,
            pending: Vec::with_capacity(INDEX_PAGE_SIZE),
            page_count: 0,
            chunk_count: 0,
            root_hasher: Sha256::new(),
            finished: false,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_styles(&self, styles: &str) -> Result<(), HcdError> {
        crate::html::validate_css_text(styles)?;
        atomic_write(&self.root.join("styles.css"), styles.as_bytes())
    }

    pub fn write_chunk(
        &mut self,
        chunk_id: String,
        region: String,
        html: String,
        source_map: ChunkSourceMap,
        block_count: usize,
        continuation: bool,
    ) -> Result<ChunkDescriptor, HcdError> {
        if html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: chunk {} is {} bytes; maximum is {}",
                chunk_id,
                html.len(),
                MAX_CHUNK_BYTES
            )));
        }
        if source_map.chunk_id != chunk_id {
            return Err(HcdError::InvalidBundle(format!(
                "source map chunk id {} does not match {}",
                source_map.chunk_id, chunk_id
            )));
        }

        let html_hash = hash_bytes(html.as_bytes());
        let html_href = format!("chunks/sha256/{html_hash}.html");
        write_content_addressed(&self.root, &html_href, html.as_bytes())?;

        let map_bytes = serde_json::to_vec(&source_map)?;
        let map_hash = hash_bytes(&map_bytes);
        let map_href = format!("maps/sha256/{map_hash}.json");
        write_content_addressed(&self.root, &map_href, &map_bytes)?;

        let html_nodes = crate::extract_html_text_nodes(&html)?;
        if html_nodes.len() != source_map.entries.len() {
            return Err(HcdError::InvalidBundle(format!(
                "chunk {} contains {} canonical text nodes but its map contains {}",
                chunk_id,
                html_nodes.len(),
                source_map.entries.len()
            )));
        }
        for entry in &source_map.entries {
            let text = html_nodes.get(&entry.node_id).ok_or_else(|| {
                HcdError::InvalidBundle(format!(
                    "mapped node {} is missing from canonical HTML",
                    entry.node_id
                ))
            })?;
            let actual_hash = hash_bytes(text.as_bytes());
            if actual_hash != entry.node_hash {
                return Err(HcdError::InvalidBundle(format!(
                    "mapped node {} expected hash {}, actual {}",
                    entry.node_id, entry.node_hash, actual_hash
                )));
            }
        }

        let descriptor = ChunkDescriptor {
            sequence: self.chunk_count,
            chunk_id,
            region,
            html_href,
            html_hash,
            map_href,
            map_hash,
            byte_length: html.len() as u64,
            block_count,
            node_count: source_map.entries.len(),
            text_chars: html_nodes.values().map(|text| text.chars().count()).sum(),
            node_bloom: node_bloom(
                source_map
                    .entries
                    .iter()
                    .map(|entry| entry.node_id.as_str()),
            ),
            first_node_id: source_map
                .entries
                .first()
                .map(|entry| entry.node_id.clone()),
            last_node_id: source_map.entries.last().map(|entry| entry.node_id.clone()),
            continuation,
        };
        self.add_descriptor(descriptor.clone())?;
        Ok(descriptor)
    }

    pub fn add_descriptor(&mut self, descriptor: ChunkDescriptor) -> Result<(), HcdError> {
        if descriptor.sequence != self.chunk_count {
            return Err(HcdError::InvalidBundle(format!(
                "chunk sequence {} is not expected {}",
                descriptor.sequence, self.chunk_count
            )));
        }
        hash_descriptor(&mut self.root_hasher, &descriptor);
        self.pending.push(descriptor);
        self.chunk_count += 1;
        if self.pending.len() == INDEX_PAGE_SIZE {
            self.flush_index_page()?;
        }
        Ok(())
    }

    pub fn write_asset_from_reader(
        &self,
        extension: &str,
        reader: &mut (impl std::io::Read + ?Sized),
    ) -> Result<(String, String, u64), HcdError> {
        let temp = tempfile::Builder::new()
            .prefix(".hcd-asset-")
            .tempfile_in(self.root.join("assets/sha256"))?;
        let mut file = temp.reopen()?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            file.write_all(&buffer[..count])?;
            hasher.update(&buffer[..count]);
            total += count as u64;
        }
        file.sync_all()?;
        let hash = encode_digest(hasher.finalize().as_slice());
        let clean_extension = extension
            .trim_start_matches('.')
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>();
        let file_name = if clean_extension.is_empty() {
            hash.clone()
        } else {
            format!("{hash}.{clean_extension}")
        };
        let relative = format!("assets/sha256/{file_name}");
        let target = self.root.join(&relative);
        if target.exists() {
            let existing_hash = crate::hash_file(&target)?;
            if existing_hash != hash {
                return Err(HcdError::InvalidBundle(format!(
                    "content-addressed asset {} contains hash {}",
                    target.display(),
                    existing_hash
                )));
            }
            drop(temp);
        } else {
            temp.persist(&target)
                .map_err(|error| HcdError::Io(error.error))?;
        }
        Ok((relative, hash, total))
    }

    pub fn finish(mut self, mut manifest: HcdManifest) -> Result<HcdManifest, HcdError> {
        self.flush_index_page()?;
        manifest.schema_version = HCD_SCHEMA_VERSION.to_string();
        manifest.revision = 0;
        let bundle = Bundle {
            root: self.root.clone(),
        };
        manifest.root_hash = finalize_root_hash(&bundle, self.root_hasher.clone())?;
        manifest.annotation_root_hash = hash_bytes(b"[]");
        manifest.annotation_href = None;
        manifest.index_prefix = "indexes/rev-00000000000000000000".to_string();
        manifest.index_page_count = self.page_count;
        manifest.chunk_count = self.chunk_count;
        manifest.styles_href = "styles.css".to_string();
        manifest.state = "COMPLETE".to_string();

        let record = RevisionRecord {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            document_id: manifest.document_id.clone(),
            revision: 0,
            parent_revision: None,
            patch_id: None,
            patch_hash: None,
            patch_base_revision: None,
            root_hash: manifest.root_hash.clone(),
            annotation_root_hash: manifest.annotation_root_hash.clone(),
            index_prefix: manifest.index_prefix.clone(),
            created_at_epoch_ms: now_epoch_ms(),
            dirty_node_ids: Vec::new(),
            dirty_chunk_ids: Vec::new(),
            dirty_source_parts: Vec::new(),
        };
        atomic_write_json(
            &self.root.join("revisions/00000000000000000000.json"),
            &record,
        )?;
        atomic_write_json(&self.root.join("manifest.json"), &manifest)?;
        let importing = self.root.join(".importing");
        if importing.exists() {
            fs::remove_file(importing)?;
        }
        self.finished = true;
        Ok(manifest)
    }

    fn flush_index_page(&mut self) -> Result<(), HcdError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let page = ChunkIndexPage {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            revision: 0,
            page: self.page_count,
            chunks: std::mem::take(&mut self.pending),
        };
        atomic_write_json(
            &self
                .root
                .join("indexes/rev-00000000000000000000")
                .join(format!("{:06}.json", self.page_count)),
            &page,
        )?;
        self.page_count += 1;
        Ok(())
    }
}

impl Drop for BundleWriter {
    fn drop(&mut self) {
        if !self.finished {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

pub(crate) fn read_json_bounded<T: serde::de::DeserializeOwned>(
    path: &Path,
    maximum_bytes: u64,
    kind: &str,
) -> Result<T, HcdError> {
    let bytes = read_bytes_bounded(path, maximum_bytes, kind)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn read_text_bounded(
    path: &Path,
    maximum_bytes: u64,
    kind: &str,
) -> Result<String, HcdError> {
    let bytes = read_bytes_bounded(path, maximum_bytes, kind)?;
    String::from_utf8(bytes)
        .map_err(|error| HcdError::InvalidBundle(format!("{kind} is not UTF-8: {error}")))
}

fn read_bytes_bounded(path: &Path, maximum_bytes: u64, kind: &str) -> Result<Vec<u8>, HcdError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > maximum_bytes {
        return Err(HcdError::ResourceLimit(format!(
            "{kind} {} is {} bytes; maximum is {maximum_bytes}",
            path.display(),
            metadata.len()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len().min(maximum_bytes) as usize);
    fs::File::open(path)?
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(HcdError::ResourceLimit(format!(
            "{kind} {} grew beyond the {maximum_bytes} byte limit while reading",
            path.display()
        )));
    }
    Ok(bytes)
}

pub(crate) fn atomic_write_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), HcdError> {
    atomic_write(path, &serde_json::to_vec(value)?)
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), HcdError> {
    let parent = path.parent().ok_or_else(|| {
        HcdError::InvalidBundle(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".hcd-")
        .tempfile_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| HcdError::Io(error.error))?;
    Ok(())
}

pub(crate) fn write_content_addressed(
    root: &Path,
    href: &str,
    bytes: &[u8],
) -> Result<(), HcdError> {
    let relative = safe_relative_path(href)?;
    let path = root.join(relative);
    if path.exists() {
        let expected_hash = hash_bytes(bytes);
        let existing_hash = crate::hash_file(&path)?;
        if existing_hash != expected_hash {
            return Err(HcdError::InvalidBundle(format!(
                "content-addressed object {} contains hash {} instead of {}",
                path.display(),
                existing_hash,
                expected_hash
            )));
        }
        return Ok(());
    }
    atomic_write(&path, bytes)
}

pub(crate) fn safe_relative_path(value: &str) -> Result<PathBuf, HcdError> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(HcdError::InvalidBundle(format!(
            "unsafe relative path: {value}"
        )));
    }
    Ok(path.to_path_buf())
}

pub(crate) fn hash_descriptor(hasher: &mut Sha256, descriptor: &ChunkDescriptor) {
    let value =
        serde_json::to_vec(descriptor).expect("ChunkDescriptor serialization is infallible");
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

/// Finish the revision body root from canonical HTML/map descriptors plus the
/// immutable rendering inputs shared by every revision. Annotations are
/// intentionally excluded and use `annotationRootHash` instead.
pub(crate) fn finalize_root_hash(
    bundle: &Bundle,
    descriptor_hasher: Sha256,
) -> Result<String, HcdError> {
    let descriptor_digest = descriptor_hasher.finalize();
    let styles = crate::hash::hash_file(&bundle.resolve_href("styles.css")?)?;
    let assets = crate::hash::hash_file(&bundle.resolve_href("assets/index.json")?)?;
    let mut root = Sha256::new();
    root.update(b"officecli-hcd-body-root/1\0");
    hash_root_component(&mut root, "chunks-and-maps", descriptor_digest.as_slice());
    hash_root_component(&mut root, "styles.css", styles.as_bytes());
    hash_root_component(&mut root, "assets/index.json", assets.as_bytes());
    Ok(encode_digest(root.finalize().as_slice()))
}

fn hash_root_component(hasher: &mut Sha256, name: &str, value: &[u8]) {
    hasher.update((name.len() as u64).to_be_bytes());
    hasher.update(name.as_bytes());
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

pub(crate) fn encode_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub(crate) fn now_epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HcdCapabilities, SourceDescriptor};

    #[test]
    fn safe_paths_reject_parent_components() {
        assert!(safe_relative_path("chunks/a.html").is_ok());
        assert!(safe_relative_path("../manifest.json").is_err());
        assert!(safe_relative_path("/tmp/file").is_err());
    }

    #[test]
    fn empty_bundle_finishes_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("doc.hcd");
        let writer = BundleWriter::create(&root).unwrap();
        writer.write_styles("article { color: #000; }").unwrap();
        let manifest = HcdManifest {
            schema_version: String::new(),
            document_id: "doc-1".to_string(),
            profile: "semantic-flow".to_string(),
            revision: 99,
            source: SourceDescriptor {
                format: "docx".to_string(),
                sha256: "00".repeat(32),
                size_bytes: 1,
            },
            root_hash: String::new(),
            annotation_root_hash: String::new(),
            annotation_href: None,
            index_prefix: String::new(),
            index_page_count: 0,
            chunk_count: 0,
            styles_href: String::new(),
            capabilities: HcdCapabilities::default(),
            fidelity: None,
            state: String::new(),
            warnings: Vec::new(),
        };
        writer.finish(manifest).unwrap();
        assert!(root.join("manifest.json").exists());
        assert!(!root.join(".importing").exists());
    }

    #[test]
    fn body_root_covers_styles_and_the_asset_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("doc.hcd");
        let writer = BundleWriter::create(&root).unwrap();
        writer.write_styles("article { color: #000; }").unwrap();
        writer.finish(test_manifest()).unwrap();
        let bundle = Bundle::open(&root).unwrap();
        assert!(crate::validate_bundle(&bundle).unwrap().valid);

        fs::write(root.join("styles.css"), b"article { color: #111; }").unwrap();
        let report = crate::validate_bundle(&bundle).unwrap();
        assert!(!report.valid);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "ROOT_HASH_MISMATCH"));

        writer_for_root_test(temp.path().join("assets.hcd"), b"[ ]");
    }

    fn writer_for_root_test(root: PathBuf, asset_index: &[u8]) {
        let writer = BundleWriter::create(&root).unwrap();
        writer.write_styles("article { color: #000; }").unwrap();
        fs::write(root.join("assets/index.json"), asset_index).unwrap();
        writer.finish(test_manifest()).unwrap();
        let bundle = Bundle::open(&root).unwrap();
        assert!(crate::validate_bundle(&bundle).unwrap().valid);
        fs::write(root.join("assets/index.json"), b"[]").unwrap();
        let report = crate::validate_bundle(&bundle).unwrap();
        assert!(!report.valid);
        assert!(report
            .issues
            .iter()
            .any(|issue| issue.code == "ROOT_HASH_MISMATCH"));
    }

    fn test_manifest() -> HcdManifest {
        HcdManifest {
            schema_version: String::new(),
            document_id: "doc-1".to_string(),
            profile: "semantic-flow".to_string(),
            revision: 99,
            source: SourceDescriptor {
                format: "docx".to_string(),
                sha256: "00".repeat(32),
                size_bytes: 1,
            },
            root_hash: String::new(),
            annotation_root_hash: String::new(),
            annotation_href: None,
            index_prefix: String::new(),
            index_page_count: 0,
            chunk_count: 0,
            styles_href: String::new(),
            capabilities: HcdCapabilities::default(),
            fidelity: None,
            state: String::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn bounded_reader_rejects_a_control_object_before_full_buffering() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("oversized.json");
        fs::write(&path, vec![b'x'; 33]).unwrap();
        let error =
            read_json_bounded::<serde_json::Value>(&path, 32, "test control object").unwrap_err();
        assert!(matches!(error, HcdError::ResourceLimit(_)));
        assert!(error.to_string().contains("maximum is 32"));
    }

    #[cfg(unix)]
    #[test]
    fn bundle_hrefs_cannot_escape_through_symbolic_links() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bundle");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("assets")).unwrap();

        let bundle = Bundle::open(&root).unwrap();
        let error = bundle.resolve_href("assets/index.json").unwrap_err();
        assert!(error.to_string().contains("symbolic link"));
    }

    #[test]
    fn local_bundle_mutations_are_serialized() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("bundle");
        fs::create_dir_all(&root).unwrap();
        let bundle = Bundle::open(&root).unwrap();

        let first = bundle.acquire_write_lock().unwrap();
        assert!(matches!(
            bundle.acquire_write_lock(),
            Err(HcdError::RevisionConflict(_))
        ));
        drop(first);
        assert!(bundle.acquire_write_lock().is_ok());
    }

    #[test]
    fn revision_reads_enforce_the_chain_limit_before_touching_disk() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = Bundle::open(temp.path()).unwrap();
        let error = bundle.revision(crate::MAX_REVISION + 1).unwrap_err();
        assert!(matches!(error, HcdError::ResourceLimit(_)));
    }
}
