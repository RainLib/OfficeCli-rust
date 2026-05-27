use handler_common::*;
use handler_common::output_format::{BinaryInfo, RawOptions};
use oxml::OxmlPackage;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::dom_types::{WordDom, WordNode, WordElementType};
use crate::navigation::{navigate_to_element, navigate_to_element_mut};
use crate::view::*;
use crate::text_offset::extract_text_with_offsets;
use crate::add::add_element;
use crate::mutations::{set_properties, remove_element, move_element};
use crate::query::query_elements;
use crate::raw::read_raw;

const DOCUMENT_PART: &str = "word/document.xml";
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

pub struct WordHandler {
    package: RefCell<OxmlPackage>,
    editable: bool,
}

impl WordHandler {
    pub fn open(path: &str, editable: bool) -> Result<Self, HandlerError> {
        let package = OxmlPackage::open(path, editable)
            .map_err(|e| HandlerError::OpenError(e.to_string()))?;
        Ok(Self { package: RefCell::new(package), editable })
    }

    /// Parse the document.xml from the ZIP package into a WordDom tree.
    fn parse_dom(&self) -> Result<WordDom, HandlerError> {
        let package = self.package.borrow();
        let xml = package.read_part_xml(DOCUMENT_PART)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        parse_document_xml(&xml)
    }

    /// Serialize the DOM back to XML and write it to the package.
    fn write_dom(&self, dom: &WordDom) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let xml = serialize_dom(dom);
        let mut package = self.package.borrow_mut();
        package.write_part_xml(DOCUMENT_PART, &xml)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        Ok(())
    }
}

impl DocumentHandler for WordHandler {
    fn format_name(&self) -> &str { "docx" }

    fn view_as_text(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        let dom = self.parse_dom()?;
        view_as_text(&dom, opts)
    }

    fn view_as_annotated(&self, opts: ViewOptions) -> Result<String, HandlerError> {
        let dom = self.parse_dom()?;
        view_as_annotated(&dom, opts)
    }

    fn view_as_outline(&self) -> Result<String, HandlerError> {
        let dom = self.parse_dom()?;
        view_as_outline(&dom)
    }

    fn view_as_stats(&self) -> Result<String, HandlerError> {
        let dom = self.parse_dom()?;
        view_as_stats(&dom)
    }

    fn view_as_issues(&self, issue_type: Option<&str>, limit: Option<usize>) -> Result<Vec<DocumentIssue>, HandlerError> {
        let dom = self.parse_dom()?;
        Ok(view_as_issues(&dom, issue_type, limit))
    }

    fn view_as_html(&self) -> Result<String, HandlerError> {
        let package = self.package.borrow();
        crate::html_preview::view_as_html(&package)
    }

    fn view_as_text_json(&self, opts: ViewOptions) -> Result<serde_json::Value, HandlerError> {
        let dom = self.parse_dom()?;
        view_as_text_json(&dom, opts)
    }

    fn view_as_outline_json(&self) -> Result<serde_json::Value, HandlerError> {
        let dom = self.parse_dom()?;
        view_as_outline_json(&dom)
    }

    fn view_as_stats_json(&self) -> Result<serde_json::Value, HandlerError> {
        let dom = self.parse_dom()?;
        view_as_stats_json(&dom)
    }

    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
        let dom = self.parse_dom()?;

        // Special case: root path "/" returns the document structure
        if path == "/" {
            let body = dom.body()
                .ok_or_else(|| HandlerError::PathNotFound("body element not found".to_string()))?;
            let mut root_node = DocumentNode::new("/", "document");

            if depth > 0 {
                // Show top-level body children
                let mut children = Vec::new();
                let mut type_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for child in &body.children {
                    let type_name_str = child.element_type.to_path_name();
                    let type_name = type_name_str.to_string();
                    let idx = *type_counts.entry(type_name.clone()).or_insert(0);
                    *type_counts.get_mut(&type_name).unwrap() += 1;
                    let child_path = format!("/body/{}[{}]", type_name_str, idx);
                    let text = child.paragraph_text();
                    let preview = if text.len() > 80 {
                        format!("{}...", text.chars().take(80).collect::<String>())
                    } else if !text.is_empty() {
                        text.clone()
                    } else {
                        String::new()
                    };
                    children.push(
                        DocumentNode::new(&child_path, type_name_str)
                            .with_text(&text)
                            .with_preview(&preview)
                    );
                }
                root_node = root_node.with_children(children);
            }
            return Ok(root_node);
        }

        let node = navigate_to_element(&dom, path)?;

        let element_type_str = node.element_type.to_path_name();
        let text = node.paragraph_text();
        let preview = if text.len() > 80 {
            Some(format!("{}...", text.chars().take(80).collect::<String>()))
        } else if !text.is_empty() {
            Some(text.clone())
        } else {
            None
        };

        let style = node.heading_level().map(|l| {
            if l == 0 { "Title".to_string() } else { format!("Heading{}", l) }
        });

        let mut doc_node = DocumentNode::new(path, element_type_str);

        if !text.is_empty() {
            doc_node = doc_node.with_text(&text);
        }
        if let Some(p) = preview {
            doc_node = doc_node.with_preview(&p);
        }
        if let Some(s) = style {
            doc_node = doc_node.with_style(&s);
        }

        doc_node.child_count = node.children.len();

        // Add format properties for paragraphs
        if node.element_type == WordElementType::Paragraph {
            if let Some(ppr) = node.paragraph_properties() {
                for child in &ppr.children {
                    if let WordElementType::Unknown(ref name) = child.element_type {
                        if name == "pStyle" {
                            if let Some(val) = child.attributes.get("val") {
                                doc_node = doc_node.with_format("style", serde_json::Value::String(val.clone()));
                            }
                        }
                        if name == "jc" {
                            if let Some(val) = child.attributes.get("val") {
                                doc_node = doc_node.with_format("alignment", serde_json::Value::String(val.clone()));
                            }
                        }
                    }
                }
            }
        }

        // Add format properties for runs
        if node.element_type == WordElementType::Run {
            if let Some(rpr) = node.run_properties() {
                for child in &rpr.children {
                    let name = child.element_type.to_local_name();
                    if name == "b" {
                        doc_node = doc_node.with_format("bold", serde_json::Value::Bool(true));
                    }
                    if name == "i" {
                        doc_node = doc_node.with_format("italic", serde_json::Value::Bool(true));
                    }
                    if name == "u" {
                        if let Some(val) = child.attributes.get("val") {
                            doc_node = doc_node.with_format("underline", serde_json::Value::String(val.clone()));
                        }
                    }
                    if name == "sz" {
                        if let Some(val) = child.attributes.get("val") {
                            if let Ok(hp) = val.parse::<f32>() {
                                doc_node = doc_node.with_format("fontSize", serde_json::Value::Number(
                                    serde_json::Number::from_f64(hp as f64 / 2.0).unwrap_or(serde_json::Number::from(12))
                                ));
                            }
                        }
                    }
                    if name == "color" {
                        if let Some(val) = child.attributes.get("val") {
                            doc_node = doc_node.with_format("color", serde_json::Value::String(val.clone()));
                        }
                    }
                    if name == "rFonts" {
                        if let Some(val) = child.attributes.get("ascii") {
                            doc_node = doc_node.with_format("font", serde_json::Value::String(val.clone()));
                        }
                    }
                }
            }
        }

        // Build children if depth > 0
        if depth > 0 {
            let children = build_children_nodes(&node, path, depth - 1);
            doc_node = doc_node.with_children(children);
        }

        Ok(doc_node)
    }

    fn query(&self, selector: &str) -> Result<Vec<DocumentNode>, HandlerError> {
        let dom = self.parse_dom()?;
        query_elements(&dom, selector)
    }

    fn set(&self, path: &str, properties: &HashMap<String, String>) -> Result<Vec<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let mut dom = self.parse_dom()?;
        let result = set_properties(&mut dom, path, properties)?;
        self.write_dom(&dom)?;
        Ok(result)
    }

    fn add(
        &self,
        parent: &str,
        element_type: &str,
        position: InsertPosition,
        properties: &HashMap<String, String>,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let mut dom = self.parse_dom()?;
        let new_path = add_element(&mut dom, parent, element_type, position, properties)?;
        self.write_dom(&dom)?;
        Ok(new_path)
    }

    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let mut dom = self.parse_dom()?;
        let result = remove_element(&mut dom, path)?;
        self.write_dom(&dom)?;
        Ok(result)
    }

    fn move_element(
        &self,
        source: &str,
        target_parent: Option<&str>,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let mut dom = self.parse_dom()?;
        let new_path = move_element(&mut dom, source, target_parent, position)?;
        self.write_dom(&dom)?;
        Ok(new_path)
    }

    fn copy_from(
        &self,
        source: &str,
        target_parent: &str,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let mut dom = self.parse_dom()?;
        let source_node = navigate_to_element(&dom, source)?.clone();
        let elem_type = source_node.element_type.to_path_name();
        let new_path = add_element(&mut dom, target_parent, elem_type, position, &HashMap::new())?;
        let target_node = navigate_to_element_mut(&mut dom, &new_path)?;
        *target_node = source_node;
        self.write_dom(&dom)?;
        Ok(new_path)
    }

    fn raw(&self, part_path: &str, opts: RawOptions) -> Result<String, HandlerError> {
        let package = self.package.borrow();
        read_raw(&*package, part_path, opts)
    }

    fn raw_set(
        &self,
        part_path: &str,
        xpath: &str,
        action: &str,
        xml: Option<&str>,
    ) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let mut package = self.package.borrow_mut();
        crate::raw::apply_raw_set(&mut *package, part_path, xpath, action, xml)
    }

    fn add_part(
        &self,
        parent: &str,
        part_type: &str,
        properties: Option<&HashMap<String, String>>,
    ) -> Result<(String, String), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed("document opened in read-only mode".to_string()));
        }
        let mut package = self.package.borrow_mut();
        crate::raw::add_part(&mut *package, parent, part_type, properties)
    }

    fn validate(&self) -> Result<Vec<ValidationError>, HandlerError> {
        let dom = self.parse_dom()?;
        let mut errors = Vec::new();
        if dom.body().is_none() {
            errors.push(ValidationError {
                error_type: "structure".to_string(),
                description: "document.xml missing w:body element".to_string(),
                path: None,
                part: Some(DOCUMENT_PART.to_string()),
            });
        }
        Ok(errors)
    }

    fn try_extract_binary(&self, path: &str, dest: &str) -> Result<Option<BinaryInfo>, HandlerError> {
        // Resolve path to a part in the package (e.g. images are in word/media/)
        let pkg = self.package.borrow();
        let content_types = pkg.content_types();

        // Search for media parts matching the path hint
        let media_path: Option<String> = if path.starts_with("/image") || path.contains("image") {
            // Try to find an image part in word/media/
            let parts = pkg.list_parts();
            // If path is like /image[N], find the Nth image
            if let Some(idx_str) = path.strip_prefix("/image[").and_then(|s| s.strip_suffix(']')) {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let image_parts: Vec<&String> = parts.into_iter()
                        .filter(|p| p.starts_with("word/media/"))
                        .collect();
                    if idx > 0 && idx <= image_parts.len() {
                        Some(image_parts[idx - 1].to_string())
                    } else {
                        None
                    }
                } else { None }
            } else {
                // Try matching by name
                parts.into_iter()
                    .find(|p| p.starts_with("word/media/"))
                    .map(|p| p.to_string())
            }
        } else {
            // Try the path directly as a part path
            if pkg.has_part(path) { Some(path.to_string()) } else { None }
        };

        let part_path = media_path
            .ok_or_else(|| HandlerError::PathNotFound(format!("binary part for path '{}'", path)))?;

        let bytes = pkg.read_part_bytes(&part_path)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

        std::fs::write(dest, bytes)
            .map_err(|e| HandlerError::OperationFailed(format!("failed to write to '{}': {}", dest, e)))?;

        let content_type = content_types.content_type_for(&part_path)
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string());

        Ok(Some(BinaryInfo {
            content_type,
            byte_count: bytes.len(),
        }))
    }

    fn save(&self) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError("document opened in read-only mode".to_string()));
        }
        self.package.borrow_mut().save()
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        Ok(())
    }

    fn extract_text_with_offsets(&self) -> Result<TextOffsetMap, HandlerError> {
        let dom = self.parse_dom()?;
        extract_text_with_offsets(&dom)
    }
}

// ============================================================
// XML Parsing: Parse document.xml into WordDom tree using roxmltree
// ============================================================

fn parse_document_xml(xml: &str) -> Result<WordDom, HandlerError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| HandlerError::OperationFailed(format!("XML parse error: {}", e)))?;

    // Find the root element (should be w:document)
    let root = doc.root_element();
    let root_node = build_node_from_roxmltree(root);
    Ok(WordDom::new(root_node))
}

fn build_node_from_roxmltree(node: roxmltree::Node) -> WordNode {
    let local_name = node.tag_name().name();
    let ns = node.tag_name().namespace().unwrap_or("");

    let element_type = if ns == W_NS || ns.is_empty() {
        WordElementType::from_local_name(local_name)
    } else if local_name == "inline" && ns.starts_with("http://schemas.openxmlformats.org/drawingml") {
        WordElementType::InlineImage
    } else {
        WordElementType::Unknown(local_name.to_string())
    };

    let mut attrs = HashMap::new();
    for attr in node.attributes() {
        attrs.insert(attr.name().to_string(), attr.value().to_string());
    }

    let mut children = Vec::new();
    let mut text_content = String::new();

    for child in node.children() {
        if child.is_element() {
            children.push(build_node_from_roxmltree(child));
        } else if child.is_text() {
            text_content.push_str(child.text().unwrap_or(""));
        }
    }

    let mut word_node = WordNode::new(element_type.clone());
    word_node.attributes = attrs;

    // For w:t and delText, store text directly and clear children
    if element_type == WordElementType::Text || element_type == WordElementType::Unknown("delText".into()) {
        word_node.text_content = if text_content.is_empty() { None } else { Some(text_content) };
        word_node.children = Vec::new();
        if word_node.attributes.get("xml:space").map(|s| s.as_str()) == Some("preserve") {
            word_node.preserve_space = true;
        }
    } else {
        word_node.children = children;
    }

    word_node
}

// ============================================================
// XML Serialization: Serialize WordDom back to XML string
// ============================================================

fn serialize_dom(dom: &WordDom) -> String {
    let mut output = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    serialize_node_to_string(&mut output, &dom.root, true);
    output
}

fn serialize_node_to_string(output: &mut String, node: &WordNode, is_root: bool) {
    let local_name = node.element_type.to_local_name();
    let prefixed_name = if needs_w_prefix(&node.element_type) {
        format!("w:{}", local_name)
    } else {
        local_name.to_string()
    };

    // w:t and w:delText: text element
    if node.element_type == WordElementType::Text || node.element_type == WordElementType::Unknown("delText".into()) {
        let space_attr = if node.preserve_space || node.attributes.get("xml:space").map(|s| s.as_str()) == Some("preserve") {
            " xml:space=\"preserve\""
        } else {
            ""
        };
        output.push_str(&format!("<w:{}{}>", local_name, space_attr));
        if let Some(text) = &node.text_content {
            output.push_str(&escape_xml_text(text));
        }
        output.push_str(&format!("</w:{}>", local_name));
        return;
    }

    // Build attribute string
    let mut attr_str = String::new();
    if is_root && node.element_type == WordElementType::Document {
        attr_str.push_str(" xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"");
        attr_str.push_str(" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"");
        attr_str.push_str(" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\"");
    }
    for (key, val) in &node.attributes {
        attr_str.push_str(&format!(" {}=\"{}\"", escape_xml_text(key), escape_xml_text(val)));
    }

    if node.children.is_empty() && node.text_content.is_none() {
        // Self-closing empty element
        output.push_str(&format!("<{}{} />", prefixed_name, attr_str));
    } else {
        // Start tag + content + end tag
        output.push_str(&format!("<{}{}>", prefixed_name, attr_str));
        for child in &node.children {
            serialize_node_to_string(output, child, false);
        }
        if let Some(text) = &node.text_content {
            output.push_str(&escape_xml_text(text));
        }
        output.push_str(&format!("</{}>", prefixed_name));
    }
}

fn needs_w_prefix(element_type: &WordElementType) -> bool {
    match element_type {
        WordElementType::InlineImage => false,
        WordElementType::Unknown(_) => true, // default to w: prefix for unknowns in docx context
        _ => true,
    }
}

fn escape_xml_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Build children DocumentNode list from a WordNode.
fn build_children_nodes(node: &WordNode, parent_path: &str, depth: usize) -> Vec<DocumentNode> {
    let mut children = Vec::new();
    let mut type_counts: HashMap<String, usize> = HashMap::new();

    for child in &node.children {
        let name = child.element_type.to_path_name().to_string();
        let idx = type_counts.entry(name.clone()).or_insert(0);
        *idx += 1;

        let child_path = format!("{}/{}[{}]", parent_path, name, *idx);

        let element_type = child.element_type.to_path_name();
        let text = child.paragraph_text();
        let preview = if text.len() > 80 {
            Some(format!("{}...", text.chars().take(80).collect::<String>()))
        } else if !text.is_empty() {
            Some(text.clone())
        } else {
            None
        };

        let mut doc_node = DocumentNode::new(&child_path, element_type);
        if !text.is_empty() {
            doc_node = doc_node.with_text(&text);
        }
        if let Some(p) = preview {
            doc_node = doc_node.with_preview(&p);
        }
        doc_node.child_count = child.children.len();

        if depth > 0 {
            let sub_children = build_children_nodes(child, &child_path, depth - 1);
            doc_node = doc_node.with_children(sub_children);
        }

        children.push(doc_node);
    }

    children
}