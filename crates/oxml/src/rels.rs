use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RelsError {
    #[error("XML parse error: {0}")]
    XmlError(String),
}

/// Represents a relationship within an OOXML package.
#[derive(Debug, Clone)]
pub struct Relationship {
    /// Relationship ID (e.g. "rId1")
    pub id: String,
    /// Relationship type URI (e.g. "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument")
    pub type_uri: String,
    /// Target path (e.g. "word/document.xml")
    pub target: String,
    /// Target mode: "Internal" or "External"
    pub target_mode: String,
}

/// Collection of relationships from a .rels file.
pub struct Relationships {
    relationships: HashMap<String, Relationship>,
}

impl Relationships {
    pub fn empty() -> Self {
        Self {
            relationships: HashMap::new(),
        }
    }

    /// Parse a .rels XML file.
    pub fn parse(xml: &[u8]) -> Result<Self, RelsError> {
        let mut relationships = HashMap::new();

        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                    if e.local_name().as_ref() == b"Relationship" {
                        let mut id = String::new();
                        let mut type_uri = String::new();
                        let mut target = String::new();
                        let mut target_mode = "Internal".to_string();

                        for attr in e.attributes().filter_map(|a| a.ok()) {
                            match attr.key.local_name().as_ref() {
                                b"Id" => id = String::from_utf8_lossy(&attr.value).to_string(),
                                b"Type" => {
                                    type_uri = String::from_utf8_lossy(&attr.value).to_string()
                                }
                                b"Target" => {
                                    target = String::from_utf8_lossy(&attr.value).to_string()
                                }
                                b"TargetMode" => {
                                    target_mode = String::from_utf8_lossy(&attr.value).to_string()
                                }
                                _ => {}
                            }
                        }

                        let rel = Relationship {
                            id,
                            type_uri,
                            target,
                            target_mode,
                        };
                        relationships.insert(rel.id.clone(), rel);
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(RelsError::XmlError(e.to_string())),
                _ => {}
            }
            buf.clear();
        }

        Ok(Self { relationships })
    }

    /// Get a relationship by its ID.
    pub fn get(&self, id: &str) -> Option<&Relationship> {
        self.relationships.get(id)
    }

    /// Get all relationships.
    pub fn all(&self) -> &HashMap<String, Relationship> {
        &self.relationships
    }

    /// Find relationships by type URI.
    pub fn by_type(&self, type_uri: &str) -> Vec<&Relationship> {
        self.relationships
            .values()
            .filter(|r| r.type_uri == type_uri)
            .collect()
    }
}
