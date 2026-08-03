use crate::dom_types::{NS_A, NS_P};
use handler_common::{DocumentNode, HandlerError, InsertPosition};
use oxml::OxmlPackage;

#[derive(Clone)]
struct LineBreakTarget {
    slide: usize,
    groups: Vec<usize>,
    selector: ShapeSelector,
    paragraph: Option<usize>,
}

#[derive(Clone)]
enum ShapeSelector {
    Ordinal(usize),
    Placeholder(String),
}

pub fn add_linebreak(
    package: &mut OxmlPackage,
    parent: &str,
    position: InsertPosition,
) -> Result<String, HandlerError> {
    let target = parse_target(parent)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, target.slide)?;
    let xml = package
        .read_part_xml(&slide_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let doc = parse_xml(&xml)?;
    let paragraph = resolve_paragraph(&doc, &target)?;
    let paragraph_index = paragraph.index;
    let insertion = match position {
        InsertPosition::AtIndex(index) => paragraph_child_insertion_offset(&paragraph.node, index)?,
        _ => paragraph_end_para_rpr_offset(&paragraph.node)
            .unwrap_or_else(|| paragraph.node.range().end - "</a:p>".len()),
    };
    let break_index = paragraph
        .node
        .children()
        .filter(|child| child.has_tag_name((NS_A, "br")) || child.has_tag_name("br"))
        .count()
        + 1;
    drop(doc);
    let mut updated = xml;
    updated.insert_str(insertion, "<a:br/>");
    package
        .write_part_xml(&slide_path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(format!(
        "{}/paragraph[{}]/br[{}]",
        target.path_head(),
        paragraph_index,
        break_index
    ))
}

pub fn get_linebreak(package: &OxmlPackage, path: &str) -> Result<DocumentNode, HandlerError> {
    let (target, break_index) = parse_break_path(path)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, target.slide)?;
    let xml = package
        .read_part_xml(&slide_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let doc = parse_xml(&xml)?;
    let paragraph = resolve_paragraph(&doc, &target)?;
    let break_count = paragraph
        .node
        .children()
        .filter(|child| child.has_tag_name((NS_A, "br")) || child.has_tag_name("br"))
        .count();
    if break_index == 0 || break_index > break_count {
        return Err(HandlerError::PathNotFound(path.to_string()));
    }
    Ok(DocumentNode::new(path, "linebreak"))
}

pub fn remove_linebreak(
    package: &mut OxmlPackage,
    path: &str,
) -> Result<Option<String>, HandlerError> {
    let (target, break_index) = parse_break_path(path)?;
    let slide_path = crate::navigation::resolve_slide_part_path(package, target.slide)?;
    let xml = package
        .read_part_xml(&slide_path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let doc = parse_xml(&xml)?;
    let paragraph = resolve_paragraph(&doc, &target)?;
    let break_node = paragraph
        .node
        .children()
        .filter(|child| child.has_tag_name((NS_A, "br")) || child.has_tag_name("br"))
        .nth(break_index.saturating_sub(1))
        .ok_or_else(|| HandlerError::PathNotFound(path.to_string()))?;
    let range = break_node.range();
    drop(doc);
    let mut updated = xml;
    updated.replace_range(range, "");
    package
        .write_part_xml(&slide_path, &updated)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(Some(path.to_string()))
}

struct ResolvedParagraph<'a> {
    index: usize,
    node: roxmltree::Node<'a, 'a>,
}

fn parse_xml(xml: &str) -> Result<roxmltree::Document<'_>, HandlerError> {
    roxmltree::Document::parse(xml)
        .map_err(|error| HandlerError::OperationFailed(format!("invalid slide XML: {}", error)))
}

fn parse_target(path: &str) -> Result<LineBreakTarget, HandlerError> {
    let components: Vec<_> = path
        .strip_prefix('/')
        .unwrap_or(path)
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    let (name, value) = components
        .first()
        .and_then(|component| parse_component(component))
        .ok_or_else(|| invalid_target(path))?;
    if name != "slide" {
        return Err(invalid_target(path));
    }
    let slide = parse_positive(value, path)?;
    let mut cursor = 1;
    let mut groups = Vec::new();
    while let Some(("group", value)) = components
        .get(cursor)
        .and_then(|component| parse_component(component))
    {
        groups.push(parse_positive(value, path)?);
        cursor += 1;
    }
    let (name, value) = components
        .get(cursor)
        .and_then(|component| parse_component(component))
        .ok_or_else(|| invalid_target(path))?;
    let selector = match name {
        "shape" => ShapeSelector::Ordinal(parse_positive(value, path)?),
        "placeholder"
            if !value.is_empty()
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_') =>
        {
            if !groups.is_empty() {
                return Err(HandlerError::InvalidPath(format!(
                    "placeholder linebreak targets cannot have /group[N] ancestors: {}",
                    path
                )));
            }
            ShapeSelector::Placeholder(value.to_string())
        }
        _ => return Err(invalid_target(path)),
    };
    cursor += 1;
    let paragraph = match components.get(cursor) {
        None => None,
        Some(component) => {
            let (name, value) = parse_component(component).ok_or_else(|| invalid_target(path))?;
            if !matches!(name, "paragraph" | "p") || cursor + 1 != components.len() {
                return Err(invalid_target(path));
            }
            Some(parse_positive(value, path)?)
        }
    };
    Ok(LineBreakTarget {
        slide,
        groups,
        selector,
        paragraph,
    })
}

fn invalid_target(path: &str) -> HandlerError {
    HandlerError::InvalidPath(format!(
        "linebreak parent must be /slide[N]/shape[M], /slide[N]/group[G]/.../shape[M], or /slide[N]/placeholder[X], optionally followed by /paragraph[K]: {}",
        path
    ))
}

fn parse_component(component: &str) -> Option<(&str, &str)> {
    let (name, value) = component.split_once('[')?;
    Some((name, value.strip_suffix(']')?))
}

fn parse_positive(value: &str, path: &str) -> Result<usize, HandlerError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|index| *index > 0)
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))
}

fn parse_break_path(path: &str) -> Result<(LineBreakTarget, usize), HandlerError> {
    let (parent, last) = path
        .rsplit_once('/')
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    let index = last
        .strip_prefix("br[")
        .or_else(|| last.strip_prefix("linebreak["))
        .and_then(|value| value.strip_suffix(']'))
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| *index > 0)
        .ok_or_else(|| HandlerError::InvalidPath(path.to_string()))?;
    Ok((parse_target(parent)?, index))
}

fn resolve_paragraph<'a>(
    doc: &'a roxmltree::Document<'a>,
    target: &LineBreakTarget,
) -> Result<ResolvedParagraph<'a>, HandlerError> {
    let mut scope = doc
        .descendants()
        .find(|node| node.has_tag_name((NS_P, "spTree")) || node.has_tag_name("spTree"))
        .ok_or_else(|| HandlerError::PathNotFound("slide has no shape tree".to_string()))?;
    for group_index in &target.groups {
        scope = scope
            .children()
            .filter(|node| node.has_tag_name((NS_P, "grpSp")) || node.has_tag_name("grpSp"))
            .nth(*group_index - 1)
            .ok_or_else(|| HandlerError::PathNotFound(format!("group {}", group_index)))?;
    }
    let shapes: Vec<_> = scope
        .children()
        .filter(|node| node.has_tag_name((NS_P, "sp")) || node.has_tag_name("sp"))
        .collect();
    let shape = match &target.selector {
        ShapeSelector::Ordinal(index) => shapes
            .get(index - 1)
            .copied()
            .ok_or_else(|| HandlerError::PathNotFound(format!("shape {}", index)))?,
        ShapeSelector::Placeholder(selector) => resolve_placeholder(&shapes, selector)?,
    };
    let text_body = shape
        .children()
        .find(|node| node.has_tag_name((NS_P, "txBody")) || node.has_tag_name("txBody"))
        .ok_or_else(|| HandlerError::PathNotFound("shape has no text body".to_string()))?;
    let paragraphs: Vec<_> = text_body
        .children()
        .filter(|node| node.has_tag_name((NS_A, "p")) || node.has_tag_name("p"))
        .collect();
    let index = target.paragraph.unwrap_or(paragraphs.len());
    let node = paragraphs
        .get(index.saturating_sub(1))
        .copied()
        .ok_or_else(|| HandlerError::PathNotFound(format!("paragraph {}", index)))?;
    Ok(ResolvedParagraph { index, node })
}

impl LineBreakTarget {
    fn path_head(&self) -> String {
        let mut path = format!("/slide[{}]", self.slide);
        for group in &self.groups {
            path.push_str(&format!("/group[{}]", group));
        }
        match &self.selector {
            ShapeSelector::Ordinal(index) => path.push_str(&format!("/shape[{}]", index)),
            ShapeSelector::Placeholder(selector) => {
                path.push_str(&format!("/placeholder[{}]", selector));
            }
        }
        path
    }
}

fn resolve_placeholder<'a>(
    shapes: &[roxmltree::Node<'a, 'a>],
    selector: &str,
) -> Result<roxmltree::Node<'a, 'a>, HandlerError> {
    if let Ok(index) = selector.parse::<usize>() {
        if index == 0 {
            return Err(HandlerError::PathNotFound(format!(
                "placeholder {}",
                selector
            )));
        }
        return shapes
            .iter()
            .copied()
            .filter(has_placeholder)
            .nth(index - 1)
            .ok_or_else(|| HandlerError::PathNotFound(format!("placeholder {}", selector)));
    }
    let expected = canonical_placeholder_type(selector).ok_or_else(|| {
        HandlerError::InvalidPath(format!("unknown placeholder type: {}", selector))
    })?;
    shapes
        .iter()
        .copied()
        .find(|shape| placeholder_type(*shape).as_deref() == Some(expected))
        .ok_or_else(|| HandlerError::PathNotFound(format!("placeholder {}", selector)))
}

fn has_placeholder(shape: &roxmltree::Node<'_, '_>) -> bool {
    placeholder_type(*shape).is_some()
}

fn placeholder_type(shape: roxmltree::Node<'_, '_>) -> Option<String> {
    shape
        .descendants()
        .find(|node| node.has_tag_name((NS_P, "ph")) || node.has_tag_name("ph"))
        .map(|placeholder| {
            placeholder
                .attribute("type")
                .unwrap_or("obj")
                .to_ascii_lowercase()
        })
}

fn canonical_placeholder_type(selector: &str) -> Option<&'static str> {
    match selector.to_ascii_lowercase().as_str() {
        "title" => Some("title"),
        "centertitle" | "centeredtitle" | "ctitle" | "ctrtitle" => Some("ctrtitle"),
        "body" | "content" => Some("body"),
        "subtitle" | "sub" | "subtitlepres" => Some("subtitle"),
        "date" | "datetime" | "dt" => Some("dt"),
        "footer" => Some("ftr"),
        "slidenum" | "slidenumber" | "sldnum" => Some("sldnum"),
        "object" | "obj" => Some("obj"),
        "chart" => Some("chart"),
        "table" => Some("tbl"),
        "clipart" => Some("clipart"),
        "diagram" | "dgm" => Some("dgm"),
        "media" => Some("media"),
        "picture" | "pic" => Some("pic"),
        "header" => Some("hdr"),
        _ => None,
    }
}

fn paragraph_end_para_rpr_offset(paragraph: &roxmltree::Node<'_, '_>) -> Option<usize> {
    paragraph
        .children()
        .find(|child| child.has_tag_name((NS_A, "endParaRPr")) || child.has_tag_name("endParaRPr"))
        .map(|node| node.range().start)
}

fn paragraph_child_insertion_offset(
    paragraph: &roxmltree::Node<'_, '_>,
    index: usize,
) -> Result<usize, HandlerError> {
    let children: Vec<_> = paragraph
        .children()
        .filter(|child| child.is_element())
        .collect();
    if index >= children.len() {
        return Ok(paragraph_end_para_rpr_offset(paragraph)
            .unwrap_or_else(|| paragraph.range().end - "</a:p>".len()));
    }
    Ok(children[index].range().start)
}

#[cfg(test)]
mod tests {
    use super::{parse_target, resolve_paragraph, ShapeSelector};

    const SLIDE_XML: &str = r#"
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree>
    <p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p/></p:txBody></p:sp>
    <p:sp><p:nvSpPr><p:nvPr><p:ph type="body"/></p:nvPr></p:nvSpPr><p:txBody><a:p/></p:txBody></p:sp>
    <p:grpSp><p:grpSp><p:sp><p:txBody><a:p/></p:txBody></p:sp></p:grpSp></p:grpSp>
  </p:spTree></p:cSld>
</p:sld>"#;

    #[test]
    fn parses_placeholder_and_nested_group_targets() {
        let placeholder = parse_target("/slide[1]/placeholder[title]/paragraph[1]").unwrap();
        assert!(
            matches!(placeholder.selector, ShapeSelector::Placeholder(ref value) if value == "title")
        );

        let grouped = parse_target("/slide[1]/group[1]/group[1]/shape[1]").unwrap();
        assert_eq!(grouped.groups, vec![1, 1]);
        assert!(matches!(grouped.selector, ShapeSelector::Ordinal(1)));
        assert!(parse_target("/slide[1]/group[1]/placeholder[title]").is_err());
    }

    #[test]
    fn resolves_placeholder_by_type_or_ordinal_and_grouped_shape() {
        let doc = roxmltree::Document::parse(SLIDE_XML).unwrap();

        let title = parse_target("/slide[1]/placeholder[title]").unwrap();
        assert_eq!(resolve_paragraph(&doc, &title).unwrap().index, 1);

        let second = parse_target("/slide[1]/placeholder[2]").unwrap();
        assert_eq!(resolve_paragraph(&doc, &second).unwrap().index, 1);

        let grouped = parse_target("/slide[1]/group[1]/group[1]/shape[1]").unwrap();
        assert_eq!(resolve_paragraph(&doc, &grouped).unwrap().index, 1);
    }
}
