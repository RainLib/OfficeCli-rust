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

    errors
}
