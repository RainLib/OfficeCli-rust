use thiserror::Error;

#[derive(Debug, Error)]
pub enum XmlUtilError {
    #[error("XML read error: {0}")]
    ReadError(String),
    #[error("XML write error: {0}")]
    WriteError(String),
    #[error("XPath not found: {0}")]
    XPathNotFound(String),
    #[error("invalid XPath: {0}")]
    InvalidXPath(String),
}

/// Strip XML prolog (<?xml ...?>) from a string if present.
pub fn strip_prolog(xml: &str) -> &str {
    if xml.starts_with("<?xml") {
        if let Some(end) = xml.find("?>") {
            xml[end + 2..].trim_start()
        } else {
            xml
        }
    } else {
        xml
    }
}

/// Parse XML and find elements matching an XPath-like expression.
/// Supports simple XPath: /root/child[N], /root/child[@attr=val]
pub fn find_elements_by_xpath(xml: &str, xpath: &str) -> Result<Vec<String>, XmlUtilError> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|e| XmlUtilError::ReadError(e.to_string()))?;

    let segments = parse_xpath_segments(xpath)?;
    let matched = walk_xpath(&doc, &segments)?;

    Ok(matched
        .iter()
        // Preserve the source fragment rather than reserializing the XML
        // node: OOXML fragments commonly rely on namespace prefixes declared
        // by an ancestor, and callers can safely splice these back into that
        // original document.
        .map(|node| xml[node.range()].to_string())
        .collect())
}

/// XPath segment: name + optional index + optional attribute filter.
#[derive(Clone)]
struct XPathSegment {
    name: String,
    index: Option<usize>,
    attr_filter: Option<(String, String)>,
}

fn parse_xpath_segments(xpath: &str) -> Result<Vec<XPathSegment>, XmlUtilError> {
    if !xpath.starts_with('/') {
        return Err(XmlUtilError::InvalidXPath(xpath.to_string()));
    }

    let mut segments = Vec::new();
    let parts = xpath.split('/').filter(|s| !s.is_empty());

    for part in parts {
        let original = part.to_string();
        let name;
        let mut index = None;
        let mut attr_filter = None;

        if let Some(bracket_start) = original.find('[') {
            name = original[..bracket_start].to_string();
            // Walk every [...] group inside this segment so /root/child[2][@id=x]
            // works alongside the single-bracket forms.
            let tail = &original[bracket_start..];
            let mut cursor = 0;
            while let Some(open) = tail[cursor..].find('[') {
                let abs_open = cursor + open;
                let abs_close = match tail[abs_open + 1..].find(']') {
                    Some(c) => abs_open + 1 + c,
                    None => break,
                };
                let content = &tail[abs_open + 1..abs_close];
                if let Some(attr_content) = content.strip_prefix('@') {
                    if let Some(eq) = attr_content.find('=') {
                        attr_filter = Some((
                            attr_content[..eq].to_string(),
                            attr_content[eq + 1..]
                                .trim_matches(|c| c == '\'' || c == '"')
                                .to_string(),
                        ));
                    }
                } else if let Ok(idx) = content.parse::<usize>() {
                    index = Some(idx);
                }
                cursor = abs_close + 1;
            }
        } else {
            name = original;
        }

        segments.push(XPathSegment {
            name,
            index,
            attr_filter,
        });
    }

    Ok(segments)
}

/// Walk the doc tree following `segments`. Returns matched nodes honoring
/// positional index `[N]` (1-based) and attribute filter `[@k=v]`.
fn walk_xpath<'input>(
    doc: &'input roxmltree::Document<'input>,
    segments: &[XPathSegment],
) -> Result<Vec<roxmltree::Node<'input, 'input>>, XmlUtilError> {
    let mut current_nodes: Vec<roxmltree::Node> = vec![doc.root_element()];

    // When the user supplies a single-segment xpath starting with `/`, they
    // usually mean "starting from the document root". So for the first
    // segment, also accept a match against the root element itself.
    let mut first = true;
    for segment in segments {
        let mut next_nodes = Vec::new();
        for node in &current_nodes {
            for child in node.children() {
                if child.is_element() && matches_segment(child, segment) {
                    next_nodes.push(child);
                }
            }
        }
        if first && next_nodes.is_empty() {
            // Treat single-segment absolute queries like "/<rootname>" as
            // matching the root element itself.
            let root = doc.root_element();
            if root.is_element() && matches_segment(root, segment) {
                next_nodes.push(root);
            }
        }
        current_nodes = apply_index(next_nodes, segment);
        first = false;
    }

    if current_nodes.is_empty() {
        return Err(XmlUtilError::XPathNotFound(segments_to_string(segments)));
    }
    Ok(current_nodes)
}

fn apply_index<'input>(
    nodes: Vec<roxmltree::Node<'input, 'input>>,
    segment: &XPathSegment,
) -> Vec<roxmltree::Node<'input, 'input>> {
    match segment.index {
        // XPath indices are 1-based; [0] is invalid but treat defensively as no-op.
        Some(0) => Vec::new(),
        Some(n) => nodes.into_iter().nth(n - 1).into_iter().collect(),
        None => nodes,
    }
}

fn segments_to_string(segments: &[XPathSegment]) -> String {
    let mut s = String::new();
    for seg in segments {
        s.push('/');
        s.push_str(&seg.name);
        if let Some(n) = seg.index {
            s.push_str(&format!("[{}]", n));
        }
        if let Some((k, v)) = &seg.attr_filter {
            s.push_str(&format!("[@{}={}]", k, v));
        }
    }
    s
}

fn matches_segment(node: roxmltree::Node, segment: &XPathSegment) -> bool {
    // XPath segments come in as written (e.g. "w:body" or "body"); match
    // either the full segment name or its local-part after ':'.
    let seg_local = segment
        .name
        .rsplit(':')
        .next()
        .unwrap_or(segment.name.as_str());
    let node_local = node.tag_name().name();
    if node_local != seg_local {
        return false;
    }

    if let Some((attr_key, attr_val)) = &segment.attr_filter {
        if let Some(attr) = node.attribute(attr_key.as_str()) {
            if attr != attr_val.as_str() {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

/// Apply an XPath action to XML.
///
/// Actions:
/// - `setattr` — set `attr=value` on the matched element (`new_xml = "attr=value"`).
/// - `remove` — drop every matched element from the document.
/// - `replace` — replace each matched element with `new_xml`.
/// - `insert` — insert `new_xml` as the previous sibling of each matched element.
/// - `append` — append `new_xml` as the last child of each matched element.
/// - `prepend` — insert `new_xml` as the first child of each matched element.
pub fn apply_xpath_action(
    xml: &str,
    xpath: &str,
    action: &str,
    new_xml: Option<&str>,
) -> Result<String, XmlUtilError> {
    match action {
        "setattr" => {
            let new = new_xml.ok_or_else(|| {
                XmlUtilError::WriteError("setattr requires attr=value".to_string())
            })?;
            let (attr_name, attr_val) = new.split_once('=').ok_or_else(|| {
                XmlUtilError::WriteError("setattr format: attr=value".to_string())
            })?;
            set_attribute_in_xml(xml, xpath, attr_name.trim(), attr_val.trim())
        }
        "remove" => remove_element_by_xpath(xml, xpath),
        "replace" => {
            let new = require_new_xml(action, new_xml)?;
            replace_element_by_xpath(xml, xpath, new)
        }
        "insert" => {
            let new = require_new_xml(action, new_xml)?;
            insert_before_by_xpath(xml, xpath, new)
        }
        "append" => {
            let new = require_new_xml(action, new_xml)?;
            append_child_by_xpath(xml, xpath, new)
        }
        "prepend" => {
            let new = require_new_xml(action, new_xml)?;
            prepend_child_by_xpath(xml, xpath, new)
        }
        _ => Err(XmlUtilError::WriteError(format!(
            "unsupported action: {}",
            action
        ))),
    }
}

fn require_new_xml<'a>(action: &str, new_xml: Option<&'a str>) -> Result<&'a str, XmlUtilError> {
    new_xml.ok_or_else(|| XmlUtilError::WriteError(format!("action `{}` requires --xml", action)))
}

/// Locate the byte ranges of all elements matched by `xpath`, sorted in
/// descending order so callers can splice the original string without
/// invalidating earlier offsets.
fn locate_matches(xml: &str, xpath: &str) -> Result<Vec<(usize, usize)>, XmlUtilError> {
    let doc =
        roxmltree::Document::parse(xml).map_err(|e| XmlUtilError::ReadError(e.to_string()))?;
    let segments = parse_xpath_segments(xpath)?;
    let matched = walk_xpath(&doc, &segments)?;

    let mut ranges: Vec<(usize, usize)> = matched
        .iter()
        .map(|n| {
            let r = n.range();
            (r.start, r.end)
        })
        .collect();
    ranges.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    Ok(ranges)
}

fn set_attribute_in_xml(
    xml: &str,
    xpath: &str,
    attr_name: &str,
    attr_val: &str,
) -> Result<String, XmlUtilError> {
    let ranges = locate_matches(xml, xpath)?;
    if ranges.is_empty() {
        return Err(XmlUtilError::XPathNotFound(xpath.to_string()));
    }

    let mut out = xml.to_string();
    for (start, end) in ranges {
        let elem_text = &out[start..end];
        let new_elem = set_attr_in_element(elem_text, attr_name, attr_val)?;
        out.replace_range(start..end, &new_elem);
    }
    Ok(out)
}

/// Set `attr_name=attr_val` on the element occupying `elem_text`.
/// If the attribute already exists, replace its value; otherwise insert a new
/// one immediately before the closing `>` of the opening tag.
fn set_attr_in_element(
    elem_text: &str,
    attr_name: &str,
    attr_val: &str,
) -> Result<String, XmlUtilError> {
    // Find the end of the opening tag.
    let open_tag_end = match elem_text.find('>') {
        Some(p) => p,
        None => return Err(XmlUtilError::WriteError("malformed element".into())),
    };
    let self_closing = open_tag_end > 0 && elem_text.as_bytes()[open_tag_end - 1] == b'/';
    let attr_marker = format!(" {}=\"{}\"", attr_name, escape_xml(attr_val));

    // Search within the opening tag for an existing occurrence of attr_name.
    let open_tag = &elem_text[..open_tag_end];
    let pat = format!("{}=\"", attr_name);
    if let Some(rel) = open_tag.find(&pat) {
        let val_start = rel + pat.len();
        let val_end_rel = match open_tag[val_start..].find('"') {
            Some(p) => val_start + p,
            None => return Err(XmlUtilError::WriteError("malformed attribute".into())),
        };
        let mut new_elem = String::with_capacity(elem_text.len() + attr_marker.len());
        new_elem.push_str(&elem_text[..val_start]);
        new_elem.push_str(&escape_xml(attr_val));
        new_elem.push_str(&elem_text[val_end_rel..]);
        return Ok(new_elem);
    }

    // Not present — insert before the closing `>` (or before the `/` if self-closing).
    let insert_pos = if self_closing {
        open_tag_end - 1
    } else {
        open_tag_end
    };
    let mut new_elem = String::with_capacity(elem_text.len() + attr_marker.len());
    new_elem.push_str(&elem_text[..insert_pos]);
    new_elem.push_str(&attr_marker);
    new_elem.push_str(&elem_text[insert_pos..]);
    Ok(new_elem)
}

fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

fn remove_element_by_xpath(xml: &str, xpath: &str) -> Result<String, XmlUtilError> {
    let ranges = locate_matches(xml, xpath)?;
    if ranges.is_empty() {
        return Err(XmlUtilError::XPathNotFound(xpath.to_string()));
    }
    let mut out = xml.to_string();
    for (start, end) in ranges {
        out.replace_range(start..end, "");
    }
    Ok(out)
}

fn replace_element_by_xpath(xml: &str, xpath: &str, new_xml: &str) -> Result<String, XmlUtilError> {
    let ranges = locate_matches(xml, xpath)?;
    if ranges.is_empty() {
        return Err(XmlUtilError::XPathNotFound(xpath.to_string()));
    }
    let sanitized = sanitize_fragment(new_xml)?;
    let mut out = xml.to_string();
    for (start, end) in ranges {
        out.replace_range(start..end, &sanitized);
    }
    Ok(out)
}

fn insert_before_by_xpath(xml: &str, xpath: &str, new_xml: &str) -> Result<String, XmlUtilError> {
    let ranges = locate_matches(xml, xpath)?;
    if ranges.is_empty() {
        return Err(XmlUtilError::XPathNotFound(xpath.to_string()));
    }
    let sanitized = sanitize_fragment(new_xml)?;
    let mut out = xml.to_string();
    for (start, _) in ranges {
        out.insert_str(start, &sanitized);
    }
    Ok(out)
}

fn append_child_by_xpath(xml: &str, xpath: &str, new_xml: &str) -> Result<String, XmlUtilError> {
    splice_into_children(xml, xpath, new_xml, true)
}

fn prepend_child_by_xpath(xml: &str, xpath: &str, new_xml: &str) -> Result<String, XmlUtilError> {
    splice_into_children(xml, xpath, new_xml, false)
}

/// Insert `new_xml` inside each matched element. `append=true` → before
/// the closing tag; `append=false` → right after the opening tag.
fn splice_into_children(
    xml: &str,
    xpath: &str,
    new_xml: &str,
    append: bool,
) -> Result<String, XmlUtilError> {
    let ranges = locate_matches(xml, xpath)?;
    if ranges.is_empty() {
        return Err(XmlUtilError::XPathNotFound(xpath.to_string()));
    }
    let sanitized = sanitize_fragment(new_xml)?;
    let mut out = xml.to_string();

    for (start, end) in ranges {
        let elem_text = &out[start..end];
        let open_tag_end = elem_text
            .find('>')
            .ok_or_else(|| XmlUtilError::WriteError("malformed element".into()))?;
        let self_closing = open_tag_end > 0 && elem_text.as_bytes()[open_tag_end - 1] == b'/';

        if self_closing {
            // Convert <tag .../> → <tag ...>new_xml</tag>
            let before_self_close = &elem_text[..open_tag_end - 1];
            let tag_name = extract_tag_name(elem_text)?;
            let replacement = format!(
                "{}>{}{}</{}>",
                before_self_close, sanitized, tag_name, tag_name
            );
            out.replace_range(start..end, &replacement);
            continue;
        }

        let abs_insert = if append {
            // Find the closing tag of THIS element (last </tagname> before end).
            let tag_name = extract_tag_name(elem_text)?;
            let close_marker = format!("</{}>", tag_name);
            let close_rel = elem_text
                .rfind(&close_marker)
                .ok_or_else(|| XmlUtilError::WriteError("missing close tag".into()))?;
            start + close_rel
        } else {
            start + open_tag_end + 1
        };
        out.insert_str(abs_insert, &sanitized);
    }
    Ok(out)
}

/// Extract the full tag name (including any namespace prefix) of the
/// opening tag, e.g. "w:p" from `<w:p ...>`. Used when we need to
/// synthesize a closing tag for a self-closing source element.
fn extract_tag_name(elem_text: &str) -> Result<String, XmlUtilError> {
    let start = elem_text
        .find('<')
        .ok_or_else(|| XmlUtilError::WriteError("missing <".into()))?
        + 1;
    let rest = &elem_text[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(rest.len());
    Ok(rest[..end].trim().to_string())
}

/// Normalize a user-supplied XML fragment. We deliberately do NOT run a
/// strict well-formedness check here — OOXML fragments routinely use namespace
/// prefixes (`w:`, `a:`, `r:`) declared on ancestor elements, so parsing them
/// in isolation would reject perfectly valid input. We just trim and verify
/// the result isn't empty.
fn sanitize_fragment(new_xml: &str) -> Result<String, XmlUtilError> {
    let trimmed = new_xml.trim();
    if trimmed.is_empty() {
        return Err(XmlUtilError::WriteError(
            "action requires non-empty --xml".into(),
        ));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        "<?xml version=\"1.0\"?>\n<root>\n  <item id=\"a\">one</item>\n  <item id=\"b\">two</item>\n  <wrap><leaf/></wrap>\n</root>\n"
    }

    #[test]
    fn find_with_index_and_attr() {
        let hits = find_elements_by_xpath(sample(), "/root/item").unwrap();
        assert_eq!(hits.len(), 2);

        let by_attr = find_elements_by_xpath(sample(), "/root/item[@id=b]").unwrap();
        assert_eq!(by_attr.len(), 1);
        assert!(by_attr[0].contains("two"));

        let by_index = find_elements_by_xpath(sample(), "/root/item[1]").unwrap();
        assert_eq!(by_index.len(), 1);
        assert!(by_index[0].contains("one"));
    }

    #[test]
    fn find_preserves_source_namespace_prefixes() {
        let xml = r#"<w:document xmlns:w="urn:word"><w:body><w:p>text</w:p></w:body></w:document>"#;
        let hits = find_elements_by_xpath(xml, "/w:document/w:body").unwrap();
        assert_eq!(hits, vec!["<w:body><w:p>text</w:p></w:body>"]);
    }

    #[test]
    fn setattr_inserts_and_replaces() {
        let fresh =
            apply_xpath_action(sample(), "/root/item[@id=a]", "setattr", Some("role=first"))
                .unwrap();
        assert!(fresh.contains("role=\"first\""));

        let updated =
            apply_xpath_action(&fresh, "/root/item[@id=a]", "setattr", Some("role=second"))
                .unwrap();
        assert!(updated.contains("role=\"second\""));
        assert!(!updated.contains("role=\"first\""));
    }

    #[test]
    fn remove_action_drops_only_matched() {
        let out = apply_xpath_action(sample(), "/root/item[@id=a]", "remove", None).unwrap();
        assert!(!out.contains("one"));
        assert!(out.contains("two"));
    }

    #[test]
    fn replace_swaps_element() {
        let out = apply_xpath_action(
            sample(),
            "/root/item[@id=b]",
            "replace",
            Some("<item id=\"b\">REPLACED</item>"),
        )
        .unwrap();
        assert!(out.contains("REPLACED"));
        assert!(!out.contains("two"));
        assert!(out.contains("one"));
    }

    #[test]
    fn insert_before_adds_sibling() {
        let out = apply_xpath_action(
            sample(),
            "/root/item[@id=b]",
            "insert",
            Some("<item id=\"mid\">MID</item>"),
        )
        .unwrap();
        let mid_pos = out.find("MID").unwrap();
        let b_pos = out.find("two").unwrap();
        assert!(mid_pos < b_pos);
        let a_pos = out.find("one").unwrap();
        assert!(a_pos < mid_pos);
    }

    #[test]
    fn append_adds_last_child() {
        let out =
            apply_xpath_action(sample(), "/root/wrap", "append", Some("<extra>X</extra>")).unwrap();
        // wrap should now contain both <leaf/> and <extra>
        let wrap_start = out.find("<wrap>").unwrap();
        let wrap_end = out.find("</wrap>").unwrap();
        let slice = &out[wrap_start..wrap_end];
        let leaf_pos = slice.find("<leaf/>").unwrap();
        let extra_pos = slice.find("<extra>").unwrap();
        assert!(leaf_pos < extra_pos);
    }

    #[test]
    fn prepend_adds_first_child() {
        let out = apply_xpath_action(sample(), "/root/wrap", "prepend", Some("<extra>X</extra>"))
            .unwrap();
        let wrap_start = out.find("<wrap>").unwrap();
        let wrap_end = out.find("</wrap>").unwrap();
        let slice = &out[wrap_start..wrap_end];
        let leaf_pos = slice.find("<leaf/>").unwrap();
        let extra_pos = slice.find("<extra>").unwrap();
        assert!(extra_pos < leaf_pos);
    }

    #[test]
    fn append_expands_self_closing() {
        let out = apply_xpath_action(
            sample(),
            "/root/wrap/leaf",
            "append",
            Some("<deep>D</deep>"),
        )
        .unwrap();
        assert!(out.contains("<deep>D</deep>"));
        assert!(out.contains("</leaf>"));
    }

    #[test]
    fn unsupported_action_errors() {
        let err = apply_xpath_action(sample(), "/root", "bogus", None).unwrap_err();
        assert!(matches!(err, XmlUtilError::WriteError(_)));
    }

    #[test]
    fn missing_xml_errors() {
        let err = apply_xpath_action(sample(), "/root/item[@id=a]", "replace", None).unwrap_err();
        assert!(matches!(err, XmlUtilError::WriteError(_)));
    }

    #[test]
    fn unknown_xpath_errors() {
        let err = apply_xpath_action(sample(), "/root/nope", "remove", None).unwrap_err();
        assert!(matches!(err, XmlUtilError::XPathNotFound(_)));
    }
}
