/// `help` command — show schema-driven capability reference for officecli.
///
/// Usage:
///   officecli help                       → list formats
///   officecli help <format>              → list all elements for a format
///   officecli help <format> <element>    → show element details
///   officecli help <format> <verb> <element> → verb-filtered element detail
///   officecli help mcp|skills|install    → show early-dispatch command usage
use clap::Args;

#[derive(Args)]
pub struct HelpCommand {
    /// Document format (docx/xlsx/pptx) or command name (mcp/skills/install)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Format metadata for help listing.
struct FormatInfo {
    name: &'static str,
    aliases: &'static str,
    description: &'static str,
    elements: &'static [&'static str],
    verbs: &'static [&'static str],
}

/// Element metadata for a format.
struct ElementInfo {
    name: &'static str,
    description: &'static str,
    supported_verbs: &'static [&'static str],
    properties: &'static [&'static str],
}

fn get_formats() -> Vec<FormatInfo> {
    vec![
        FormatInfo {
            name: "docx",
            aliases: "word",
            description: "Word processing documents (.docx)",
            elements: &[
                "paragraph",
                "run",
                "table",
                "row",
                "cell",
                "image",
                "textbox",
                "header",
                "footer",
                "section",
                "bookmark",
                "hyperlink",
                "comment",
                "footnote",
                "endnote",
            ],
            verbs: &["add", "set", "get", "query", "remove"],
        },
        FormatInfo {
            name: "xlsx",
            aliases: "excel",
            description: "Spreadsheet documents (.xlsx)",
            elements: &[
                "cell",
                "sheet",
                "row",
                "column",
                "range",
                "formula",
                "chart",
                "pivot",
                "table",
                "detectedtable",
                "image",
                "comment",
            ],
            verbs: &["add", "set", "get", "query", "remove"],
        },
        FormatInfo {
            name: "pptx",
            aliases: "ppt, powerpoint",
            description: "Presentation documents (.pptx)",
            elements: &[
                "slide",
                "shape",
                "text-block",
                "image",
                "table",
                "chart",
                "textbox",
                "video",
                "morph",
                "animation",
                "transition",
                "linebreak",
                "comment",
                "modernComment",
                "notes",
            ],
            verbs: &["add", "set", "get", "query", "remove"],
        },
    ]
}

fn get_elements(format: &str) -> Vec<ElementInfo> {
    match format {
        "docx" | "word" => vec![
            ElementInfo {
                name: "paragraph",
                description: "A text paragraph (<w:p>)",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &["text", "style", "alignment", "indent", "spacing"],
            },
            ElementInfo {
                name: "run",
                description: "A text run within a paragraph (<w:r>)",
                supported_verbs: &["set", "get", "query"],
                properties: &[
                    "text",
                    "bold",
                    "italic",
                    "underline",
                    "font",
                    "size",
                    "color",
                ],
            },
            ElementInfo {
                name: "table",
                description: "A table element (<w:tbl>)",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &["rows", "cols", "border", "style"],
            },
            ElementInfo {
                name: "row",
                description: "A table row",
                supported_verbs: &["add", "set", "get", "remove"],
                properties: &["height", "cells"],
            },
            ElementInfo {
                name: "cell",
                description: "A table cell",
                supported_verbs: &["set", "get"],
                properties: &["text", "width", "span", "shading"],
            },
            ElementInfo {
                name: "image",
                description: "An inline or floating image",
                supported_verbs: &["add", "get", "query", "remove"],
                properties: &["file", "width", "height", "alt-text"],
            },
            ElementInfo {
                name: "textbox",
                description: "A text box shape",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &["text", "width", "height"],
            },
            ElementInfo {
                name: "header",
                description: "A header section",
                supported_verbs: &["get", "set"],
                properties: &["text"],
            },
            ElementInfo {
                name: "footer",
                description: "A footer section",
                supported_verbs: &["get", "set"],
                properties: &["text"],
            },
            ElementInfo {
                name: "section",
                description: "A document section with page layout",
                supported_verbs: &["get", "query"],
                properties: &["page-width", "page-height", "margins", "orientation"],
            },
            ElementInfo {
                name: "bookmark",
                description: "A bookmark anchor",
                supported_verbs: &["get", "query"],
                properties: &["name"],
            },
            ElementInfo {
                name: "hyperlink",
                description: "A hyperlink",
                supported_verbs: &["get", "query"],
                properties: &["url", "text"],
            },
            ElementInfo {
                name: "comment",
                description: "A comment annotation",
                supported_verbs: &["get", "query"],
                properties: &["text", "author"],
            },
        ],
        "xlsx" | "excel" => vec![
            ElementInfo {
                name: "cell",
                description: "A spreadsheet cell",
                supported_verbs: &["set", "get", "query"],
                properties: &["text", "value", "formula", "format", "color", "bg-color"],
            },
            ElementInfo {
                name: "sheet",
                description: "A worksheet tab",
                supported_verbs: &["add", "get", "query", "remove"],
                properties: &["name", "index"],
            },
            ElementInfo {
                name: "row",
                description: "A row of cells",
                supported_verbs: &["add", "get", "set"],
                properties: &["height", "cells"],
            },
            ElementInfo {
                name: "column",
                description: "A column of cells",
                supported_verbs: &["get", "set"],
                properties: &["width", "auto-filter"],
            },
            ElementInfo {
                name: "range",
                description: "A cell range (e.g. A1:C10)",
                supported_verbs: &["get", "set", "query"],
                properties: &["highlight", "bg-color", "border"],
            },
            ElementInfo {
                name: "formula",
                description: "A cell formula",
                supported_verbs: &["get", "query"],
                properties: &["expression", "result"],
            },
            ElementInfo {
                name: "chart",
                description: "A chart object",
                supported_verbs: &["add", "get", "query"],
                properties: &["type", "title", "data-range"],
            },
            ElementInfo {
                name: "pivot",
                description: "A pivot table definition",
                supported_verbs: &["get", "query"],
                properties: &["name", "source-range", "fields"],
            },
            ElementInfo {
                name: "table",
                description: "Real ListObjects plus detected table-shaped blocks when queried",
                supported_verbs: &["add", "get", "query"],
                properties: &["name", "range", "style"],
            },
            ElementInfo {
                name: "detectedtable",
                description: "A read-only header-sniff table-shaped cell block",
                supported_verbs: &[],
                properties: &["source", "stable", "ref", "columns", "dataRange"],
            },
            ElementInfo {
                name: "image",
                description: "An embedded image",
                supported_verbs: &["add", "get", "query"],
                properties: &["file", "position"],
            },
            ElementInfo {
                name: "comment",
                description: "A cell comment",
                supported_verbs: &["get", "query"],
                properties: &["text", "author"],
            },
        ],
        "pptx" | "ppt" | "powerpoint" => vec![
            ElementInfo {
                name: "linebreak",
                description: "A soft line break (<a:br/>) within a shape paragraph",
                supported_verbs: &["add", "get", "remove"],
                properties: &[],
            },
            ElementInfo {
                name: "slide",
                description: "A presentation slide",
                supported_verbs: &["add", "get", "query", "remove"],
                properties: &["title", "layout", "index"],
            },
            ElementInfo {
                name: "shape",
                description: "A shape on a slide",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &["text", "name", "position", "size"],
            },
            ElementInfo {
                name: "text-block",
                description: "A text content block within a shape",
                supported_verbs: &["get", "query", "set"],
                properties: &["text", "font", "size", "color", "bold", "italic"],
            },
            ElementInfo {
                name: "image",
                description: "A picture on a slide",
                supported_verbs: &["add", "get", "query", "remove"],
                properties: &["file", "position", "size"],
            },
            ElementInfo {
                name: "table",
                description: "A table on a slide",
                supported_verbs: &["add", "set", "get", "query"],
                properties: &["rows", "cols", "style"],
            },
            ElementInfo {
                name: "chart",
                description: "A chart on a slide",
                supported_verbs: &["add", "get", "query"],
                properties: &["type", "data-range", "title"],
            },
            ElementInfo {
                name: "textbox",
                description: "A text box on a slide",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &["text", "position", "size"],
            },
            ElementInfo {
                name: "video",
                description: "A video on a slide",
                supported_verbs: &["add", "get"],
                properties: &["file", "position"],
            },
            ElementInfo {
                name: "morph",
                description: "A morph transition between slides",
                supported_verbs: &["get", "query"],
                properties: &["candidates"],
            },
            ElementInfo {
                name: "animation",
                description: "A slide animation effect",
                supported_verbs: &["get", "query"],
                properties: &["type", "duration"],
            },
            ElementInfo {
                name: "transition",
                description: "A slide transition effect",
                supported_verbs: &["get", "query"],
                properties: &["type", "duration", "morph"],
            },
            ElementInfo {
                name: "comment",
                description: "A legacy slide comment annotation",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &["text", "author", "initials", "date", "x", "y"],
            },
            ElementInfo {
                name: "modernComment",
                description: "An Office 2018+ threaded slide comment",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &[
                    "text", "author", "initials", "created", "resolved", "parent",
                ],
            },
            ElementInfo {
                name: "notes",
                description: "Speaker notes for a slide",
                supported_verbs: &["add", "set", "get", "query", "remove"],
                properties: &["text", "direction", "lang"],
            },
        ],
        _ => vec![],
    }
}

const HELP_VERBS: &[&str] = &["add", "set", "get", "query", "remove"];

/// Early-dispatch help text for mcp/skills/install commands.
fn early_dispatch_help(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "mcp" => Some(&[
            "Usage:",
            "  officecli mcp                    Start MCP stdio server (for AI agents)",
            "  officecli mcp <target>           Register officecli with an MCP client",
            "  officecli mcp uninstall <target>  Unregister officecli from an MCP client",
            "  officecli mcp list               Show registration status across all clients",
            "",
            "Targets: lms, claude, cursor, vscode (Copilot)",
        ]),
        "skills" | "skill" => Some(&[
            "Usage:",
            "  officecli skills install                Install base SKILL.md to all detected agents",
            "  officecli skills install <skill-name>   Install a specific skill to all detected agents",
            "  officecli skills list                   List all available skills",
            "",
            "Skills: pptx, word, excel, morph-ppt, pitch-deck, academic-paper, data-dashboard, financial-model",
            "Agents: claude, copilot, codex, cursor, windsurf, all",
        ]),
        "load_skill" => Some(&[
            "Usage:",
            "  officecli load_skill                         List all skills with the triggers that say when to use each",
            "  officecli load_skill <name>                 Print the skill's SKILL.md + a manifest of its bundled reference files",
            "  officecli load_skill <name> --path <relpath> Print one bundled reference file (e.g. --path reference/decision-rules.md)",
            "",
            "Skills: pptx, word, excel, word-form, morph-ppt, morph-ppt-3d, pitch-deck, academic-paper, data-dashboard, financial-model",
            "To install a skill (with binary assets) on disk, run: officecli skills install <name>",
        ]),
        "install" => Some(&[
            "Usage:",
            "  officecli install           One-step setup: install binary + skills + MCP to all detected agents",
            "  officecli install <target>  Install to a specific agent (claude, copilot, cursor, vscode, ...)",
            "",
            "Equivalent to: installing the binary, then `officecli skills install` and `officecli mcp <target>`.",
            "Targets: claude, copilot, codex, cursor, windsurf, vscode, all",
        ]),
        _ => None,
    }
}

pub fn handle_help(cmd: HelpCommand, json: bool) -> Result<String, handler_common::HandlerError> {
    let args = &cmd.args;

    // No arguments → list formats
    if args.is_empty() {
        if json {
            let formats = get_formats();
            let json_arr: Vec<serde_json::Value> = formats
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "format": f.name,
                        "aliases": f.aliases,
                        "description": f.description,
                        "elements": f.elements,
                        "verbs": f.verbs,
                    })
                })
                .collect();
            return Ok(serde_json::to_string_pretty(&serde_json::json!({
                "formats": json_arr,
            }))
            .unwrap_or_default());
        }

        let mut out = String::new();
        out.push_str("officecli — AI-friendly CLI for Office documents\n\n");
        out.push_str("Formats:\n");
        for f in get_formats() {
            out.push_str(&format!(
                "  {} ({}) — {}\n",
                f.name, f.aliases, f.description
            ));
        }
        out.push_str("\nUsage: officecli help <format> [verb] [element]\n");
        out.push_str("       officecli help <command>   (mcp, skills, install)\n");
        out.push_str("\nVerbs: add, set, get, query, remove\n");
        return Ok(out);
    }

    let first = &args[0].to_lowercase();

    // Check for early-dispatch commands (mcp, skills, install)
    if let Some(lines) = early_dispatch_help(first) {
        return Ok(lines.join("\n"));
    }

    // Resolve format name
    let format_name = match first.as_str() {
        "word" => "docx",
        "excel" => "xlsx",
        "ppt" | "powerpoint" => "pptx",
        other => other,
    };

    let formats = get_formats();
    let format_info = match formats.iter().find(|f| f.name == format_name) {
        Some(f) => f,
        None => {
            return Err(handler_common::HandlerError::InvalidArgument(format!(
                "unknown format '{}'. Use: docx, xlsx, pptx",
                first
            )));
        }
    };

    // One argument: list elements for the format
    if args.len() == 1 {
        let elements = get_elements(format_name);
        if json {
            let json_arr: Vec<serde_json::Value> = elements
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "element": e.name,
                        "description": e.description,
                        "verbs": e.supported_verbs,
                        "properties": e.properties,
                    })
                })
                .collect();
            return Ok(serde_json::to_string_pretty(&serde_json::json!({
                "format": format_name,
                "elements": json_arr,
            }))
            .unwrap_or_default());
        }

        let mut out = String::new();
        out.push_str(&format!(
            "{} — {}\n\n",
            format_info.name, format_info.description
        ));
        out.push_str("Elements:\n");
        for e in get_elements(format_name) {
            out.push_str(&format!(
                "  {:15} {} [{}]\n",
                e.name,
                e.description,
                e.supported_verbs.join(", ")
            ));
        }
        out.push_str(&format!("\nVerbs: {}\n", format_info.verbs.join(", ")));
        out.push_str(&format!(
            "Usage: officecli help {} <element>  or  officecli help {} <verb> <element>\n",
            format_name, format_name
        ));
        return Ok(out);
    }

    // Two or three arguments
    let second = &args[1].to_lowercase();

    // If second arg is a verb, the third is the element
    let (verb, element_name) = if HELP_VERBS.contains(&second.as_str()) {
        let elem = args.get(2).map(|s| s.to_lowercase());
        (Some(second.as_str()), elem)
    } else {
        (None, Some(second.clone()))
    };

    let elements = get_elements(format_name);
    let element = match element_name {
        Some(ref name) => {
            // `add --type-name note` is a supported concise alias for the
            // `/slide[N]/notes` resource. Keep help discoverable through the
            // same spelling rather than making a valid command look missing.
            let canonical_name = if format_name == "pptx" && name.eq_ignore_ascii_case("note") {
                "notes"
            } else {
                name
            };
            elements
                .iter()
                .find(|element| element.name.eq_ignore_ascii_case(canonical_name))
        }
        None => None,
    };

    match (verb, element) {
        (None, None) => {
            // Second arg wasn't a verb or known element
            Err(handler_common::HandlerError::InvalidArgument(
                format!("unknown element or verb '{}' for format '{}'. Use 'officecli help {}' to list elements.", second, format_name, format_name)
            ))
        }
        (Some(verb), None) => {
            // Verb given but no element — list elements supporting that verb
            let matching: Vec<&ElementInfo> = elements
                .iter()
                .filter(|e| e.supported_verbs.contains(&verb))
                .collect();
            if matching.is_empty() {
                return Ok(format!(
                    "No elements support the '{}' verb for {}.\n",
                    verb, format_name
                ));
            }
            let mut out = format!("{} elements supporting '{}':\n", format_name, verb);
            for e in &matching {
                out.push_str(&format!("  {:15} {}\n", e.name, e.description));
            }
            Ok(out)
        }
        (None, Some(elem)) => {
            // Element without verb — show full detail
            if json {
                return Ok(serde_json::to_string_pretty(&serde_json::json!({
                    "format": format_name,
                    "element": elem.name,
                    "description": elem.description,
                    "verbs": elem.supported_verbs,
                    "properties": elem.properties,
                }))
                .unwrap_or_default());
            }
            let mut out = String::new();
            out.push_str(&format!(
                "{}:{} — {}\n\n",
                format_name, elem.name, elem.description
            ));
            out.push_str("Supported verbs:\n");
            for v in elem.supported_verbs {
                out.push_str(&format!(
                    "  officecli {} <file> ...  ({} on {})\n",
                    v, v, elem.name
                ));
            }
            out.push_str("\nProperties:\n");
            for p in elem.properties {
                out.push_str(&format!("  --prop {}=<value>\n", p));
            }
            Ok(out)
        }
        (Some(verb), Some(elem)) => {
            // Verb + element — verb-filtered detail
            if !elem.supported_verbs.contains(&verb) {
                return Ok(format!(
                    "{} does not support the '{}' verb for {}.\nSupported verbs: {}\n",
                    elem.name,
                    verb,
                    format_name,
                    elem.supported_verbs.join(", ")
                ));
            }
            if json {
                return Ok(serde_json::to_string_pretty(&serde_json::json!({
                    "format": format_name,
                    "element": elem.name,
                    "verb": verb,
                    "description": elem.description,
                    "properties": elem.properties,
                }))
                .unwrap_or_default());
            }
            let mut out = String::new();
            out.push_str(&format!(
                "officecli {} <file> ... — {} {} on {}\n\n",
                verb, verb, elem.name, format_name
            ));
            out.push_str(&format!("{}: {}\n", elem.name, elem.description));
            out.push_str("Properties:\n");
            for p in elem.properties {
                out.push_str(&format!("  --prop {}=<value>\n", p));
            }
            Ok(out)
        }
    }
}
