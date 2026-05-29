use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ContentTypesError {
    #[error("XML parse error: {0}")]
    XmlError(String),
}

/// Represents the [Content_Types].xml in an OOXML package.
pub struct ContentTypes {
    /// Default content types: extension -> content type
    defaults: HashMap<String, String>,
    /// Override content types: part path -> content type
    overrides: HashMap<String, String>,
}

impl ContentTypes {
    pub fn empty() -> Self {
        Self {
            defaults: HashMap::new(),
            overrides: HashMap::new(),
        }
    }

    /// Parse [Content_Types].xml content.
    pub fn parse(xml: &[u8]) -> Result<Self, ContentTypesError> {
        let mut defaults = HashMap::new();
        let mut overrides = HashMap::new();

        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) => {
                    match e.local_name().as_ref() {
                        b"Default" => {
                            let ext = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"Extension")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            let ct = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"ContentType")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            if let (Some(ext), Some(ct)) = (ext, ct) {
                                defaults.insert(ext, ct);
                            }
                        }
                        b"Override" => {
                            let pn = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"PartName")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            let ct = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"ContentType")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            if let (Some(pn), Some(ct)) = (pn, ct) {
                                // Strip leading "/" from PartName
                                let pn = if pn.starts_with('/') {
                                    pn[1..].to_string()
                                } else {
                                    pn
                                };
                                overrides.insert(pn, ct);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Empty(e)) => {
                    // Handle self-closing Default/Override elements
                    match e.local_name().as_ref() {
                        b"Default" => {
                            let ext = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"Extension")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            let ct = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"ContentType")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            if let (Some(ext), Some(ct)) = (ext, ct) {
                                defaults.insert(ext, ct);
                            }
                        }
                        b"Override" => {
                            let pn = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"PartName")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            let ct = e
                                .attributes()
                                .filter_map(|a| a.ok())
                                .find(|a| a.key.local_name().as_ref() == b"ContentType")
                                .and_then(|a| String::from_utf8(a.value.to_vec()).ok());
                            if let (Some(pn), Some(ct)) = (pn, ct) {
                                let pn = if pn.starts_with('/') {
                                    pn[1..].to_string()
                                } else {
                                    pn
                                };
                                overrides.insert(pn, ct);
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(ContentTypesError::XmlError(e.to_string())),
                _ => {}
            }
            buf.clear();
        }

        Ok(Self {
            defaults,
            overrides,
        })
    }

    /// Get content type for a part by its path (checks overrides first, then defaults).
    pub fn content_type_for(&self, part_path: &str) -> Option<&String> {
        // Check overrides first
        if let Some(ct) = self.overrides.get(part_path) {
            return Some(ct);
        }
        // Check defaults by extension
        let ext = part_path.rsplit('.').next().unwrap_or("");
        self.defaults.get(ext)
    }

    /// Get all overrides.
    pub fn overrides(&self) -> &HashMap<String, String> {
        &self.overrides
    }

    /// Get all defaults.
    pub fn defaults(&self) -> &HashMap<String, String> {
        &self.defaults
    }
}
