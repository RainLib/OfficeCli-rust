use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let workspace_dir = manifest_dir.join("../..");
    generate_schema_entries(&workspace_dir);
    generate_skill_entries(&workspace_dir);
}

fn generate_schema_entries(workspace_dir: &Path) {
    let schema_root = workspace_dir.join("schemas/help");
    println!("cargo:rerun-if-changed={}", schema_root.display());

    let mut files = Vec::new();
    collect_json_files(&schema_root, &mut files);
    files.sort_by(|left, right| {
        canonical_name(&schema_root, left).cmp(&canonical_name(&schema_root, right))
    });
    assert!(!files.is_empty(), "schemas/help contains no JSON files");

    let mut generated = String::from("pub(crate) const SCHEMA_ENTRIES: &[(&str, &[u8])] = &[\n");
    for path in files {
        let canonical = canonical_name(&schema_root, &path);
        let absolute = fs::canonicalize(&path).unwrap_or(path);
        generated.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            canonical,
            absolute.to_string_lossy()
        ));
    }
    generated.push_str("];\n");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out_dir.join("schema_entries.rs"), generated).unwrap();
}

fn generate_skill_entries(workspace_dir: &Path) {
    let skill_root = workspace_dir.join("skills");
    println!("cargo:rerun-if-changed={}", skill_root.display());

    let mut files = Vec::new();
    collect_skill_files(&skill_root, &mut files);
    files.sort_by_key(|path| relative_name(&skill_root, path).to_lowercase());
    assert!(!files.is_empty(), "skills contains no bundled files");

    let mut generated = String::from("pub(crate) const SKILL_ENTRIES: &[(&str, &[u8])] = &[\n");
    for path in files {
        let relative = relative_name(&skill_root, &path);
        let absolute = fs::canonicalize(&path).unwrap_or(path);
        generated.push_str(&format!(
            "    ({:?}, include_bytes!({:?})),\n",
            relative,
            absolute.to_string_lossy()
        ));
    }
    generated.push_str("];\n");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::write(out_dir.join("skill_entries.rs"), generated).unwrap();
}

fn collect_json_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {}", directory.display(), error));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_json_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("json") {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }
}

fn collect_skill_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {}", directory.display(), error));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_skill_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) != Some("glb") {
            println!("cargo:rerun-if-changed={}", path.display());
            files.push(path);
        }
    }
}

fn relative_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn canonical_name(schema_root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(schema_root).unwrap();
    format!(
        "schemas/help/{}",
        relative.to_string_lossy().replace('\\', "/")
    )
    .to_lowercase()
}
