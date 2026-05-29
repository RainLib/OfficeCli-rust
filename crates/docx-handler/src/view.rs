use crate::dom_types::{WordDom, WordElementType};
use handler_common::{DocumentIssue, HandlerError, IssueSeverity, ViewOptions};

/// Render the document as plain text.
/// Each paragraph on its own line. Tables are rendered with tab-separated cells.
pub fn view_as_text(dom: &WordDom, opts: ViewOptions) -> Result<String, HandlerError> {
    let full_text = dom.full_text();
    apply_line_range(&full_text, &opts)
}

/// Render the document as annotated text.
/// Each paragraph prefixed with its path ID, e.g.:
///   /body/p[1]: Hello world
///   /body/p[2]: Second paragraph
pub fn view_as_annotated(dom: &WordDom, opts: ViewOptions) -> Result<String, HandlerError> {
    let body = dom
        .body()
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let mut result = String::new();
    let mut para_idx = 0;
    let mut tbl_idx = 0;
    let mut sdt_idx = 0;

    for child in &body.children {
        match child.element_type {
            WordElementType::Paragraph => {
                para_idx += 1;
                let path = format!("/body/p[{}]", para_idx);
                let text = child.paragraph_text();
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&format!("{}: {}", path, text));
            }
            WordElementType::Table => {
                tbl_idx += 1;
                let path = format!("/body/tbl[{}]", tbl_idx);
                let text = dom.table_text(child);
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&format!("{}: {}", path, text));
            }
            WordElementType::Sdt => {
                sdt_idx += 1;
                let path = format!("/body/sdt[{}]", sdt_idx);
                let mut sdt_text = String::new();
                for sdt_child in &child.children {
                    if sdt_child.element_type == WordElementType::SdtContent {
                        for content_child in &sdt_child.children {
                            if content_child.element_type == WordElementType::Paragraph {
                                if !sdt_text.is_empty() {
                                    sdt_text.push('\n');
                                }
                                sdt_text.push_str(&content_child.paragraph_text());
                            }
                        }
                    }
                }
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str(&format!("{}: {}", path, sdt_text));
            }
            _ => {}
        }
    }

    apply_line_range(&result, &opts)
}

/// Render document as outline (headings only).
/// Shows heading hierarchy with indentation.
pub fn view_as_outline(dom: &WordDom) -> Result<String, HandlerError> {
    let outline = dom.outline();
    if outline.is_empty() {
        return Ok("(no headings found)".to_string());
    }

    let mut result = String::new();
    for entry in &outline {
        let indent: usize = if entry.level == 0 {
            0
        } else {
            (entry.level as usize - 1) * 2
        };
        if entry.level == 0 {
            result.push_str("# ");
        } else {
            result.push_str(&format!("{}H{} ", " ".repeat(indent), entry.level));
        }
        result.push_str(&entry.text);
        result.push('\n');
    }

    Ok(result.trim_end().to_string())
}

/// Render document stats as text.
pub fn view_as_stats(dom: &WordDom) -> Result<String, HandlerError> {
    let stats = dom.stats();
    Ok(format!(
        "Paragraphs: {}\nTables: {}\nRows: {}\nCells: {}\nRuns: {}\nWords: {}\nCharacters: {}\nHeadings: {}\nImages: {}",
        stats.paragraph_count,
        stats.table_count,
        stats.row_count,
        stats.cell_count,
        stats.run_count,
        stats.word_count,
        stats.char_count,
        stats.heading_count,
        stats.image_count,
    ))
}

/// Render document stats as JSON.
pub fn view_as_stats_json(dom: &WordDom) -> Result<serde_json::Value, HandlerError> {
    let stats = dom.stats();
    Ok(serde_json::json!({
        "format": "docx",
        "paragraphs": stats.paragraph_count,
        "tables": stats.table_count,
        "rows": stats.row_count,
        "cells": stats.cell_count,
        "runs": stats.run_count,
        "words": stats.word_count,
        "characters": stats.char_count,
        "headings": stats.heading_count,
        "images": stats.image_count,
    }))
}

/// Render annotated view as JSON.
pub fn view_as_text_json(
    dom: &WordDom,
    opts: ViewOptions,
) -> Result<serde_json::Value, HandlerError> {
    let body = dom
        .body()
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let mut paragraphs = Vec::new();
    let mut para_idx = 0;
    let mut tbl_idx = 0;

    let start_line = opts.start_line.unwrap_or(0);
    let end_line = opts.end_line.unwrap_or(usize::MAX);

    for child in &body.children {
        match child.element_type {
            WordElementType::Paragraph => {
                para_idx += 1;
                if para_idx < start_line || para_idx > end_line {
                    continue;
                }
                let path = format!("/body/p[{}]", para_idx);
                let text = child.paragraph_text();
                let mut runs = Vec::new();
                let mut run_idx = 0;
                for run_child in &child.children {
                    if run_child.element_type == WordElementType::Run {
                        run_idx += 1;
                        let run_text = run_child.paragraph_text();
                        if !run_text.is_empty() {
                            runs.push(serde_json::json!({
                                "path": format!("{}/r[{}]", path, run_idx),
                                "text": run_text,
                                "bold": run_child.is_bold(),
                                "italic": run_child.is_italic(),
                            }));
                        }
                    }
                }

                paragraphs.push(serde_json::json!({
                    "path": path,
                    "text": text,
                    "runs": runs,
                    "heading": child.heading_level(),
                }));
            }
            WordElementType::Table => {
                tbl_idx += 1;
                let path = format!("/body/tbl[{}]", tbl_idx);
                let text = dom.table_text(child);
                paragraphs.push(serde_json::json!({
                    "path": path,
                    "text": text,
                    "type": "table",
                }));
            }
            _ => {}
        }
    }

    Ok(serde_json::json!({
        "format": "docx",
        "paragraphs": paragraphs,
    }))
}

/// Render outline as JSON.
pub fn view_as_outline_json(dom: &WordDom) -> Result<serde_json::Value, HandlerError> {
    let outline = dom.outline();
    let entries: Vec<serde_json::Value> = outline
        .iter()
        .map(|e| {
            serde_json::json!({
                "level": e.level,
                "text": e.text,
                "path": format!("/body/p[{}]", e.para_index + 1),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "format": "docx",
        "outline": entries,
    }))
}

/// Check for common document issues.
pub fn view_as_issues(
    dom: &WordDom,
    issue_type: Option<&str>,
    limit: Option<usize>,
) -> Vec<DocumentIssue> {
    let mut issues = Vec::new();
    let max = limit.unwrap_or(50);

    let paragraphs = dom.paragraphs();

    // Check for empty paragraphs
    if issue_type.is_none() || issue_type == Some("empty") {
        for (i, para) in paragraphs.iter().enumerate() {
            if para.paragraph_text().trim().is_empty() {
                issues.push(DocumentIssue {
                    severity: IssueSeverity::Info,
                    issue_type: "empty-paragraph".to_string(),
                    description: "Empty paragraph".to_string(),
                    path: Some(format!("/body/p[{}]", i + 1)),
                });
                if issues.len() >= max {
                    return issues;
                }
            }
        }
    }

    // Check for very long paragraphs
    if issue_type.is_none() || issue_type == Some("long") {
        for (i, para) in paragraphs.iter().enumerate() {
            let word_count = para.paragraph_text().split_whitespace().count();
            if word_count > 200 {
                issues.push(DocumentIssue {
                    severity: IssueSeverity::Warning,
                    issue_type: "long-paragraph".to_string(),
                    description: format!("Paragraph has {} words (over 200)", word_count),
                    path: Some(format!("/body/p[{}]", i + 1)),
                });
                if issues.len() >= max {
                    return issues;
                }
            }
        }
    }

    // Check for inconsistent spacing
    if issue_type.is_none() || issue_type == Some("spacing") {
        for (i, para) in paragraphs.iter().enumerate() {
            let text = para.paragraph_text();
            if text.contains("  ") && !text.trim().is_empty() {
                issues.push(DocumentIssue {
                    severity: IssueSeverity::Info,
                    issue_type: "double-space".to_string(),
                    description: "Paragraph contains double spaces".to_string(),
                    path: Some(format!("/body/p[{}]", i + 1)),
                });
                if issues.len() >= max {
                    return issues;
                }
            }
        }
    }

    // Check for missing heading styles
    if issue_type.is_none() || issue_type == Some("heading") {
        let outline = dom.outline();
        if outline.is_empty() && paragraphs.len() > 5 {
            issues.push(DocumentIssue {
                severity: IssueSeverity::Warning,
                issue_type: "no-headings".to_string(),
                description: "Document has no headings defined".to_string(),
                path: None,
            });
        }
    }

    issues
}

/// Apply line range filtering from ViewOptions.
fn apply_line_range(text: &str, opts: &ViewOptions) -> Result<String, HandlerError> {
    let lines: Vec<&str> = text.lines().collect();
    let start = opts.start_line.unwrap_or(0);
    let end = opts.end_line.unwrap_or(lines.len());

    let start = if start > 0 { start - 1 } else { 0 };
    let end = end.min(lines.len());

    if start >= end {
        return Ok(String::new());
    }

    let selected = &lines[start..end];
    let max_lines = opts.max_lines.unwrap_or(selected.len());
    let limited = &selected[..max_lines.min(selected.len())];

    Ok(limited.join("\n"))
}
