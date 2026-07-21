use clap::Args;
use handler_common::HandlerError;
use std::collections::BTreeMap;

include!(concat!(env!("OUT_DIR"), "/skill_entries.rs"));

const SKILLS: &[(&str, &str)] = &[
    ("pptx", "officecli-pptx"),
    ("word", "officecli-docx"),
    ("excel", "officecli-xlsx"),
    ("morph-ppt", "morph-ppt"),
    ("morph-ppt-3d", "morph-ppt-3d"),
    ("pitch-deck", "officecli-pitch-deck"),
    ("academic-paper", "officecli-academic-paper"),
    ("data-dashboard", "officecli-data-dashboard"),
    ("financial-model", "officecli-financial-model"),
    ("word-form", "officecli-word-form"),
];

const BINARY_EXTENSIONS: &[&str] = &[
    "pptx", "docx", "xlsx", "png", "jpg", "jpeg", "gif", "webp", "glb", "pdf", "zip", "ico",
];

/// Read a bundled workflow skill without installing it.
#[derive(Args)]
pub struct LoadSkillCommand {
    /// Skill name. Omit to list available skills and their routing descriptions.
    pub name: Option<String>,

    /// Read one bundled reference file relative to the skill directory.
    #[arg(long, value_name = "RELPATH", requires = "name")]
    pub path: Option<String>,
}

pub fn handle_load_skill(cmd: LoadSkillCommand) -> Result<String, HandlerError> {
    let output = match (cmd.name.as_deref(), cmd.path.as_deref()) {
        (None, None) => build_skill_catalog()?,
        (Some(name), None) => load_skill_content(name)?,
        (Some(name), Some(path)) => load_skill_file(name, path)?,
        (None, Some(_)) => unreachable!("clap requires a skill name when --path is present"),
    };

    // The CLI's common output path appends one newline. Remove only trailing
    // newlines here so catalog and file reads do not gain a second blank line.
    Ok(output.trim_end_matches('\n').to_string())
}

fn build_skill_catalog() -> Result<String, HandlerError> {
    let mut output = String::from(
        "# officecli skills\n\n\
Workflow guides for building documents. Match the triggers below, then:\n\
- `load_skill <name>` — the skill's full SKILL.md + a manifest of its bundled reference files\n\
- `load_skill <name> --path <relpath>` — one bundled reference file\n\n",
    );

    for (name, folder) in SKILLS {
        let skill = embedded_text(&format!("{folder}/SKILL.md")).ok_or_else(|| {
            HandlerError::OperationFailed(format!("Embedded SKILL.md not found for '{name}'"))
        })?;
        let description = full_description(skill);
        output.push_str(&format!(
            "## {name}\n{}\n\n",
            if description.is_empty() {
                "(no description)"
            } else {
                description
            }
        ));
    }

    Ok(output.trim_end().to_string() + "\n")
}

fn load_skill_content(skill_name: &str) -> Result<String, HandlerError> {
    let folder = skill_folder(skill_name)?;
    let content = embedded_text(&format!("{folder}/SKILL.md")).ok_or_else(|| {
        HandlerError::OperationFailed(format!("Embedded SKILL.md not found for '{skill_name}'"))
    })?;
    Ok(strip_setup_section(content) + &build_reference_manifest(skill_name, folder))
}

fn load_skill_file(skill_name: &str, relative_path: &str) -> Result<String, HandlerError> {
    let folder = skill_folder(skill_name)?;
    let relative = relative_path.replace('\\', "/");
    let relative = relative.trim_start_matches('/');

    if relative.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "path is empty — pass a relative skill file, e.g. reference/decision-rules.md"
                .to_string(),
        ));
    }
    if relative
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(HandlerError::InvalidArgument(format!(
            "Invalid skill file path: {relative_path}"
        )));
    }
    if is_binary_path(relative) {
        return Err(HandlerError::InvalidArgument(format!(
            "'{relative}' is a binary asset and cannot be served over the text channel. \
Install the skill to get it on disk: officecli skills install {skill_name}"
        )));
    }

    embedded_text(&format!("{folder}/{relative}")).map(str::to_string).ok_or_else(|| {
        HandlerError::InvalidArgument(format!(
            "Skill file not found: {relative}. List available files via the manifest at the end of: \
officecli load_skill {skill_name}"
        ))
    })
}

fn skill_folder(skill_name: &str) -> Result<&'static str, HandlerError> {
    SKILLS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(skill_name))
        .map(|(_, folder)| *folder)
        .ok_or_else(|| {
            HandlerError::InvalidArgument(format!(
                "Unknown skill: {skill_name}. Available: {}",
                known_skills_list()
            ))
        })
}

fn known_skills_list() -> String {
    let mut names: Vec<&str> = SKILLS.iter().map(|(name, _)| *name).collect();
    names.sort_unstable_by_key(|name| name.to_ascii_lowercase());
    names.join(", ")
}

fn embedded_text(path: &str) -> Option<&'static str> {
    let bytes = SKILL_ENTRIES
        .iter()
        .find(|(entry_path, _)| *entry_path == path)
        .map(|(_, bytes)| *bytes)?;
    std::str::from_utf8(bytes).ok()
}

fn list_skill_files(folder: &str) -> Vec<&'static str> {
    let prefix = format!("{folder}/");
    let mut files: Vec<&str> = SKILL_ENTRIES
        .iter()
        .filter_map(|(path, _)| path.strip_prefix(&prefix))
        .filter(|path| !path.eq_ignore_ascii_case("SKILL.md"))
        .collect();
    files.sort_unstable_by_key(|path| path.to_ascii_lowercase());
    files
}

fn build_reference_manifest(skill_name: &str, folder: &str) -> String {
    let files = list_skill_files(folder);
    if files.is_empty() {
        return String::new();
    }

    let mut shallow = Vec::new();
    let mut deep_groups: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        let segments: Vec<&str> = file.split('/').collect();
        if segments.len() <= 2
            || segments
                .last()
                .is_some_and(|name| name.eq_ignore_ascii_case("INDEX.md"))
        {
            shallow.push(file);
        } else {
            let key = format!("{}/{}/", segments[0], segments[1]);
            *deep_groups.entry(key).or_default() += 1;
        }
    }

    let mut output = format!(
        "\n\n## Reference files (bundled with this skill)\n\n\
This skill defers detail to the files below. The body's `reference/…` pointers refer to these. Fetch one with:\n\
- `load_skill {skill_name} --path <relpath>`\n\
- or install the whole tree to disk: `officecli skills install {skill_name}`\n\n"
    );
    for file in shallow {
        output.push_str(&format!("- `{file}`\n"));
    }
    for (group, count) in deep_groups {
        output.push_str(&format!(
            "- `{group}` — {count} files (binary assets need `skills install`; browse an `INDEX.md` here if present)\n"
        ));
    }
    output
}

fn full_description(content: &str) -> &str {
    if !content.starts_with("---") {
        return "";
    }
    let Some(end_offset) = content[3..].find("---") else {
        return "";
    };
    let front_matter = &content[3..3 + end_offset];
    for line in front_matter.lines() {
        let line = line.trim();
        if let Some(value) = line
            .to_ascii_lowercase()
            .strip_prefix("description:")
            .map(|_| &line["description:".len()..])
        {
            return value.trim().trim_matches('"');
        }
    }
    ""
}

fn strip_setup_section(content: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut in_setup = false;
    for line in content.split('\n') {
        if !in_setup && line.starts_with("## Setup") {
            in_setup = true;
            continue;
        }
        if in_setup && line.starts_with("## ") {
            in_setup = false;
        }
        if !in_setup {
            output.push_str(line);
            output.push('\n');
        }
    }
    if !content.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }
    output
}

fn is_binary_path(path: &str) -> bool {
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    extension.is_some_and(|extension| {
        BINARY_EXTENSIONS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(extension))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_aliases_with_full_front_matter_descriptions() {
        let catalog = build_skill_catalog().unwrap();
        assert!(catalog.contains("## pptx\nUse this skill any time a .pptx file is involved"));
        assert!(catalog.contains("## word-form\nUse this skill to create fillable Word forms"));
        assert!(!catalog.contains("## officecli\n"));
    }

    #[test]
    fn content_strips_setup_and_lists_shallow_and_collapsed_references() {
        let content = load_skill_content("morph-ppt").unwrap();
        assert!(!content.contains("## Setup"));
        assert!(content.contains("`reference/decision-rules.md`"));
        assert!(content.contains("`reference/styles/INDEX.md`"));
        assert!(content.contains("`reference/styles/` —"));
        assert!(!content.contains("reference/styles/dark--premium-navy/template.pptx`"));
    }

    #[test]
    fn reference_file_can_be_loaded_with_windows_separators() {
        let content = load_skill_file("MORPH-PPT", r"reference\decision-rules.md").unwrap();
        assert!(content.contains("# PPT Planner"));
    }

    #[test]
    fn reference_file_rejects_traversal_and_binary_assets() {
        let traversal = load_skill_file("morph-ppt", "../SKILL.md").unwrap_err();
        assert!(traversal.to_string().contains("Invalid skill file path"));

        let binary = load_skill_file(
            "morph-ppt",
            "reference/styles/dark--premium-navy/template.pptx",
        )
        .unwrap_err();
        assert!(binary.to_string().contains("binary asset"));
    }

    #[test]
    fn unknown_skill_lists_supported_aliases() {
        let error = load_skill_content("officecli-pptx").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Unknown skill: officecli-pptx"));
        assert!(message.contains("academic-paper"));
        assert!(message.contains("word-form"));
    }
}
