use handler_common::output_format::{BinaryInfo, RawOptions};
use handler_common::*;
use oxml::OxmlPackage;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

use crate::add::add_element;
use crate::dom_types::{WordDom, WordElementType, WordNode};
use crate::mutations::{self, move_element, remove_element, set_properties, swap_elements};
use crate::navigation::{navigate_to_element, navigate_to_element_mut, parse_path};
use crate::query::query_elements;
use crate::raw::read_raw;
use crate::revision;
use crate::text_offset::extract_text_with_offsets;
use crate::view::*;

const DOCUMENT_PART: &str = "word/document.xml";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const C_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const O_NS: &str = "urn:schemas-microsoft-com:office:office";
const PIC_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const V_NS: &str = "urn:schemas-microsoft-com:vml";
const W10_NS: &str = "urn:schemas-microsoft-com:office:word";
const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
const W15_NS: &str = "http://schemas.microsoft.com/office/word/2012/wordml";
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const WP14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing";
const WPG_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const WPS_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

fn hyperlink_target(properties: &HashMap<String, String>) -> Option<&str> {
    properties
        .get("url")
        .or_else(|| properties.get("target"))
        .or_else(|| properties.get("link"))
        .map(String::as_str)
}

pub struct WordHandler {
    package: RefCell<OxmlPackage>,
    editable: bool,
}

#[derive(Clone)]
pub struct AddBatchItem {
    pub parent: String,
    pub element_type: String,
    pub position: InsertPosition,
    pub properties: HashMap<String, String>,
    pub wrap: Option<String>,
}

#[derive(Clone)]
pub struct SetRangeBatchItem {
    pub properties: HashMap<String, String>,
}

impl WordHandler {
    pub fn open(path: &str, editable: bool) -> Result<Self, HandlerError> {
        let package = OxmlPackage::open(path, editable)
            .map_err(|e| HandlerError::OpenError(e.to_string()))?;
        Ok(Self {
            package: RefCell::new(package),
            editable,
        })
    }

    /// Parse the document.xml from the ZIP package into a WordDom tree.
    fn parse_dom(&self) -> Result<WordDom, HandlerError> {
        let package = self.package.borrow();
        let xml = package
            .read_part_xml(DOCUMENT_PART)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        parse_document_xml(&xml)
    }

    /// Serialize the DOM back to XML and write it to the package.
    fn write_dom(&self, dom: &WordDom) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        let xml = serialize_dom(dom);
        let mut package = self.package.borrow_mut();
        package
            .write_part_xml(DOCUMENT_PART, &xml)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;
        Ok(())
    }

    pub fn add_batch(
        &self,
        items: &[AddBatchItem],
    ) -> Result<Vec<Result<String, String>>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }

        let mut dom = self.parse_dom()?;
        let mut results = Vec::with_capacity(items.len());
        let mut has_mutations = false;
        let mut bookmark_names = collect_bookmark_names(&dom);
        let mut next_bookmark_id = crate::helpers::max_bookmark_id(&dom) + 1;

        for item in items {
            if matches!(
                item.element_type.as_str(),
                "image"
                    | "drawing"
                    | "picture"
                    | "img"
                    | "chart"
                    | "chartSpace"
                    | "shape"
                    | "sp"
                    | "textbox"
                    | "txbx"
            ) {
                results.push(Err(format!(
                    "batch add fast path does not support part-aware element type: {}",
                    item.element_type
                )));
                continue;
            }

            let mut properties = item.properties.clone();
            if is_bookmark_add_type(&item.element_type) {
                match prepare_bookmark_batch_properties(
                    &properties,
                    &mut bookmark_names,
                    &mut next_bookmark_id,
                ) {
                    Ok(prepared) => properties = prepared,
                    Err(e) => {
                        results.push(Err(e.to_string()));
                        continue;
                    }
                }
            }

            match add_element(
                &mut dom,
                &item.parent,
                &item.element_type,
                item.position.clone(),
                &properties,
                item.wrap.as_deref(),
            ) {
                Ok(path) => {
                    has_mutations = true;
                    results.push(Ok(path));
                }
                Err(e) => results.push(Err(e.to_string())),
            }
        }

        if has_mutations {
            self.write_dom(&dom)?;
        }
        Ok(results)
    }

    pub fn set_range_batch(
        &self,
        items: &[SetRangeBatchItem],
    ) -> Result<Vec<Result<Vec<String>, String>>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }

        let mut dom = self.parse_dom()?;
        let mut results = Vec::with_capacity(items.len());
        let mut has_mutations = false;

        for item in items {
            let Some(range_paths_str) = item.properties.get("range_paths") else {
                results.push(Err("'range_paths' property is required".to_string()));
                continue;
            };
            let segments = match handler_common::parse_range_paths(range_paths_str) {
                Ok(segments) => segments,
                Err(e) => {
                    results.push(Err(format!("invalid range paths: {}", e)));
                    continue;
                }
            };
            match apply_docx_range_highlights(&mut dom, &item.properties, &segments) {
                Ok(unsupported) => {
                    has_mutations = true;
                    results.push(Ok(unsupported));
                }
                Err(e) => results.push(Err(e.to_string())),
            }
        }

        if has_mutations {
            self.write_dom(&dom)?;
        }
        Ok(results)
    }
}

fn is_bookmark_add_type(element_type: &str) -> bool {
    matches!(element_type, "bookmark" | "bookmarkStart" | "bookmarkstart")
}

fn prepare_bookmark_batch_properties(
    properties: &HashMap<String, String>,
    bookmark_names: &mut HashSet<String>,
    next_bookmark_id: &mut i32,
) -> Result<HashMap<String, String>, HandlerError> {
    let name = properties.get("name").cloned().unwrap_or_default();
    crate::helpers::validate_bookmark_name(&name)?;
    if !bookmark_names.insert(name.clone()) {
        return Err(HandlerError::InvalidArgument(format!(
            "bookmark name '{}' already exists; pick a unique name.",
            name
        )));
    }

    let mut prepared = properties.clone();
    prepared.insert(
        "__officecliBatchSkipDuplicateCheck".to_string(),
        "true".to_string(),
    );
    if !prepared.contains_key("id") {
        prepared.insert("id".to_string(), next_bookmark_id.to_string());
        *next_bookmark_id += 1;
    }
    Ok(prepared)
}

fn collect_bookmark_names(dom: &WordDom) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(body) = dom
        .root
        .children
        .iter()
        .find(|c| c.element_type == WordElementType::Body)
    {
        collect_bookmark_names_in_node(body, &mut names);
    }
    names
}

fn collect_bookmark_names_in_node(node: &WordNode, names: &mut HashSet<String>) {
    if node.element_type == WordElementType::BookmarkStart {
        if let Some(name) = node.attributes.get("name") {
            names.insert(name.clone());
        }
    }
    for child in &node.children {
        collect_bookmark_names_in_node(child, names);
    }
}

impl DocumentHandler for WordHandler {
    fn format_name(&self) -> &str {
        "docx"
    }

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

    fn view_as_issues(
        &self,
        issue_type: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<DocumentIssue>, HandlerError> {
        let dom = self.parse_dom()?;
        Ok(view_as_issues(&dom, issue_type, limit))
    }

    fn view_as_html(&self, _opts: ViewOptions) -> Result<String, HandlerError> {
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
        let mut stats = view_as_stats_json(&dom)?;
        // Merge docProps/app.xml extended properties so `view --mode stats --json`
        // surfaces Template / Manager / Company / Application / etc. alongside
        // the body counts. See handler_common::extended_properties.
        let package = self.package.borrow();
        if let Ok(app_xml) = package.read_part_bytes("docProps/app.xml") {
            let mut node = DocumentNode::new("/", "root");
            handler_common::extended_properties::populate_extended_properties(
                Some(app_xml.as_slice()),
                &mut node,
            );
            if let serde_json::Value::Object(ref mut map) = stats {
                if !node.format.is_empty() {
                    let mut extended = serde_json::Map::new();
                    for (k, v) in node.format.iter() {
                        if let Some(val) = v {
                            extended.insert(k.clone(), val.clone());
                        }
                    }
                    map.insert("extended".into(), serde_json::Value::Object(extended));
                }
            }
        }
        Ok(stats)
    }

    fn view_as_forms(&self) -> Result<String, HandlerError> {
        let dom = self.parse_dom()?;
        crate::view::view_as_forms(&dom)
    }

    fn get(&self, path: &str, depth: usize) -> Result<DocumentNode, HandlerError> {
        if path.starts_with("/revision") {
            let dom = self.parse_dom()?;
            return revision::get_revision(&dom, path);
        }
        if path == "/comments" || path.starts_with("/comments/") {
            let package = self.package.borrow();
            if path == "/comments" {
                let comments = mutations::list_comments(&package)?;
                let mut root = DocumentNode::new("/comments", "comments");
                if depth > 0 {
                    root =
                        root.with_children(comments.iter().map(comment_to_document_node).collect());
                }
                return Ok(root);
            }
            if path.contains("]/") {
                return mutations::get_comment_body_element(&package, path, depth);
            }
            return Ok(comment_to_document_node(&mutations::get_comment(
                &package, path,
            )?));
        }
        if path == "/numbering" || path.starts_with("/numbering/") {
            let package = self.package.borrow();
            return get_numbering_node(&package, path, depth);
        }
        if let Some((paragraph_path, tab_index)) = parse_tabstop_path(path) {
            let dom = self.parse_dom()?;
            return tabstop_to_document_node(&dom, path, &paragraph_path, tab_index);
        }
        if mutations::is_drawing_shape_path(path) {
            return mutations::get_drawing_shape(&self.package.borrow(), path);
        }
        if mutations::is_drawing_group_path(path) {
            return mutations::get_drawing_group(&self.package.borrow(), path);
        }
        let dom = self.parse_dom()?;

        // Special case: root path "/" returns the document structure
        if path == "/" {
            let body = dom
                .body()
                .ok_or_else(|| HandlerError::PathNotFound("body element not found".to_string()))?;
            let mut root_node = DocumentNode::new("/", "document");

            if depth > 0 {
                // Show top-level body children
                let mut children = Vec::new();
                let mut type_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
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
                            .with_preview(&preview),
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
            if l == 0 {
                "Title".to_string()
            } else {
                format!("Heading{}", l)
            }
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

        // Preserve readback for OOXML markers represented as generic DOM
        // nodes (permission ranges and similar structural extensions).
        if matches!(&node.element_type, WordElementType::Unknown(_)) {
            for (key, value) in &node.attributes {
                doc_node = doc_node.with_format(key, serde_json::Value::String(value.clone()));
            }
        }

        // Add format properties for paragraphs
        if node.element_type == WordElementType::Paragraph {
            if let Some(ppr) = node.paragraph_properties() {
                for child in &ppr.children {
                    if let WordElementType::Unknown(ref name) = child.element_type {
                        if name == "pStyle" {
                            if let Some(val) = child.attributes.get("val") {
                                doc_node = doc_node
                                    .with_format("style", serde_json::Value::String(val.clone()));
                            }
                        }
                        if name == "jc" {
                            if let Some(val) = child.attributes.get("val") {
                                doc_node = doc_node.with_format(
                                    "alignment",
                                    serde_json::Value::String(val.clone()),
                                );
                            }
                        }
                        if name == "numPr" {
                            for num_child in &child.children {
                                if let WordElementType::Unknown(ref num_name) =
                                    num_child.element_type
                                {
                                    let key = match num_name.as_str() {
                                        "numId" => Some("numId"),
                                        "ilvl" => Some("numLevel"),
                                        _ => None,
                                    };
                                    if let (Some(key), Some(value)) =
                                        (key, num_child.attributes.get("val"))
                                    {
                                        doc_node = doc_node.with_format(
                                            key,
                                            serde_json::Value::String(value.clone()),
                                        );
                                    }
                                }
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
                            doc_node = doc_node
                                .with_format("underline", serde_json::Value::String(val.clone()));
                        }
                    }
                    if name == "sz" {
                        if let Some(val) = child.attributes.get("val") {
                            if let Ok(hp) = val.parse::<f32>() {
                                doc_node = doc_node.with_format(
                                    "fontSize",
                                    serde_json::Value::Number(
                                        serde_json::Number::from_f64(hp as f64 / 2.0)
                                            .unwrap_or(serde_json::Number::from(12)),
                                    ),
                                );
                            }
                        }
                    }
                    if name == "color" {
                        if let Some(val) = child.attributes.get("val") {
                            doc_node = doc_node
                                .with_format("color", serde_json::Value::String(val.clone()));
                        }
                    }
                    if name == "rFonts" {
                        if let Some(val) = child.attributes.get("ascii") {
                            doc_node = doc_node
                                .with_format("font", serde_json::Value::String(val.clone()));
                        }
                    }
                }
            }
        }

        // Build children if depth > 0
        if depth > 0 {
            let children = build_children_nodes(node, path, depth - 1);
            doc_node = doc_node.with_children(children);
        }

        Ok(doc_node)
    }

    fn query(&self, selector: &str) -> Result<Vec<DocumentNode>, HandlerError> {
        let parsed = Selector::parse(selector)
            .map_err(|e| HandlerError::InvalidArgument(format!("invalid selector: {}", e)))?;
        if matches!(parsed.element_type.as_deref(), Some("comment" | "comments")) {
            let package = self.package.borrow();
            return Ok(mutations::list_comments(&package)?
                .iter()
                .filter(|comment| comment_matches_selector(comment, selector, &parsed))
                .map(comment_to_document_node)
                .collect());
        }
        if matches!(parsed.element_type.as_deref(), Some("revision")) {
            let dom = self.parse_dom()?;
            return Ok(revision::query_revisions(&dom, &parsed));
        }
        if matches!(
            parsed.element_type.as_deref(),
            Some("abstractNum" | "abstractnum" | "num")
        ) {
            let package = self.package.borrow();
            let root = get_numbering_node(&package, "/numbering", 1)?;
            let wanted_type = parsed.element_type.as_deref().unwrap();
            let wanted_type = if wanted_type.eq_ignore_ascii_case("abstractnum") {
                "abstractNum"
            } else {
                wanted_type
            };
            return Ok(root
                .children
                .into_iter()
                .filter(|node| node.element_type == wanted_type)
                .filter(|node| {
                    parsed.attributes.iter().all(|(key, value)| {
                        (key == "id" || key == "numId" || key == "abstractNumId")
                            && node
                                .format
                                .get(key)
                                .and_then(|value| value.as_ref())
                                .and_then(|value| value.as_str())
                                == Some(value)
                    })
                })
                .collect());
        }
        let dom = self.parse_dom()?;
        let mut results = query_elements(&dom, selector)?;
        if matches!(
            parsed.element_type.as_deref(),
            Some("p" | "paragraph" | "r" | "run")
        ) {
            results.extend(mutations::query_comment_body_elements(
                &self.package.borrow(),
                selector,
            )?);
        }
        Ok(results)
    }

    fn set(
        &self,
        path: &str,
        properties: &HashMap<String, String>,
    ) -> Result<Vec<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        // Find/replace and range edits can legitimately target the whole document
        // or pass an empty path because their target lives in the property map.
        if !properties.contains_key("find") && !properties.contains_key("range_paths") {
            handler_common::ensure_scoped(path, "set")?;
        }

        // Part-aware routing: /styles, comments, footnotes, endnotes live in
        // separate parts (word/styles.xml, comments.xml, footnotes.xml,
        // endnotes.xml). These need raw-package access, not the WordDom.
        let path_lc = path.trim().to_lowercase();
        if properties.contains_key("revision.action") {
            let mut dom = self.parse_dom()?;
            let result = revision::apply_revision_action(&mut dom, path, properties)?;
            self.write_dom(&dom)?;
            return Ok(result);
        }
        if properties.contains_key("revision.type") && !properties.contains_key("find") {
            let mut dom = self.parse_dom()?;
            if properties
                .get("revision.type")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("format"))
            {
                let snapshot = crate::add::format_revision_snapshot(&dom, path)?;
                let regular_properties: HashMap<String, String> = properties
                    .iter()
                    .filter(|(key, _)| !key.starts_with("revision."))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                if regular_properties.is_empty() {
                    return Err(HandlerError::InvalidArgument(
                        "revision.type=format requires at least one regular format property"
                            .to_string(),
                    ));
                }
                let result = set_properties(&mut dom, path, &regular_properties)?;
                crate::add::mark_existing_format_revision(&mut dom, path, properties, snapshot)?;
                self.write_dom(&dom)?;
                return Ok(result);
            }
            let result = crate::add::wrap_existing_run_revision(&mut dom, path, properties)?;
            self.write_dom(&dom)?;
            return Ok(result);
        }
        if path_lc.starts_with("/styles") {
            return mutations::set_style_on_part(&mut self.package.borrow_mut(), path, properties);
        }
        if path_lc.starts_with("/docdefaults") {
            return mutations::set_doc_defaults_on_part(&mut self.package.borrow_mut(), properties);
        }
        if path_lc.starts_with("/settings") {
            return mutations::set_settings_on_part(&mut self.package.borrow_mut(), properties);
        }
        if path_lc.starts_with("/numbering/") {
            if parse_numbering_level_path(path).is_some() {
                return mutations::set_numbering_level(
                    &mut self.package.borrow_mut(),
                    path,
                    properties,
                );
            }
            return mutations::set_numbering_definition(
                &mut self.package.borrow_mut(),
                path,
                properties,
            );
        }
        if path_lc.starts_with("/comments") || path_lc.contains("/comment[") {
            return mutations::set_comment_on_part(
                &mut self.package.borrow_mut(),
                "word/comments.xml",
                path,
                properties,
            );
        }
        if path_lc.starts_with("/footnotes") {
            return mutations::set_footnote_endnote_on_part(
                &mut self.package.borrow_mut(),
                "word/footnotes.xml",
                path,
                properties,
            );
        }
        if path_lc.starts_with("/endnotes") {
            return mutations::set_footnote_endnote_on_part(
                &mut self.package.borrow_mut(),
                "word/endnotes.xml",
                path,
                properties,
            );
        }
        if mutations::is_drawing_shape_path(path) {
            return mutations::set_drawing_shape(&mut self.package.borrow_mut(), path, properties);
        }
        if mutations::is_drawing_group_path(path) {
            return mutations::set_drawing_group(&mut self.package.borrow_mut(), path, properties);
        }
        if let Some((paragraph_path, tab_index)) = parse_tabstop_path(path) {
            let mut dom = self.parse_dom()?;
            let unsupported =
                set_tabstop_properties(&mut dom, &paragraph_path, tab_index, properties)?;
            self.write_dom(&dom)?;
            return Ok(unsupported);
        }

        let mut dom = self.parse_dom()?;
        let result = if let Some(range_paths_str) = properties.get("range_paths") {
            let segments = handler_common::parse_range_paths(range_paths_str).map_err(|e| {
                HandlerError::InvalidArgument(format!("invalid range paths: {}", e))
            })?;
            apply_docx_range_highlights(&mut dom, properties, &segments)?
        } else {
            set_properties(&mut dom, path, properties)?
        };
        if path_lc.contains("/hyperlink[") {
            if let Some(target) = hyperlink_target(properties) {
                let relationship_id = mutations::add_document_hyperlink_relationship(
                    &mut self.package.borrow_mut(),
                    target,
                )?;
                mutations::set_hyperlink_relationship_id(&mut dom, path, &relationship_id)?;
            }
        }
        self.write_dom(&dom)?;
        Ok(result)
    }

    fn add(
        &self,
        parent: &str,
        element_type: &str,
        position: InsertPosition,
        properties: &HashMap<String, String>,
        wrap: Option<&str>,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        // Image requires part-aware work (word/media + rels + Content Types).
        // Route before WordDom parsing so we can wire the OOXML package.
        if matches!(element_type, "image" | "drawing" | "picture" | "img") {
            return mutations::add_image_part_aware(
                &mut self.package.borrow_mut(),
                parent,
                properties,
            );
        }
        // Charts likewise need word/charts/chartN.xml + rels + Content Types.
        if matches!(element_type, "chart" | "chartSpace") {
            return mutations::add_chart_part_aware(
                &mut self.package.borrow_mut(),
                parent,
                properties,
            );
        }
        if matches!(element_type, "shape" | "sp" | "textbox" | "txbx") {
            return mutations::add_drawing_shape_part_aware(
                &mut self.package.borrow_mut(),
                parent,
                element_type,
                properties,
            );
        }
        if matches!(element_type, "diagram" | "flowchart") {
            return mutations::add_diagram_part_aware(
                &mut self.package.borrow_mut(),
                parent,
                properties,
            );
        }
        if matches!(element_type, "comment" | "comments") {
            return mutations::add_comment_part_aware(
                &mut self.package.borrow_mut(),
                parent,
                properties,
            );
        }
        if parent.starts_with("/comments/comment[") {
            return mutations::add_comment_body_element(
                &mut self.package.borrow_mut(),
                parent,
                element_type,
                position,
                properties,
            );
        }
        if matches!(element_type, "abstractNum" | "abstractnum" | "num") {
            return mutations::add_numbering_definition(
                &mut self.package.borrow_mut(),
                parent,
                element_type,
                properties,
            );
        }
        if matches!(element_type, "level" | "lvl") && parent.starts_with("/numbering/") {
            return mutations::add_numbering_level(
                &mut self.package.borrow_mut(),
                parent,
                properties,
            );
        }
        let mut dom = self.parse_dom()?;
        let new_path = add_element(&mut dom, parent, element_type, position, properties, wrap)?;
        if element_type.eq_ignore_ascii_case("hyperlink")
            || element_type.eq_ignore_ascii_case("link")
        {
            if let Some(target) = hyperlink_target(properties) {
                let relationship_id = mutations::add_document_hyperlink_relationship(
                    &mut self.package.borrow_mut(),
                    target,
                )?;
                mutations::set_hyperlink_relationship_id(&mut dom, &new_path, &relationship_id)?;
            }
        }
        self.write_dom(&dom)?;
        Ok(new_path)
    }

    fn remove(&self, path: &str) -> Result<Option<String>, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        handler_common::ensure_scoped(path, "remove")?;
        if path.starts_with("/comments/") {
            if path.contains("]/") {
                mutations::remove_comment_body_element(&mut self.package.borrow_mut(), path)?;
                return Ok(Some(path.to_string()));
            }
            mutations::remove_comment_part_aware(&mut self.package.borrow_mut(), path)?;
            return Ok(None);
        }
        if parse_numbering_level_path(path).is_some() {
            mutations::remove_numbering_level(&mut self.package.borrow_mut(), path)?;
            return Ok(Some(path.to_string()));
        }
        if mutations::is_numbering_num_path(path) {
            mutations::remove_numbering_num(&mut self.package.borrow_mut(), path)?;
            return Ok(Some(path.to_string()));
        }
        if mutations::is_drawing_shape_path(path) {
            mutations::remove_drawing_shape(&mut self.package.borrow_mut(), path)?;
            return Ok(Some(path.to_string()));
        }
        if mutations::is_drawing_group_path(path) {
            mutations::remove_drawing_group(&mut self.package.borrow_mut(), path)?;
            return Ok(Some(path.to_string()));
        }
        if let Some((paragraph_path, tab_index)) = parse_tabstop_path(path) {
            let mut dom = self.parse_dom()?;
            remove_tabstop(&mut dom, &paragraph_path, tab_index)?;
            self.write_dom(&dom)?;
            return Ok(Some(path.to_string()));
        }
        let mut dom = self.parse_dom()?;
        let result = remove_element(&mut dom, path)?;
        self.write_dom(&dom)?;
        Ok(result)
    }

    fn remove_with_properties(
        &self,
        path: &str,
        properties: &HashMap<String, String>,
    ) -> Result<Option<String>, HandlerError> {
        let has_revision = properties
            .keys()
            .any(|key| key.starts_with("revision.") || key.starts_with("trackChange."));
        if !has_revision {
            return self.remove(path);
        }

        // C# exposes remove --prop revision.*. Retain the older
        // trackChange.* spelling as a compatibility alias while routing both
        // to the Rust handler's native revision model.
        let mut revision_properties = HashMap::new();
        for (key, value) in properties {
            let normalized = key
                .strip_prefix("trackChange.")
                .map_or(key.as_str(), |suffix| match suffix {
                    "author" => "revision.author",
                    "date" => "revision.date",
                    "id" => "revision.id",
                    "type" => "revision.type",
                    _ => key.as_str(),
                });
            revision_properties.insert(normalized.to_string(), value.clone());
        }
        revision_properties
            .entry("revision.type".to_string())
            .or_insert_with(|| "del".to_string());
        self.set(path, &revision_properties)?;
        Ok(None)
    }

    fn move_element(
        &self,
        source: &str,
        target_parent: Option<&str>,
        position: InsertPosition,
    ) -> Result<String, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
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
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        let mut dom = self.parse_dom()?;
        let source_node = navigate_to_element(&dom, source)?.clone();
        let elem_type = source_node.element_type.to_path_name();
        let new_path = add_element(
            &mut dom,
            target_parent,
            elem_type,
            position,
            &HashMap::new(),
            None,
        )?;
        let target_node = navigate_to_element_mut(&mut dom, &new_path)?;
        *target_node = source_node;
        self.write_dom(&dom)?;
        Ok(new_path)
    }

    fn swap(&self, path1: &str, path2: &str) -> Result<(String, String), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        let mut dom = self.parse_dom()?;
        let result = swap_elements(&mut dom, path1, path2)?;
        self.write_dom(&dom)?;
        Ok(result)
    }

    fn merge(&self, data: &HashMap<String, String>) -> Result<MergeResult, HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        let mut pkg = self.package.borrow_mut();
        let parts = template_merger::docx_merge_parts(&pkg);
        template_merger::merge_ooxml_parts(&mut pkg, &parts, "w:t", data)
    }

    fn raw(&self, part_path: &str, opts: RawOptions) -> Result<String, HandlerError> {
        let package = self.package.borrow();
        let resolved = resolve_docx_raw_part_path(&package, part_path)?;
        read_raw(&package, &resolved, opts)
    }

    fn raw_set(
        &self,
        part_path: &str,
        xpath: &str,
        action: &str,
        xml: Option<&str>,
    ) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        let mut package = self.package.borrow_mut();
        if part_path.eq_ignore_ascii_case("/comments") && action.eq_ignore_ascii_case("replace") {
            let replacement = xml.filter(|value| !value.is_empty()).ok_or_else(|| {
                HandlerError::InvalidArgument("/comments replace requires XML".to_string())
            })?;
            mutations::prepare_comments_raw_replace(&mut package)?;
            return package
                .write_part_xml("word/comments.xml", replacement)
                .map_err(|error| HandlerError::SaveError(error.to_string()));
        }
        if part_path.eq_ignore_ascii_case("/commentsExtended") {
            if !action.eq_ignore_ascii_case("replace") {
                return Err(HandlerError::InvalidArgument(format!(
                    "/commentsExtended supports only the 'replace' action (got '{action}')"
                )));
            }
            let replacement = xml.filter(|value| !value.is_empty()).ok_or_else(|| {
                HandlerError::InvalidArgument("/commentsExtended replace requires XML".to_string())
            })?;
            mutations::prepare_comments_extended_raw_replace(&mut package)?;
            return package
                .write_part_xml("word/commentsExtended.xml", replacement)
                .map_err(|error| HandlerError::SaveError(error.to_string()));
        }
        let resolved = resolve_docx_raw_part_path(&package, part_path)?;
        crate::raw::apply_raw_set(&mut package, &resolved, xpath, action, xml)
    }

    fn add_part(
        &self,
        parent: &str,
        part_type: &str,
        properties: Option<&HashMap<String, String>>,
    ) -> Result<(String, String), HandlerError> {
        if !self.editable {
            return Err(HandlerError::OperationFailed(
                "document opened in read-only mode".to_string(),
            ));
        }
        let mut package = self.package.borrow_mut();
        crate::raw::add_part(&mut package, parent, part_type, properties)
    }

    fn validate(&self) -> Result<Vec<ValidationError>, HandlerError> {
        let pkg = self.package.borrow();
        let mut errors = Vec::new();

        // Required part: word/document.xml
        if !pkg.has_part(DOCUMENT_PART) {
            errors.push(ValidationError {
                error_type: "missing-part".to_string(),
                description: "required main document part".to_string(),
                path: None,
                part: Some(DOCUMENT_PART.to_string()),
            });
            return Ok(errors);
        }

        let document = pkg
            .read_part_xml(DOCUMENT_PART)
            .map_err(|e| HandlerError::OperationFailed(format!("read document.xml: {}", e)))?;

        // Structural: must have <w:body>
        if !document.contains("<w:body") {
            errors.push(ValidationError {
                error_type: "structure".to_string(),
                description: "document.xml missing w:body element".to_string(),
                path: None,
                part: Some(DOCUMENT_PART.to_string()),
            });
        }

        // Build a set of declared style IDs so we can flag dangling pStyle refs.
        let declared_styles = pkg
            .read_part_xml("word/styles.xml")
            .map(|xml| extract_style_ids(&xml))
            .unwrap_or_default();

        // Walk document.xml looking for pStyle val=... and dangling rels.
        for (style_id, byte_offset) in extract_pstyle_refs(&document) {
            if !declared_styles.contains(style_id.as_str()) {
                errors.push(ValidationError {
                    error_type: "dangling-reference".to_string(),
                    description: format!("paragraph references unknown style '{}'", style_id),
                    path: Some(format!("word/document.xml#offset{}", byte_offset)),
                    part: Some(DOCUMENT_PART.to_string()),
                });
            }
        }

        // Hyperlink r:id and image r:embed — verify each resolves in
        // word/_rels/document.xml.rels.
        let rels_xml = pkg
            .read_part_xml("word/_rels/document.xml.rels")
            .unwrap_or_else(|_| "<Relationships/>".to_string());
        let declared_rel_ids = extract_rel_ids(&rels_xml);

        for rid in extract_unresolved_rids(&document, &declared_rel_ids) {
            errors.push(ValidationError {
                error_type: "dangling-reference".to_string(),
                description: format!(
                    "document.xml references relationship '{}' not present in document.xml.rels",
                    rid
                ),
                path: Some(format!("word/document.xml#rId={}", rid)),
                part: Some("word/_rels/document.xml.rels".to_string()),
            });
        }

        Ok(errors)
    }

    fn try_extract_binary(
        &self,
        path: &str,
        dest: &str,
    ) -> Result<Option<BinaryInfo>, HandlerError> {
        // Resolve a node's relationship first.  This is the normal CLI path
        // (for example `/body/p[1]/drawing[1]`); `/image[N]` remains a
        // compatibility shorthand for callers that enumerate media directly.
        let relationship_id = if path.starts_with("/image[") || path.contains("image") {
            None
        } else {
            let dom = self.parse_dom()?;
            let node = navigate_to_element(&dom, path)?;
            find_binary_relationship_id(node)
        };

        // Resolve path to a part in the package (e.g. images are in word/media/)
        let pkg = self.package.borrow();
        let content_types = pkg.content_types();

        // Search for media parts matching the path hint
        let media_path: Option<String> = if let Some(rel_id) = relationship_id {
            resolve_document_relationship_target(&pkg, &rel_id)
        } else if path.starts_with("/image") || path.contains("image") {
            // Try to find an image part in word/media/
            let parts = pkg.list_parts();
            // If path is like /image[N], find the Nth image
            if let Some(idx_str) = path
                .strip_prefix("/image[")
                .and_then(|s| s.strip_suffix(']'))
            {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    let image_parts: Vec<&String> = parts
                        .into_iter()
                        .filter(|p| p.starts_with("word/media/"))
                        .collect();
                    if idx > 0 && idx <= image_parts.len() {
                        Some(image_parts[idx - 1].to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                // Try matching by name
                parts
                    .into_iter()
                    .find(|p| p.starts_with("word/media/"))
                    .map(|p| p.to_string())
            }
        } else {
            // Try the path directly as a part path
            if pkg.has_part(path) {
                Some(path.to_string())
            } else {
                None
            }
        };

        let part_path = media_path.ok_or_else(|| {
            HandlerError::PathNotFound(format!("binary part for path '{}'", path))
        })?;

        let bytes = pkg
            .read_part_bytes(&part_path)
            .map_err(|e| HandlerError::OperationFailed(e.to_string()))?;

        std::fs::write(dest, bytes).map_err(|e| {
            HandlerError::OperationFailed(format!("failed to write to '{}': {}", dest, e))
        })?;

        let content_type = content_types
            .content_type_for(&part_path)
            .cloned()
            .unwrap_or_else(|| "application/octet-stream".to_string());

        Ok(Some(BinaryInfo {
            content_type,
            byte_count: bytes.len(),
        }))
    }

    fn save(&self) -> Result<(), HandlerError> {
        if !self.editable {
            return Err(HandlerError::SaveError(
                "document opened in read-only mode".to_string(),
            ));
        }
        self.package
            .borrow_mut()
            .save()
            .map_err(|e| HandlerError::SaveError(e.to_string()))?;
        Ok(())
    }

    fn extract_text_with_offsets(&self) -> Result<TextOffsetMap, HandlerError> {
        let dom = self.parse_dom()?;
        extract_text_with_offsets(&dom)
    }
}

/// C# batch resources address a few Word package parts by logical names.
/// Keep the physical part path accepted too, while accepting the logical names
/// makes a commentsExtended round-trip portable across both implementations.
fn resolve_docx_raw_part_path(
    package: &OxmlPackage,
    part_path: &str,
) -> Result<String, HandlerError> {
    let alias = part_path.trim_matches('/');
    match alias.to_ascii_lowercase().as_str() {
        "document" => return Ok("word/document.xml".to_string()),
        "styles" => return Ok("word/styles.xml".to_string()),
        "settings" => return Ok("word/settings.xml".to_string()),
        "numbering" => return Ok("word/numbering.xml".to_string()),
        "comments" => return Ok("word/comments.xml".to_string()),
        "commentsextended" => return Ok("word/commentsExtended.xml".to_string()),
        _ => {}
    }
    if alias == "theme" {
        return indexed_document_relationship(package, THEME_REL, 1, "theme");
    }
    for (prefix, rel_type) in [
        ("header", HEADER_REL),
        ("footer", FOOTER_REL),
        ("chart", CHART_REL),
    ] {
        if let Some(index) = semantic_part_index(alias, prefix) {
            return indexed_document_relationship(package, rel_type, index, prefix);
        }
    }
    Ok(alias.to_string())
}

const THEME_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
const HEADER_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
const FOOTER_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
const CHART_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";

fn semantic_part_index(path: &str, prefix: &str) -> Option<usize> {
    path.strip_prefix(prefix)
        .and_then(|suffix| suffix.strip_prefix('['))
        .and_then(|suffix| suffix.strip_suffix(']'))
        .and_then(|suffix| suffix.parse::<usize>().ok())
        .filter(|index| *index > 0)
}

fn indexed_document_relationship(
    package: &OxmlPackage,
    relationship_type: &str,
    index: usize,
    label: &str,
) -> Result<String, HandlerError> {
    let rels = package
        .part_rels("word/document.xml")
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut matches = rels.by_type(relationship_type);
    matches.sort_by(|left, right| left.id.cmp(&right.id));
    matches
        .into_iter()
        .nth(index - 1)
        .map(|rel| package.resolve_rel_target("word/document.xml", &rel.target))
        .ok_or_else(|| HandlerError::PathNotFound(format!("{}[{}]", label, index)))
}

#[cfg(test)]
mod raw_part_path_tests {
    use super::*;

    #[test]
    fn resolves_csharp_semantic_raw_relationship_paths() {
        let mut package = OxmlPackage::create("semantic-parts.docx");
        package.add_part("word/document.xml", b"<w:document/>");
        package.add_part(
            "word/_rels/document.xml.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header2.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer4.xml"/><Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart9.xml"/><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme3.xml"/></Relationships>"#,
        );
        package.add_part("word/header2.xml", b"<w:hdr/>");
        package.add_part("word/footer4.xml", b"<w:ftr/>");
        package.add_part("word/charts/chart9.xml", b"<c:chartSpace/>");
        package.add_part("word/theme/theme3.xml", b"<a:theme/>");

        assert_eq!(
            resolve_docx_raw_part_path(&package, "/theme").unwrap(),
            "word/theme/theme3.xml"
        );
        assert_eq!(
            resolve_docx_raw_part_path(&package, "/header[1]").unwrap(),
            "word/header2.xml"
        );
        assert_eq!(
            resolve_docx_raw_part_path(&package, "/footer[1]").unwrap(),
            "word/footer4.xml"
        );
        assert_eq!(
            resolve_docx_raw_part_path(&package, "/chart[1]").unwrap(),
            "word/charts/chart9.xml"
        );
    }
}

/// Find the relationship id backing a picture/OLE/media descendant.  Parsed
/// DOM attributes retain their local name, so both `r:embed` and `r:id` are
/// represented as `embed`/`id` respectively.
fn find_binary_relationship_id(node: &WordNode) -> Option<String> {
    node.attributes
        .get("embed")
        .or_else(|| node.attributes.get("id"))
        .filter(|value| value.starts_with("rId"))
        .cloned()
        .or_else(|| node.children.iter().find_map(find_binary_relationship_id))
}

/// Resolve a document.xml relationship to its package part.  OOXML targets
/// are relative to `word/document.xml`, hence `media/image1.png` becomes
/// `word/media/image1.png`.
fn resolve_document_relationship_target(package: &OxmlPackage, rel_id: &str) -> Option<String> {
    let rels = package.read_part_xml("word/_rels/document.xml.rels").ok()?;
    let marker = format!("Id=\"{}\"", rel_id);
    let relation_start = rels.find(&marker)?;
    let relation = &rels[rels[..relation_start].rfind("<Relationship")?..];
    let target_marker = "Target=\"";
    let target_start = relation.find(target_marker)? + target_marker.len();
    let target_end = relation[target_start..].find('"')? + target_start;
    let target = &relation[target_start..target_end];
    let normalized = target.trim_start_matches('/');
    let part_path = if normalized.starts_with("word/") {
        normalized.to_string()
    } else {
        format!("word/{}", normalized)
    };
    package.has_part(&part_path).then_some(part_path)
}

/// C# exposes paragraph tab stops through a flat path even though OOXML stores
/// them below `w:pPr/w:tabs`.  Keep that public path stable while retaining
/// the standard OOXML nesting internally.
fn parse_tabstop_path(path: &str) -> Option<(String, usize)> {
    let (paragraph_path, index) = path.rsplit_once("/tab[")?;
    let index = index.strip_suffix(']')?.parse::<usize>().ok()?;
    (index > 0 && paragraph_path.contains("/p[")).then(|| (paragraph_path.to_string(), index))
}

fn get_numbering_node(
    package: &OxmlPackage,
    path: &str,
    depth: usize,
) -> Result<DocumentNode, HandlerError> {
    let xml = package
        .read_part_xml("word/numbering.xml")
        .map_err(|_| HandlerError::PathNotFound("numbering definition not found".to_string()))?;
    let document = roxmltree::Document::parse(&xml).map_err(|error| {
        HandlerError::OperationFailed(format!("invalid numbering.xml: {}", error))
    })?;
    if let Some((abstract_id, level)) = parse_numbering_level_path(path) {
        let level_node = document
            .root_element()
            .children()
            .find(|node| {
                node.has_tag_name((W_NS, "abstractNum"))
                    && node.attribute((W_NS, "abstractNumId")) == Some(abstract_id.as_str())
            })
            .and_then(|abstract_num| {
                abstract_num.children().find(|node| {
                    node.has_tag_name((W_NS, "lvl"))
                        && node.attribute((W_NS, "ilvl")) == Some(level.as_str())
                })
            })
            .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
        let mut result =
            DocumentNode::new(path, "level").with_format("ilvl", serde_json::Value::String(level));
        if let Some(format) = level_node
            .children()
            .find(|node| node.has_tag_name((W_NS, "numFmt")))
            .and_then(|node| node.attribute((W_NS, "val")))
        {
            result = result.with_format("format", serde_json::Value::String(format.to_string()));
        }
        if let Some(text) = level_node
            .children()
            .find(|node| node.has_tag_name((W_NS, "lvlText")))
            .and_then(|node| node.attribute((W_NS, "val")))
        {
            result = result.with_format("text", serde_json::Value::String(text.to_string()));
        }
        if let Some(start) = level_node
            .children()
            .find(|node| node.has_tag_name((W_NS, "start")))
            .and_then(|node| node.attribute((W_NS, "val")))
        {
            result = result.with_format("start", serde_json::Value::String(start.to_string()));
        }
        for (tag, key) in [
            ("lvlRestart", "lvlRestart"),
            ("suff", "suff"),
            ("lvlJc", "justification"),
        ] {
            if let Some(value) = level_node
                .children()
                .find(|node| node.has_tag_name((W_NS, tag)))
                .and_then(|node| node.attribute((W_NS, "val")))
            {
                result = result.with_format(key, serde_json::Value::String(value.to_string()));
            }
        }
        if level_node
            .children()
            .any(|node| node.has_tag_name((W_NS, "isLgl")))
        {
            result = result.with_format("isLgl", serde_json::Value::Bool(true));
        }
        return Ok(result);
    }
    let defs: Vec<DocumentNode> = document
        .root_element()
        .children()
        .filter(|node| node.is_element())
        .filter_map(|node| match node.tag_name().name() {
            "abstractNum" => node.attribute((W_NS, "abstractNumId")).map(|id| {
                let mut item = DocumentNode::new(
                    &format!("/numbering/abstractNum[@id={}]", id),
                    "abstractNum",
                )
                .with_format("id", serde_json::Value::String(id.to_string()))
                .with_format(
                    "levelCount",
                    serde_json::Value::from(
                        node.children()
                            .filter(|child| child.has_tag_name("lvl"))
                            .count(),
                    ),
                );
                for (tag, key) in [
                    ("multiLevelType", "type"),
                    ("name", "name"),
                    ("styleLink", "styleLink"),
                    ("numStyleLink", "numStyleLink"),
                ] {
                    if let Some(value) = node
                        .children()
                        .find(|child| child.has_tag_name((W_NS, tag)))
                        .and_then(|child| child.attribute((W_NS, "val")))
                    {
                        item = item.with_format(key, serde_json::Value::String(value.to_string()));
                    }
                }
                item
            }),
            "num" => node.attribute((W_NS, "numId")).map(|id| {
                let mut item = DocumentNode::new(&format!("/numbering/num[@id={}]", id), "num")
                    .with_format("id", serde_json::Value::String(id.to_string()))
                    .with_format("numId", serde_json::Value::String(id.to_string()));
                if let Some(abstract_id) = node
                    .children()
                    .find(|child| child.has_tag_name("abstractNumId"))
                    .and_then(|child| child.attribute((W_NS, "val")))
                {
                    item = item.with_format(
                        "abstractNumId",
                        serde_json::Value::String(abstract_id.to_string()),
                    );
                }
                for override_node in node
                    .children()
                    .filter(|child| child.has_tag_name("lvlOverride"))
                {
                    if let (Some(level), Some(start)) = (
                        override_node.attribute((W_NS, "ilvl")),
                        override_node
                            .children()
                            .find(|child| child.has_tag_name("startOverride"))
                            .and_then(|child| child.attribute((W_NS, "val"))),
                    ) {
                        item = item.with_format(
                            &format!("startOverride.{}", level),
                            serde_json::Value::String(start.to_string()),
                        );
                    }
                }
                item
            }),
            _ => None,
        })
        .collect();
    if path == "/numbering" {
        let mut root = DocumentNode::new("/numbering", "numbering");
        if depth > 0 {
            root = root.with_children(defs);
        }
        return Ok(root);
    }
    defs.into_iter()
        .find(|item| item.path == path)
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))
}

fn parse_numbering_level_path(path: &str) -> Option<(String, String)> {
    let remaining = path.strip_prefix("/numbering/abstractNum[@id=")?;
    let (id, remaining) = remaining.split_once("]/level[")?;
    let level = remaining.strip_suffix(']')?;
    (level
        .parse::<u8>()
        .ok()
        .filter(|level| *level <= 8)
        .is_some())
    .then(|| (id.to_string(), level.to_string()))
}

fn tabstop_to_document_node(
    dom: &WordDom,
    path: &str,
    paragraph_path: &str,
    index: usize,
) -> Result<DocumentNode, HandlerError> {
    let paragraph = navigate_to_element(dom, paragraph_path)?;
    if paragraph.element_type != WordElementType::Paragraph {
        return Err(HandlerError::PathNotFound(format!(
            "tab stop parent '{}' is not a paragraph",
            paragraph_path
        )));
    }
    let tab = paragraph
        .children
        .iter()
        .find(|child| child.element_type == WordElementType::ParagraphProperties)
        .and_then(|ppr| {
            ppr.children.iter().find(|child| {
                matches!(&child.element_type, WordElementType::Unknown(name) if name == "tabs")
            })
        })
        .and_then(|tabs| {
            tabs.children
                .iter()
                .filter(|child| child.element_type == WordElementType::TabStop)
                .nth(index - 1)
        })
        .ok_or_else(|| HandlerError::PathNotFound(format!("tab stop '{}' not found", path)))?;

    let mut node = DocumentNode::new(path, "tab");
    for key in ["pos", "val", "leader"] {
        if let Some(value) = tab.attributes.get(key) {
            node = node.with_format(key, serde_json::Value::String(value.clone()));
        }
    }
    Ok(node)
}

fn tabstop_mut<'a>(
    dom: &'a mut WordDom,
    paragraph_path: &str,
    index: usize,
) -> Result<&'a mut WordNode, HandlerError> {
    let paragraph = navigate_to_element_mut(dom, paragraph_path)?;
    let ppr = paragraph
        .children
        .iter_mut()
        .find(|child| child.element_type == WordElementType::ParagraphProperties)
        .ok_or_else(|| HandlerError::PathNotFound("paragraph has no tab stops".to_string()))?;
    let tabs = ppr
        .children
        .iter_mut()
        .find(
            |child| matches!(&child.element_type, WordElementType::Unknown(name) if name == "tabs"),
        )
        .ok_or_else(|| HandlerError::PathNotFound("paragraph has no tab stops".to_string()))?;
    tabs.children
        .iter_mut()
        .filter(|child| child.element_type == WordElementType::TabStop)
        .nth(index - 1)
        .ok_or_else(|| HandlerError::PathNotFound(format!("tab stop {} not found", index)))
}

fn set_tabstop_properties(
    dom: &mut WordDom,
    paragraph_path: &str,
    index: usize,
    properties: &HashMap<String, String>,
) -> Result<Vec<String>, HandlerError> {
    let tab = tabstop_mut(dom, paragraph_path, index)?;
    let mut unsupported = Vec::new();
    for (key, value) in properties {
        match key.as_str() {
            "pos" | "position" => {
                tab.attributes.insert(
                    "pos".to_string(),
                    crate::add::parse_signed_twips(value)?.to_string(),
                );
            }
            "val" | "type" => {
                let value = value.to_lowercase();
                if !matches!(
                    value.as_str(),
                    "left"
                        | "center"
                        | "right"
                        | "decimal"
                        | "bar"
                        | "clear"
                        | "num"
                        | "start"
                        | "end"
                ) {
                    return Err(HandlerError::InvalidArgument(format!(
                        "invalid tab val '{}'",
                        value
                    )));
                }
                tab.attributes.insert("val".to_string(), value);
            }
            "leader" => {
                let value = value.to_lowercase();
                if !matches!(
                    value.as_str(),
                    "none" | "dot" | "heavy" | "hyphen" | "middledot" | "underscore"
                ) {
                    return Err(HandlerError::InvalidArgument(format!(
                        "invalid tab leader '{}'",
                        value
                    )));
                }
                tab.attributes.insert("leader".to_string(), value);
            }
            _ => unsupported.push(key.clone()),
        }
    }
    Ok(unsupported)
}

fn remove_tabstop(
    dom: &mut WordDom,
    paragraph_path: &str,
    index: usize,
) -> Result<(), HandlerError> {
    let paragraph = navigate_to_element_mut(dom, paragraph_path)?;
    let ppr = paragraph
        .children
        .iter_mut()
        .find(|child| child.element_type == WordElementType::ParagraphProperties)
        .ok_or_else(|| HandlerError::PathNotFound("paragraph has no tab stops".to_string()))?;
    let tabs = ppr
        .children
        .iter_mut()
        .find(
            |child| matches!(&child.element_type, WordElementType::Unknown(name) if name == "tabs"),
        )
        .ok_or_else(|| HandlerError::PathNotFound("paragraph has no tab stops".to_string()))?;
    let child_index = tabs
        .children
        .iter()
        .enumerate()
        .filter(|(_, child)| child.element_type == WordElementType::TabStop)
        .nth(index - 1)
        .map(|(index, _)| index)
        .ok_or_else(|| HandlerError::PathNotFound(format!("tab stop {} not found", index)))?;
    tabs.children.remove(child_index);
    Ok(())
}

fn comment_to_document_node(comment: &mutations::DocxComment) -> DocumentNode {
    let mut node = DocumentNode::new(
        &format!("/comments/comment[@commentId={}]", comment.id),
        "comment",
    )
    .with_text(&comment.text)
    .with_preview(&comment.text)
    .with_format("id", serde_json::Value::String(comment.id.clone()));
    if let Some(author) = &comment.author {
        node = node.with_format("author", serde_json::Value::String(author.clone()));
    }
    if let Some(initials) = &comment.initials {
        node = node.with_format("initials", serde_json::Value::String(initials.clone()));
    }
    if let Some(date) = &comment.date {
        node = node.with_format("date", serde_json::Value::String(date.clone()));
    }
    if let Some(anchor) = &comment.anchor {
        node = node.with_format("anchoredTo", serde_json::Value::String(anchor.clone()));
    }
    if let Some(parent_id) = &comment.parent_id {
        node = node.with_format("parentId", serde_json::Value::String(parent_id.clone()));
    }
    node = node.with_format("done", serde_json::Value::Bool(comment.done));
    node
}

fn comment_matches_selector(
    comment: &mutations::DocxComment,
    raw_selector: &str,
    selector: &Selector,
) -> bool {
    let mut values = HashMap::new();
    values.insert("id", comment.id.as_str());
    values.insert("commentId", comment.id.as_str());
    if let Some(author) = &comment.author {
        values.insert("author", author.as_str());
    }
    if let Some(initials) = &comment.initials {
        values.insert("initials", initials.as_str());
    }
    if let Some(date) = &comment.date {
        values.insert("date", date.as_str());
    }
    let done = if comment.done { "true" } else { "false" };
    values.insert("done", done);
    values.insert("resolved", done);
    if let Some(parent_id) = &comment.parent_id {
        values.insert("parentId", parent_id.as_str());
    }
    for (key, value) in &selector.attributes {
        if values.get(key.as_str()).copied() != Some(value.as_str()) {
            return false;
        }
    }
    for filter in &selector.style_shorthands {
        if let Some((key, value)) = filter.split_once('=') {
            if values.get(key).copied() != Some(value) {
                return false;
            }
        }
    }
    if let Some(needle) = raw_selector
        .split(":contains(")
        .nth(1)
        .and_then(|tail| tail.strip_suffix(')'))
    {
        let needle = needle.trim_matches(|c| c == '\'' || c == '"');
        if !comment.text.contains(needle) {
            return false;
        }
    }
    true
}

fn apply_docx_range_highlights(
    dom: &mut WordDom,
    properties: &HashMap<String, String>,
    segments: &[PathRangeSegment],
) -> Result<Vec<String>, HandlerError> {
    let mut unsupported = Vec::new();

    let mut format_props = properties.clone();
    format_props.remove("range_paths");
    // Range text replacement: when `text` is present, the selected region's text
    // is replaced. Color/highlight (and other rPr) still apply to the new run.
    let new_text = format_props.remove("text");
    // Only force the default yellow highlight for pure formatting (no text
    // replacement) and when no explicit background/highlight was requested.
    if new_text.is_none()
        && !format_props.contains_key("bgColor")
        && !format_props.contains_key("highlight")
        && !format_props.contains_key("bg")
    {
        format_props.insert("highlight".to_string(), "yellow".to_string());
    }

    let segments = resolve_docx_range_segments(dom, segments)?;

    let mut emitted_replacement_groups = HashSet::new();
    for seg in &segments {
        let replacement_group = new_text.as_ref().map(|_| replacement_group_path(&seg.path));
        let segment_text = new_text.as_deref().map(|text| {
            if replacement_group
                .as_ref()
                .is_some_and(|group| !emitted_replacement_groups.insert(group.clone()))
            {
                ""
            } else {
                text
            }
        });
        let target_node = navigate_to_element_mut(dom, &seg.path)?;
        match target_node.element_type {
            WordElementType::Paragraph => {
                apply_docx_paragraph_range(target_node, seg, segment_text, &format_props)?;
            }
            WordElementType::Run => {
                apply_docx_run_range(target_node, seg, segment_text, &format_props)?;
            }
            _ => {
                return Err(HandlerError::InvalidArgument(format!(
                    "Range path must point to a Paragraph or Run, found: {:?}",
                    target_node.element_type
                )));
            }
        }
    }

    for key in properties.keys() {
        if !matches!(
            key.as_str(),
            "range_paths"
                | "text"
                | "bgColor"
                | "highlight"
                | "bg"
                | "color"
                | "fontColor"
                | "bold"
                | "b"
                | "italic"
                | "i"
                | "underline"
                | "u"
                | "strike"
                | "strikeout"
                | "font"
                | "fontFamily"
                | "size"
                | "fontSize"
                | "shading"
                | "shd"
        ) {
            unsupported.push(key.clone());
        }
    }

    Ok(unsupported)
}

fn replacement_group_path(path: &str) -> String {
    extract_paragraph_path(path).unwrap_or_else(|_| path.to_string())
}

fn resolve_docx_range_segments(
    dom: &WordDom,
    segments: &[PathRangeSegment],
) -> Result<Vec<PathRangeSegment>, HandlerError> {
    let mut resolved = Vec::new();

    for seg in segments {
        resolved.extend(resolve_docx_range_segment(dom, seg)?);
    }

    if resolved.is_empty() {
        return Err(HandlerError::InvalidArgument(
            "range_paths did not resolve to any editable text".to_string(),
        ));
    }

    Ok(resolved)
}

fn resolve_docx_range_segment(
    dom: &WordDom,
    seg: &PathRangeSegment,
) -> Result<Vec<PathRangeSegment>, HandlerError> {
    if is_virtual_text_offset_path(&seg.path) {
        return Ok(Vec::new());
    }

    match navigate_to_element(dom, &seg.path) {
        Ok(node) => resolve_existing_range_segment(dom, node, seg),
        Err(path_err) => {
            if let Some(span_index) = parse_body_p_index(&seg.path) {
                return resolve_span_index_range_segment(dom, seg, span_index);
            }
            Err(path_err)
        }
    }
}

fn is_virtual_text_offset_path(path: &str) -> bool {
    path.ends_with("/sep") || path.ends_with("/break")
}

fn resolve_existing_range_segment(
    dom: &WordDom,
    node: &WordNode,
    seg: &PathRangeSegment,
) -> Result<Vec<PathRangeSegment>, HandlerError> {
    match node.element_type {
        WordElementType::Paragraph | WordElementType::Run => Ok(vec![seg.clone()]),
        WordElementType::Hyperlink => Ok(vec![resolve_hyperlink_range_to_paragraph_segment(
            dom, seg,
        )?]),
        WordElementType::TableCell => {
            resolve_cell_range_segments(node, &seg.path, seg.start, seg.end)
        }
        _ => Err(HandlerError::InvalidArgument(format!(
            "Range path must point to a Paragraph, Run, Hyperlink, or TableCell, found: {:?}",
            node.element_type
        ))),
    }
}

fn resolve_span_index_range_segment(
    dom: &WordDom,
    seg: &PathRangeSegment,
    span_index: usize,
) -> Result<Vec<PathRangeSegment>, HandlerError> {
    if span_index == 0 {
        return Err(HandlerError::InvalidArgument(format!(
            "range path '{}' uses 0-based span index; expected 1-based",
            seg.path
        )));
    }

    let map = extract_text_with_offsets(dom)?;
    let span = map.spans.get(span_index - 1).ok_or_else(|| {
        HandlerError::PathNotFound(format!(
            "span-index path '{}' out of range (max span index: {})",
            seg.path,
            map.spans.len()
        ))
    })?;

    match span.element_type.as_str() {
        "run" | "paragraph" | "cell" => {
            let mapped = PathRangeSegment {
                path: span.path.clone(),
                start: seg.start,
                end: seg.end,
            };
            resolve_docx_range_segment(dom, &mapped)
        }
        other => Err(HandlerError::InvalidArgument(format!(
            "span-index path '{}' points to non-editable {} span '{}'",
            seg.path, other, span.path
        ))),
    }
}

fn parse_body_p_index(path: &str) -> Option<usize> {
    let segments = parse_path(path).ok()?;
    if segments.len() == 2 && segments[0].name == "body" && segments[1].name == "p" {
        segments[1].index
    } else {
        None
    }
}

fn resolve_cell_range_segments(
    cell: &WordNode,
    cell_path: &str,
    seg_start: Option<usize>,
    seg_end: Option<usize>,
) -> Result<Vec<PathRangeSegment>, HandlerError> {
    let para_count = cell
        .children
        .iter()
        .filter(|child| child.element_type == WordElementType::Paragraph)
        .count();

    if para_count == 0 {
        return Err(HandlerError::InvalidArgument(format!(
            "range_paths target table cell '{}' has no paragraphs",
            cell_path
        )));
    }

    let total_len = cell_text_len(cell);
    let target_start = seg_start.unwrap_or(0);
    let target_end = seg_end.unwrap_or(total_len);

    if target_start > target_end {
        return Err(HandlerError::InvalidArgument(format!(
            "range {}[{}..{}] is reversed",
            cell_path, target_start, target_end
        )));
    }

    let mut ranges = Vec::new();
    let mut para_idx = 0;
    let mut cursor = 0;

    for child in &cell.children {
        if child.element_type != WordElementType::Paragraph {
            continue;
        }

        para_idx += 1;
        let text_len = child.paragraph_text().chars().count();
        let para_start = cursor;
        let para_end = para_start + text_len;

        let overlap_start = target_start.max(para_start);
        let overlap_end = target_end.min(para_end);
        if overlap_start < overlap_end || (text_len == 0 && target_start == para_start) {
            ranges.push(PathRangeSegment {
                path: format!("{}/p[{}]", cell_path, para_idx),
                start: Some(overlap_start.saturating_sub(para_start)),
                end: Some(overlap_end.saturating_sub(para_start)),
            });
        }

        cursor = para_end;
        if para_idx < para_count {
            cursor += 1;
        }
    }

    if ranges.is_empty() {
        return Err(HandlerError::InvalidArgument(format!(
            "range {}[{}..{}] did not overlap table cell text length {}",
            cell_path, target_start, target_end, total_len
        )));
    }

    Ok(ranges)
}

fn cell_text_len(cell: &WordNode) -> usize {
    let para_count = cell
        .children
        .iter()
        .filter(|child| child.element_type == WordElementType::Paragraph)
        .count();
    let text_len: usize = cell
        .children
        .iter()
        .filter(|child| child.element_type == WordElementType::Paragraph)
        .map(|child| child.paragraph_text().chars().count())
        .sum();

    text_len + para_count.saturating_sub(1)
}

fn resolve_hyperlink_range_to_paragraph_segment(
    dom: &WordDom,
    seg: &PathRangeSegment,
) -> Result<PathRangeSegment, HandlerError> {
    let para_path = extract_paragraph_path(&seg.path)?;
    let para_node = navigate_to_element(dom, &para_path)?;
    if para_node.element_type != WordElementType::Paragraph {
        return Err(HandlerError::InvalidArgument(format!(
            "cannot find paragraph for hyperlink range path '{}'",
            seg.path
        )));
    }

    let (node_start, node_end) = compute_text_range_in_paragraph(para_node, &para_path, &seg.path)?;
    let node_text_len = node_end.saturating_sub(node_start);

    Ok(PathRangeSegment {
        path: para_path,
        start: Some(node_start + seg.start.unwrap_or(0)),
        end: Some(node_start + seg.end.unwrap_or(node_text_len)),
    })
}

fn extract_paragraph_path(path: &str) -> Result<String, HandlerError> {
    let Some(pos) = path.rfind("/p[") else {
        return Err(HandlerError::InvalidArgument(format!(
            "cannot extract paragraph path from '{}'",
            path
        )));
    };
    let rest = &path[pos..];
    let Some(end) = rest.find(']') else {
        return Err(HandlerError::InvalidArgument(format!(
            "malformed paragraph path '{}'",
            path
        )));
    };
    Ok(path[..pos + end + 1].to_string())
}

fn compute_text_range_in_paragraph(
    para_node: &WordNode,
    para_path: &str,
    target_path: &str,
) -> Result<(usize, usize), HandlerError> {
    let mut offset = 0;
    if let Some(range) = find_text_range_by_path(para_node, para_path, target_path, &mut offset) {
        return Ok(range);
    }
    Err(HandlerError::PathNotFound(format!(
        "range path '{}' was not found in paragraph '{}'",
        target_path, para_path
    )))
}

fn find_text_range_by_path(
    node: &WordNode,
    current_path: &str,
    target_path: &str,
    offset: &mut usize,
) -> Option<(usize, usize)> {
    if current_path == target_path {
        let start = *offset;
        let len = node.paragraph_text().chars().count();
        *offset += len;
        return Some((start, start + len));
    }

    if node.element_type == WordElementType::Run {
        *offset += node.paragraph_text().chars().count();
        return None;
    }

    let mut type_counts: HashMap<String, usize> = HashMap::new();
    for child in &node.children {
        let name = child.element_type.to_path_name().to_string();
        let idx = type_counts.entry(name.clone()).or_insert(0);
        *idx += 1;
        let child_path = format!("{}/{}[{}]", current_path, name, *idx);
        if let Some(range) = find_text_range_by_path(child, &child_path, target_path, offset) {
            return Some(range);
        }
    }
    None
}

fn normalize_range_bounds(
    path: &str,
    start: usize,
    end: usize,
    total_text_len: usize,
    allow_suffix_fallback: bool,
) -> Result<(usize, usize), HandlerError> {
    if start <= end && start < total_text_len {
        return Ok((start, end));
    }

    let selection_len = end.saturating_sub(start);
    if allow_suffix_fallback && selection_len > 0 && selection_len <= total_text_len {
        return Ok((total_text_len - selection_len, total_text_len));
    }

    Err(HandlerError::InvalidArgument(format!(
        "range {}[{}..{}] did not overlap text length {}",
        path, start, end, total_text_len
    )))
}

fn apply_docx_paragraph_range(
    para_node: &mut WordNode,
    seg: &PathRangeSegment,
    new_text: Option<&str>,
    format_props: &HashMap<String, String>,
) -> Result<(), HandlerError> {
    // 1. Collect all runs under the paragraph with their index paths and text contents
    let mut collected_runs = Vec::new();
    let mut path_tracker = Vec::new();
    collect_run_locations(para_node, &mut path_tracker, &mut collected_runs);

    // 2. Map global character offsets to the runs
    let mut global_start = 0;
    let mut runs_with_spans = Vec::new();
    for (path, text) in collected_runs {
        let len = text.chars().count();
        let global_end = global_start + len;
        runs_with_spans.push((path, global_start, global_end, len));
        global_start = global_end;
    }

    let total_text_len = global_start;
    let (target_start, target_end) = normalize_range_bounds(
        &seg.path,
        seg.start.unwrap_or(0),
        seg.end.unwrap_or(total_text_len),
        total_text_len,
        new_text.is_some(),
    )?;

    if target_start >= target_end {
        return Err(HandlerError::InvalidArgument(format!(
            "range {}[{}..{}] is empty or reversed",
            seg.path, target_start, target_end
        )));
    }

    // For text replacement, only the first overlapping run receives the new
    // text; the selected portions of subsequent runs are removed so the new
    // text appears exactly once across the whole selection.
    let first_overlap_path = runs_with_spans
        .iter()
        .find(|(_p, r_start, r_end, _len)| (*r_start).max(target_start) < (*r_end).min(target_end))
        .map(|(p, _, _, _)| p.clone());

    if first_overlap_path.is_none() {
        return Err(HandlerError::InvalidArgument(format!(
            "range {}[{}..{}] did not overlap paragraph text length {}",
            seg.path, target_start, target_end, total_text_len
        )));
    }

    // 3. Process runs in reverse order to keep index paths stable
    for (path, r_start, r_end, _r_len) in runs_with_spans.into_iter().rev() {
        let overlap_start = r_start.max(target_start);
        let overlap_end = r_end.min(target_end);

        if overlap_start < overlap_end {
            let local_start = overlap_start - r_start;
            let local_end = overlap_end - r_start;

            let parent = get_node_mut_by_path(para_node, &path[..path.len() - 1]);
            let last_idx = path[path.len() - 1];

            let run = parent.children[last_idx].clone();
            let text = run.paragraph_text();

            // Convert char offsets to byte indices
            let byte_start = char_offset_to_byte_index(&text, local_start);
            let byte_end = char_offset_to_byte_index(&text, local_end);

            // Split run at byte offsets
            let (left, rest) = crate::helpers::split_run_at_offset(&run, byte_start);
            let mut mid = None;
            let mut right = None;
            if let Some(r) = rest {
                let (m, rg) = crate::helpers::split_run_at_offset(&r, byte_end - byte_start);
                mid = m;
                right = rg;
            }

            // Build the replacement run list for this run.
            let mut inserted_runs = Vec::new();
            if let Some(l) = left {
                inserted_runs.push(l);
            }
            match new_text {
                Some(nt) => {
                    // Text replacement: insert the new text only on the first
                    // overlapping run; drop the selected mid of all others.
                    if first_overlap_path.as_ref() == Some(&path) {
                        let mut new_run = crate::helpers::build_run_with_text(&run, nt);
                        merge_run_properties(&mut new_run, format_props);
                        inserted_runs.push(new_run);
                    }
                }
                None => {
                    if let Some(mut m) = mid {
                        merge_run_properties(&mut m, format_props);
                        inserted_runs.push(m);
                    }
                }
            }
            if let Some(rg) = right {
                inserted_runs.push(rg);
            }

            parent.children.splice(last_idx..=last_idx, inserted_runs);
        }
    }

    Ok(())
}

fn apply_docx_run_range(
    run_node: &mut WordNode,
    seg: &PathRangeSegment,
    new_text: Option<&str>,
    format_props: &HashMap<String, String>,
) -> Result<(), HandlerError> {
    let Some(nt) = new_text else {
        merge_run_properties(run_node, format_props);
        return Ok(());
    };

    let source_run = run_node.clone();
    let text = source_run.paragraph_text();

    let replacement = if seg.start.is_none() && seg.end.is_none() {
        nt.to_string()
    } else {
        let total_text_len = text.chars().count();
        let (target_start, target_end) = normalize_range_bounds(
            &seg.path,
            seg.start.unwrap_or(0),
            seg.end.unwrap_or(total_text_len),
            total_text_len,
            true,
        )?;

        if target_start >= target_end {
            return Err(HandlerError::InvalidArgument(format!(
                "range {}[{}..{}] did not overlap run text length {}",
                seg.path, target_start, target_end, total_text_len
            )));
        }

        let clamped_end = target_end.min(total_text_len);
        let byte_start = char_offset_to_byte_index(&text, target_start);
        let byte_end = char_offset_to_byte_index(&text, clamped_end);
        format!("{}{}{}", &text[..byte_start], nt, &text[byte_end..])
    };

    *run_node = crate::helpers::build_run_with_text(&source_run, &replacement);
    merge_run_properties(run_node, format_props);
    Ok(())
}

fn char_offset_to_byte_index(text: &str, offset: usize) -> usize {
    if offset == text.chars().count() {
        return text.len();
    }

    text.char_indices()
        .nth(offset)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn collect_run_locations(
    node: &WordNode,
    current_path: &mut Vec<usize>,
    runs: &mut Vec<(Vec<usize>, String)>,
) {
    if node.element_type == WordElementType::Run {
        runs.push((current_path.clone(), node.paragraph_text()));
        return;
    }
    if is_alternate_content_node(node) {
        if let Some(preferred_idx) = preferred_alternate_content_child_index(node) {
            current_path.push(preferred_idx);
            collect_run_locations(&node.children[preferred_idx], current_path, runs);
            current_path.pop();
        }
        return;
    }
    for (i, child) in node.children.iter().enumerate() {
        current_path.push(i);
        collect_run_locations(child, current_path, runs);
        current_path.pop();
    }
}

fn is_alternate_content_node(node: &WordNode) -> bool {
    matches!(
        &node.element_type,
        WordElementType::Unknown(local) if local == "AlternateContent"
    )
}

fn preferred_alternate_content_child_index(node: &WordNode) -> Option<usize> {
    node.children
        .iter()
        .position(
            |child| matches!(&child.element_type, WordElementType::Unknown(n) if n == "Choice"),
        )
        .or_else(|| {
            node.children.iter().position(|child| {
                matches!(&child.element_type, WordElementType::Unknown(n) if n == "Fallback")
            })
        })
}

fn get_node_mut_by_path<'a>(mut node: &'a mut WordNode, path: &[usize]) -> &'a mut WordNode {
    for &idx in path {
        node = &mut node.children[idx];
    }
    node
}

fn merge_run_properties(run: &mut WordNode, format_props: &HashMap<String, String>) {
    if let Some(new_rpr) = crate::helpers::build_run_properties(format_props) {
        if let Some(existing_rpr_idx) = run
            .children
            .iter()
            .position(|c| c.element_type == WordElementType::RunProperties)
        {
            let mut existing_rpr = run.children.remove(existing_rpr_idx);
            for new_child in new_rpr.children {
                existing_rpr
                    .children
                    .retain(|c| c.element_type != new_child.element_type);
                existing_rpr.children.push(new_child);
            }
            run.children.insert(existing_rpr_idx, existing_rpr);
        } else {
            run.children.insert(0, new_rpr);
        }
    }
}

// ============================================================
// XML Parsing: Parse document.xml into WordDom tree using roxmltree
// ============================================================

pub(crate) fn parse_document_xml(xml: &str) -> Result<WordDom, HandlerError> {
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
        if local_name == "tab"
            && node
                .parent()
                .is_some_and(|parent| parent.tag_name().name() == "tabs")
        {
            WordElementType::TabStop
        } else {
            WordElementType::from_local_name(local_name)
        }
    } else if local_name == "inline"
        && ns.starts_with("http://schemas.openxmlformats.org/drawingml")
    {
        WordElementType::InlineImage
    } else {
        WordElementType::Unknown(local_name.to_string())
    };

    let mut attrs = HashMap::new();
    let mut attr_namespaces = HashMap::new();
    for attr in node.attributes() {
        let is_xml_space = attr.namespace() == Some(XML_NS)
            || (local_name == "t" && attr.name() == "space" && attr.value() == "preserve");
        let key = if is_xml_space {
            format!("xml:{}", attr.name())
        } else {
            attr.name().to_string()
        };
        attrs.insert(key.clone(), attr.value().to_string());
        if is_xml_space {
            attr_namespaces.insert(key, XML_NS.to_string());
        } else if let Some(attr_ns) = attr.namespace() {
            attr_namespaces.insert(key, attr_ns.to_string());
        }
    }

    let mut namespace_declarations = HashMap::new();
    for namespace in node.namespaces() {
        namespace_declarations.insert(
            namespace.name().unwrap_or_default().to_string(),
            namespace.uri().to_string(),
        );
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
    if !ns.is_empty() {
        word_node.namespace = Some(ns.to_string());
    }
    word_node.attributes = attrs;
    word_node.attribute_namespaces = attr_namespaces;
    word_node.namespace_declarations = namespace_declarations;

    // For w:t and delText, store text directly and clear children
    if element_type == WordElementType::Text
        || element_type == WordElementType::Unknown("delText".into())
    {
        word_node.text_content = if text_content.is_empty() {
            None
        } else {
            Some(text_content)
        };
        word_node.children = Vec::new();
        if word_node.attributes.get("xml:space").map(|s| s.as_str()) == Some("preserve") {
            word_node.preserve_space = true;
        }
    } else {
        if !text_content.is_empty() && (children.is_empty() || !text_content.trim().is_empty()) {
            word_node.text_content = Some(text_content);
        }
        word_node.children = children;
    }

    word_node
}

// ============================================================
// XML Serialization: Serialize WordDom back to XML string
// ============================================================

pub(crate) fn serialize_dom(dom: &WordDom) -> String {
    let mut output =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    serialize_node_to_string(&mut output, &dom.root, true);
    output
}

fn serialize_node_to_string(output: &mut String, node: &WordNode, is_root: bool) {
    let prefixed_name = qualified_element_name(node);

    // w:t and w:delText: text element
    if node.element_type == WordElementType::Text
        || node.element_type == WordElementType::Unknown("delText".into())
    {
        let mut attr_str = build_attribute_string(node, is_root);
        if node.preserve_space && !has_xml_space_attr(node) {
            attr_str.push_str(" xml:space=\"preserve\"");
        }
        output.push_str(&format!("<{}{}>", prefixed_name, attr_str));
        if let Some(text) = &node.text_content {
            output.push_str(&escape_xml_text(text));
        }
        output.push_str(&format!("</{}>", prefixed_name));
        return;
    }

    let attr_str = build_attribute_string(node, is_root);

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

fn qualified_element_name(node: &WordNode) -> String {
    let local_name = node.element_type.to_local_name();
    if let Some(namespace) = &node.namespace {
        if let Some(prefix) = prefix_for_namespace(namespace, node) {
            if prefix.is_empty() {
                return local_name.to_string();
            }
            return format!("{}:{}", prefix, local_name);
        }
    }

    match node.element_type {
        WordElementType::InlineImage => format!("wp:{}", local_name),
        _ => format!("w:{}", local_name),
    }
}

fn build_attribute_string(node: &WordNode, is_root: bool) -> String {
    let mut parts = Vec::new();

    let namespace_declarations = namespace_declarations_for_node(node, is_root);
    for (prefix, uri) in namespace_declarations {
        let name = if prefix.is_empty() {
            "xmlns".to_string()
        } else {
            format!("xmlns:{}", prefix)
        };
        parts.push(format!("{}=\"{}\"", name, escape_xml_text(&uri)));
    }

    let mut attrs: Vec<(&String, &String)> = node.attributes.iter().collect();
    attrs.sort_by(|a, b| attribute_sort_key(node, a.0).cmp(&attribute_sort_key(node, b.0)));
    for (key, val) in attrs {
        parts.push(format!(
            "{}=\"{}\"",
            qualified_attribute_name(node, key),
            escape_xml_text(val)
        ));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(" "))
    }
}

fn namespace_declarations_for_node(node: &WordNode, is_root: bool) -> BTreeMap<String, String> {
    let mut declarations = BTreeMap::new();

    if is_root && node.element_type == WordElementType::Document {
        for (prefix, uri) in known_office_namespaces() {
            declarations.insert(prefix.to_string(), uri.to_string());
        }
    }

    for (prefix, uri) in &node.namespace_declarations {
        let known = known_office_namespaces()
            .iter()
            .any(|(_, known_uri)| *known_uri == uri);
        if prefix != "xml" && (is_root || !known) {
            declarations.insert(prefix.clone(), uri.clone());
        }
    }

    declarations
}

fn known_office_namespaces() -> &'static [(&'static str, &'static str)] {
    &[
        ("a", A_NS),
        ("c", C_NS),
        ("mc", MC_NS),
        ("o", O_NS),
        ("pic", PIC_NS),
        ("r", R_NS),
        ("v", V_NS),
        ("w", W_NS),
        ("w10", W10_NS),
        ("w14", W14_NS),
        ("w15", W15_NS),
        ("wp", WP_NS),
        ("wp14", WP14_NS),
        ("wpg", WPG_NS),
        ("wps", WPS_NS),
    ]
}

fn prefix_for_namespace(namespace: &str, node: &WordNode) -> Option<String> {
    if namespace == XML_NS {
        return Some("xml".to_string());
    }

    if let Some((prefix, _)) = known_office_namespaces()
        .iter()
        .find(|(_, uri)| *uri == namespace)
    {
        return Some((*prefix).to_string());
    }

    node.namespace_declarations
        .iter()
        .find(|(_, uri)| uri.as_str() == namespace)
        .map(|(prefix, _)| prefix.clone())
}

fn qualified_attribute_name(node: &WordNode, key: &str) -> String {
    if key.starts_with("xmlns") {
        return key.to_string();
    }
    if key.contains(':') {
        return key.to_string();
    }

    if let Some(namespace) = node.attribute_namespaces.get(key) {
        if let Some(prefix) = prefix_for_namespace(namespace, node) {
            if prefix.is_empty() {
                return key.to_string();
            }
            return format!("{}:{}", prefix, key);
        }
    }

    if default_attribute_namespace_is_word(node) {
        format!("w:{}", key)
    } else {
        key.to_string()
    }
}

fn default_attribute_namespace_is_word(node: &WordNode) -> bool {
    match node.namespace.as_deref() {
        Some(W_NS) => true,
        Some(_) => false,
        None => node.element_type != WordElementType::InlineImage,
    }
}

fn attribute_sort_key(node: &WordNode, key: &str) -> String {
    qualified_attribute_name(node, key)
}

fn has_xml_space_attr(node: &WordNode) -> bool {
    node.attributes.contains_key("xml:space")
        || node
            .attribute_namespaces
            .iter()
            .any(|(key, ns)| key == "space" && ns == XML_NS)
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

// ─── validate() helpers ────────────────────────────────────────────────

/// Pull every <w:style w:styleId="..."/> ID out of word/styles.xml so we
/// can flag dangling pStyle references.
fn extract_style_ids(styles_xml: &str) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    let bytes = styles_xml.as_bytes();
    let mut cursor = 0;
    while let Some(rel) = find_byte_substring(bytes, b"<w:style ", cursor) {
        let after = &styles_xml[rel..];
        if let Some(id_attr_start) = after.find("w:styleId=\"") {
            let val_start = rel + id_attr_start + "w:styleId=\"".len();
            if let Some(end_rel) = styles_xml[val_start..].find('"') {
                ids.insert(styles_xml[val_start..val_start + end_rel].to_string());
            }
        }
        cursor = rel + 1;
    }
    ids
}

/// Find all `<w:pStyle w:val="..."/>` references in document.xml, returning
/// (style_id, byte_offset_of_pStyle_tag) tuples.
fn extract_pstyle_refs(document_xml: &str) -> Vec<(String, usize)> {
    let mut refs = Vec::new();
    let bytes = document_xml.as_bytes();
    let mut cursor = 0;
    while let Some(rel) = find_byte_substring(bytes, b"<w:pStyle ", cursor) {
        let after = &document_xml[rel..];
        if let Some(attr_start) = after.find("w:val=\"") {
            let val_start = rel + attr_start + "w:val=\"".len();
            if let Some(end_rel) = document_xml[val_start..].find('"') {
                refs.push((
                    document_xml[val_start..val_start + end_rel].to_string(),
                    rel,
                ));
            }
        }
        cursor = rel + 1;
    }
    refs
}

/// Extract every <Relationship Id="..."/> from a .rels part.
fn extract_rel_ids(rels_xml: &str) -> std::collections::HashSet<String> {
    let mut ids = std::collections::HashSet::new();
    let bytes = rels_xml.as_bytes();
    let mut cursor = 0;
    while let Some(rel) = find_byte_substring(bytes, b"Id=\"", cursor) {
        let val_start = rel + "Id=\"".len();
        if let Some(end_rel) = rels_xml[val_start..].find('"') {
            ids.insert(rels_xml[val_start..val_start + end_rel].to_string());
        }
        cursor = rel + 1;
    }
    ids
}

/// Walk document.xml and find every `r:id="..."` / `r:embed="..."` /
/// `r:link="..."` reference, returning the ones not present in `declared`.
fn extract_unresolved_rids(
    document_xml: &str,
    declared: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut unresolved = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let needles: &[&[u8]] = &[b"r:id=\"", b"r:embed=\"", b"r:link=\""];
    for needle in needles {
        let bytes = document_xml.as_bytes();
        let mut cursor = 0;
        while let Some(rel) = find_byte_substring(bytes, needle, cursor) {
            let val_start = rel + needle.len();
            if let Some(end_rel) = document_xml[val_start..].find('"') {
                let rid = &document_xml[val_start..val_start + end_rel];
                if !declared.contains(rid) && !seen.contains(rid) {
                    seen.insert(rid.to_string());
                    unresolved.push(rid.to_string());
                }
            }
            cursor = rel + 1;
        }
    }
    unresolved
}

fn find_byte_substring(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_preserves_non_wordprocessing_namespaces() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w10="urn:schemas-microsoft-com:office:word" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="w14 wp14">
  <w:body>
    <w:p>
      <w:r>
        <mc:AlternateContent>
          <mc:Choice Requires="wps">
            <w:drawing>
              <wp:anchor distT="0" distB="0">
                <wp:positionH relativeFrom="column"><wp:posOffset>5147945</wp:posOffset></wp:positionH>
                <a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
                  <a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">
                    <wps:wsp>
                      <wps:txbx>
                        <w:txbxContent>
                          <w:p>
                            <w:r><w:t xml:space="preserve"> Text </w:t></w:r>
                          </w:p>
                        </w:txbxContent>
                      </wps:txbx>
                    </wps:wsp>
                  </a:graphicData>
                </a:graphic>
              </wp:anchor>
            </w:drawing>
          </mc:Choice>
          <mc:Fallback>
            <w:pict>
              <v:shape o:allowincell="f">
                <v:textbox><w:txbxContent><w:p><w:r><w:t>Fallback</w:t></w:r></w:p></w:txbxContent></v:textbox>
              </v:shape>
            </w:pict>
          </mc:Fallback>
        </mc:AlternateContent>
      </w:r>
    </w:p>
  </w:body>
</w:document>"#;

        let dom = parse_document_xml(xml).unwrap();
        let serialized = serialize_dom(&dom);

        assert!(serialized.contains("<mc:AlternateContent"));
        assert!(serialized.contains("<mc:Choice"));
        assert!(serialized.contains("Requires=\"wps\""));
        assert!(serialized.contains("<wp:anchor"));
        assert!(serialized.contains("<wp:posOffset>5147945</wp:posOffset>"));
        assert!(serialized.contains("distB=\"0\""));
        assert!(serialized.contains("distT=\"0\""));
        assert!(serialized.contains("<a:graphic"));
        assert!(serialized.contains("<a:graphicData"));
        assert!(serialized
            .contains("uri=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\""));
        assert!(serialized.contains("<wps:wsp"));
        assert!(serialized.contains("<wps:txbx"));
        assert!(serialized.contains("<v:shape"));
        assert!(serialized.contains("o:allowincell=\"f\""));
        assert!(serialized.contains("mc:Ignorable=\"w14 wp14\""));
        assert!(serialized.contains("<w:t"));
        assert!(serialized.contains("xml:space=\"preserve\""));
        assert!(serialized.contains("> Text </w:t>"));

        assert!(!serialized.contains("<w:AlternateContent"));
        assert!(!serialized.contains("<w:anchor"));
        assert!(!serialized.contains("<w:graphic"));
        assert!(!serialized.contains("<w:wsp"));
        assert!(!serialized.contains("<w:shape"));
        assert!(!serialized.contains("w:space=\"preserve\""));
    }

    #[test]
    fn generated_word_nodes_serialize_local_attributes_in_word_namespace() {
        let dom = WordDom::new(WordNode::new(WordElementType::Document).with_children(vec![
            WordNode::new(WordElementType::Body).with_children(vec![
                WordNode::new(WordElementType::Paragraph).with_children(vec![
                    WordNode::new(WordElementType::ParagraphProperties).with_children(vec![
                        WordNode::new(WordElementType::Unknown("pStyle".to_string()))
                            .with_attribute("val", "Heading1"),
                    ]),
                ]),
            ]),
        ]));

        let serialized = serialize_dom(&dom);

        assert!(serialized.contains("<w:pStyle w:val=\"Heading1\" />"));
    }

    #[test]
    fn range_text_replacement_emits_text_once_across_segments() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>A</w:t></w:r>
      <w:r><w:t>B</w:t></w:r>
      <w:r><w:t>C</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;
        let mut dom = parse_document_xml(xml).unwrap();
        let segments =
            handler_common::parse_range_paths("/body/p[1]/r[1],/body/p[1]/r[2],/body/p[1]/r[3]")
                .unwrap();
        let mut properties = HashMap::new();
        properties.insert("range_paths".to_string(), "unused".to_string());
        properties.insert("text".to_string(), "[金额]".to_string());

        apply_docx_range_highlights(&mut dom, &properties, &segments).unwrap();

        let paragraph = navigate_to_element(&dom, "/body/p[1]").unwrap();
        assert_eq!(paragraph.paragraph_text(), "[金额]");
    }

    #[test]
    fn range_text_replacement_does_not_merge_across_paragraphs() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>A</w:t></w:r></w:p>
    <w:p><w:r><w:t>B</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let mut dom = parse_document_xml(xml).unwrap();
        let segments =
            handler_common::parse_range_paths("/body/p[1]/r[1],/body/p[2]/r[1]").unwrap();
        let mut properties = HashMap::new();
        properties.insert("range_paths".to_string(), "unused".to_string());
        properties.insert("text".to_string(), "[金额]".to_string());

        apply_docx_range_highlights(&mut dom, &properties, &segments).unwrap();

        assert_eq!(
            navigate_to_element(&dom, "/body/p[1]")
                .unwrap()
                .paragraph_text(),
            "[金额]"
        );
        assert_eq!(
            navigate_to_element(&dom, "/body/p[2]")
                .unwrap()
                .paragraph_text(),
            "[金额]"
        );
    }

    #[test]
    fn range_text_replacement_merges_textbox_runs_in_same_paragraph() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r>
        <w:drawing>
          <w:txbxContent>
            <w:p>
              <w:r><w:t>A</w:t></w:r>
              <w:r><w:t>B</w:t></w:r>
              <w:r><w:t>C</w:t></w:r>
            </w:p>
          </w:txbxContent>
        </w:drawing>
      </w:r>
    </w:p>
  </w:body>
</w:document>"#;
        let mut dom = parse_document_xml(xml).unwrap();
        let segments = handler_common::parse_range_paths(
            "/body/p[1]/r[1]/drawing[1]/txbxContent[1]/p[1]/r[1],\
             /body/p[1]/r[1]/drawing[1]/txbxContent[1]/p[1]/r[2],\
             /body/p[1]/r[1]/drawing[1]/txbxContent[1]/p[1]/r[3]",
        )
        .unwrap();
        let mut properties = HashMap::new();
        properties.insert("range_paths".to_string(), "unused".to_string());
        properties.insert("text".to_string(), "[金额]".to_string());

        apply_docx_range_highlights(&mut dom, &properties, &segments).unwrap();

        let textbox_para =
            navigate_to_element(&dom, "/body/p[1]/r[1]/drawing[1]/txbxContent[1]/p[1]").unwrap();
        assert_eq!(textbox_para.paragraph_text(), "[金额]");
    }
}
