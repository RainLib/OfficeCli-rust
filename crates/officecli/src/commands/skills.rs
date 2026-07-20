use clap::{Args, Subcommand};
use handler_common::{HandlerError, OutputFormat};

/// Install agent skill definitions (Claude Code, Cursor, Copilot, etc.)
#[derive(Args)]
pub struct SkillsCommand {
    #[command(subcommand)]
    pub action: SkillsAction,
}

#[derive(Subcommand)]
pub enum SkillsAction {
    /// List all available skills
    List,
    /// Install a specific skill
    Install {
        /// Skill name to install (e.g. "pitch-deck", "academic-paper"), or "all" for all
        skill: String,
        /// Target agent (optional, default: all agents). Supported: claude, copilot, cursor, windsurf, opencode, all
        agent: Option<String>,
    },
}

pub fn handle_skills(cmd: SkillsCommand, _format: OutputFormat) -> Result<String, HandlerError> {
    match cmd.action {
        SkillsAction::List => handle_list(),
        SkillsAction::Install { skill, agent } => handle_install(&skill, agent.as_deref()),
    }
}

fn handle_list() -> Result<String, HandlerError> {
    let skills_dir = find_skills_dir();
    if !skills_dir.exists() {
        return Ok("No skills found (skills/ directory not found)".to_string());
    }

    let mut output = String::from("Available skills:\n");
    let mut count = 0;

    if let Ok(entries) = std::fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                let skill_md = path.join("SKILL.md");
                let description = if skill_md.exists() {
                    std::fs::read_to_string(&skill_md)
                        .ok()
                        .and_then(|content| {
                            content
                                .lines()
                                .next()
                                .map(|l| l.trim_start_matches("# ").trim().to_string())
                        })
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                output.push_str(&format!("  {:<30} {}\n", skill_name, description));
                count += 1;
            }
        }
    }

    output.push_str(&format!("\n{} skill(s) available\n", count));
    output.push_str("Usage: officecli skills install <skill-name> [agent]\n");
    Ok(output)
}

fn handle_install(skill: &str, agent: Option<&str>) -> Result<String, HandlerError> {
    let skills_dir = find_skills_dir();
    if !skills_dir.exists() {
        return Err(HandlerError::OperationFailed(
            "skills/ directory not found".into(),
        ));
    }

    if skill == "all" {
        let mut installed = 0;
        if let Ok(entries) = std::fs::read_dir(&skills_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("SKILL.md").exists() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        install_single_skill(name, agent)?;
                        installed += 1;
                    }
                }
            }
        }
        Ok(format!("Installed {} skill(s) to agent(s)", installed))
    } else {
        let skill_path = skills_dir.join(skill).join("SKILL.md");
        if !skill_path.exists() {
            return Err(HandlerError::OperationFailed(format!(
                "Skill '{}' not found. Use 'officecli skills list' to see available skills",
                skill
            )));
        }
        install_single_skill(skill, agent)?;
        Ok(format!("Installed skill '{}'", skill))
    }
}

fn install_single_skill(skill_name: &str, agent: Option<&str>) -> Result<(), HandlerError> {
    let skills_dir = find_skills_dir();
    let source_path = skills_dir.join(skill_name).join("SKILL.md");
    let content = std::fs::read_to_string(&source_path).map_err(|e| {
        HandlerError::OperationFailed(format!("Failed to read skill: {}", e))
    })?;

    let target_agents: Vec<String> = match agent {
        Some("all") | None => vec![
            "claude".to_string(),
            "cursor".to_string(),
            "windsurf".to_string(),
            "opencode".to_string(),
        ],
        Some(a) => vec![a.to_string()],
    };

    for agent_name in &target_agents {
        let dest_dir = get_agent_skill_dir(agent_name);
        if let Some(dest_dir) = dest_dir {
            let dest_path = dest_dir.join(format!("officecli-{}.md", skill_name));
            if let Some(parent) = dest_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&dest_path, &content);
        }
    }

    Ok(())
}

fn find_skills_dir() -> std::path::PathBuf {
    // 1. Look next to the binary (installed location)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            // Walk up the directory tree to find skills/ (handles target/release/ -> project root)
            let mut dir = Some(exe_dir.to_path_buf());
            while let Some(current) = dir {
                let candidate = current.join("skills");
                if candidate.is_dir() {
                    return candidate;
                }
                // Stop at filesystem root
                let parent = current.parent().map(|p| p.to_path_buf());
                if parent.as_ref() == Some(&current) {
                    break;
                }
                dir = parent;
            }
        }
    }

    // 2. Look relative to cwd
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join("skills");
        if path.exists() {
            return path;
        }
    }

    // 3. Look in user config dir
    if let Some(home) = home_dir() {
        let path = home.join(".officecli").join("skills");
        if path.exists() {
            return path;
        }
    }

    // 4. Check OFFICECLI_SKILLS_DIR env var
    if let Ok(env_dir) = std::env::var("OFFICECLI_SKILLS_DIR") {
        let path = std::path::PathBuf::from(env_dir);
        if path.exists() {
            return path;
        }
    }

    std::path::PathBuf::from("skills")
}

fn get_agent_skill_dir(agent: &str) -> Option<std::path::PathBuf> {
    match agent {
        "claude" => home_dir().map(|h| h.join(".claude").join("skills")),
        "cursor" => home_dir().map(|h| h.join(".cursor").join("skills")),
        "windsurf" => home_dir().map(|h| h.join(".windsurf").join("skills")),
        "opencode" => home_dir().map(|h| h.join(".config").join("opencode").join("skills")),
        _ => None,
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME").ok().map(std::path::PathBuf::from)
}
