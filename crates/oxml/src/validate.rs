use handler_common::ValidationError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ValidateError {
    #[error("validation failed: {0}")]
    Failed(String),
}

/// Validate an OOXML document against OpenXML schema.
/// NOTE: Full schema validation requires the OpenXML schema files.
/// This implementation provides basic structural validation.
pub fn validate_package(
    parts: &std::collections::HashMap<String, Vec<u8>>,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Check required parts exist
    let has_content_types = parts.contains_key("[Content_Types].xml");
    let has_rels = parts.contains_key("_rels/.rels");

    if !has_content_types {
        errors.push(ValidationError {
            error_type: "MissingPart".to_string(),
            description: "[Content_Types].xml is missing".to_string(),
            path: None,
            part: None,
        });
    }

    if !has_rels {
        errors.push(ValidationError {
            error_type: "MissingPart".to_string(),
            description: "_rels/.rels is missing".to_string(),
            path: None,
            part: None,
        });
    }

    // Validate XML parts are parseable
    for (path, content) in parts {
        if path.ends_with(".xml") || path.ends_with(".rels") {
            if let Err(e) = roxmltree::Document::parse(&String::from_utf8_lossy(content)) {
                errors.push(ValidationError {
                    error_type: "XmlParseError".to_string(),
                    description: e.to_string(),
                    path: None,
                    part: Some(path.clone()),
                });
            }
        }
    }

    // Every non-metadata part needs either an Override or a Default content
    // type. Missing entries make a package unreadable in Office even when the
    // XML itself is well formed.
    if let Some(content_types) = parts.get("[Content_Types].xml") {
        if let Ok(types) = crate::content_types::ContentTypes::parse(content_types) {
            for path in parts
                .keys()
                .filter(|path| path.as_str() != "[Content_Types].xml")
            {
                if types.content_type_for(path).is_none() {
                    errors.push(ValidationError {
                        error_type: "missing-content-type".to_string(),
                        description: format!("part '{}' has no content type declaration", path),
                        path: Some(path.clone()),
                        part: Some(path.clone()),
                    });
                }
            }
        }
    }

    // Relationship targets are part of the package graph. Check every
    // internal target, including relationships owned by headers, charts, and
    // worksheets; format-specific validators cannot reliably cover all of
    // those secondary parts.
    for (rels_path, content) in parts.iter().filter(|(path, _)| path.ends_with(".rels")) {
        let Ok(rels) = crate::rels::Relationships::parse(content) else {
            continue;
        };
        let base = relationship_base(rels_path);
        for rel in rels.all().values() {
            if rel.target_mode.eq_ignore_ascii_case("external") || rel.target.is_empty() {
                continue;
            }
            let target = normalize_target(&base, &rel.target);
            if !parts.contains_key(&target) {
                errors.push(ValidationError {
                    error_type: "broken-relationship".to_string(),
                    description: format!(
                        "relationship '{}' targets missing part '{}'",
                        rel.id, target
                    ),
                    path: Some(rels_path.clone()),
                    part: Some(rels_path.clone()),
                });
            }
        }
    }

    errors
}

fn relationship_base(rels_path: &str) -> String {
    if rels_path == "_rels/.rels" {
        return String::new();
    }
    // word/_rels/document.xml.rels is owned by word/document.xml, so targets
    // resolve relative to word/ rather than word/_rels/.
    rels_path
        .strip_suffix(".rels")
        .and_then(|path| path.rsplit_once("/_rels/"))
        .map(|(directory, _)| directory.to_string())
        .unwrap_or_default()
}

fn normalize_target(base: &str, target: &str) -> String {
    let mut segments: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::validate_package;
    use std::collections::HashMap;

    #[test]
    fn reports_missing_internal_relationship_target() {
        let mut parts = HashMap::new();
        parts.insert("[Content_Types].xml".to_string(), br#"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/></Types>"#.to_vec());
        parts.insert("_rels/.rels".to_string(), br#"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"x\" Target=\"word/document.xml\"/></Relationships>"#.to_vec());
        let errors = validate_package(&parts);
        assert!(errors
            .iter()
            .any(|error| error.error_type == "broken-relationship"));
    }
}
