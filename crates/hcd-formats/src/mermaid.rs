use hcd_core::{stable_node_id, HcdError};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashMap;

const MERMAID_CLASS: &str = "class=\"language-mermaid\"";
const MAX_MERMAID_SOURCE_BYTES: usize = 256 * 1024;
const MAX_MERMAID_NODES: usize = 256;
const MAX_MERMAID_EDGES: usize = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    TopDown,
    BottomUp,
    LeftRight,
    RightLeft,
}

#[derive(Clone, Copy)]
enum NodeShape {
    Box,
    Rounded,
    Circle,
    Diamond,
}

struct GraphNode {
    id: String,
    label: String,
    shape: NodeShape,
}

struct GraphEdge {
    from: usize,
    to: usize,
    label: String,
    dashed: bool,
    directed: bool,
}

struct GraphDiagram {
    direction: Direction,
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
}

struct SequenceMessage {
    from: usize,
    to: usize,
    label: String,
    dashed: bool,
}

struct SequenceDiagram {
    participants: Vec<(String, String)>,
    messages: Vec<SequenceMessage>,
}

/// Add a derived, read-only SVG next to editable Mermaid fenced code blocks.
/// The canonical HCD chunk is never changed; this function runs against the
/// selected revision's materialized text, so nodeId patches immediately affect
/// the next preview or PDF export.
pub(crate) fn enhance_fragment(html: &str) -> Result<String, HcdError> {
    let mut output = String::with_capacity(html.len());
    let mut remainder = html;
    while let Some(marker) = remainder.find(MERMAID_CLASS) {
        let Some(pre_start) = remainder[..marker].rfind("<pre") else {
            output.push_str(&remainder[..marker + MERMAID_CLASS.len()]);
            remainder = &remainder[marker + MERMAID_CLASS.len()..];
            continue;
        };
        let Some(code_open_end) = remainder[marker..]
            .find('>')
            .map(|offset| marker + offset + 1)
        else {
            break;
        };
        let Some(close_offset) = remainder[code_open_end..].find("</code></pre>") else {
            break;
        };
        let close_start = code_open_end + close_offset;
        let block_end = close_start + "</code></pre>".len();
        output.push_str(&remainder[..pre_start]);
        let original = &remainder[pre_start..block_end];
        let inner = &remainder[code_open_end..close_start];
        match extract_source(inner).and_then(|(source_node_id, source)| {
            let diagram_id = stable_node_id(&[&source_node_id, "mermaid-preview", "v1"]);
            render_mermaid(&source).map(|svg| (source_node_id, diagram_id, svg))
        }) {
            Ok((source_node_id, diagram_id, svg)) => {
                output.push_str(&format!(
                    "<figure class=\"hcd-source-block hcd-mermaid\" data-hcd-id=\"{diagram_id}\" data-hcd-node-kind=\"diagram\" data-hcd-editable=\"false\" data-hcd-source-node-id=\"{source_node_id}\"><div class=\"hcd-mermaid-preview\">{svg}</div><details class=\"hcd-mermaid-source\"><summary>Mermaid source</summary>{original}</details></figure>"
                ));
            }
            Err(error) => {
                output.push_str(original);
                output.push_str(&format!(
                    "<div class=\"hcd-mermaid-error\" data-hcd-render-status=\"unsupported\">Mermaid preview unavailable: {}</div>",
                    escape_text(&error.to_string())
                ));
            }
        }
        remainder = &remainder[block_end..];
    }
    output.push_str(remainder);
    Ok(output)
}

fn extract_source(inner: &str) -> Result<(String, String), HcdError> {
    let wrapped = format!("<root>{inner}</root>");
    let mut reader = Reader::from_str(&wrapped);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut source_node_id = None;
    let mut source = String::new();
    loop {
        match reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("Mermaid HTML parse error: {error}"))
        })? {
            Event::Start(event) | Event::Empty(event) => {
                for attribute in event.attributes().with_checks(true) {
                    let attribute = attribute.map_err(|error| {
                        HcdError::InvalidBundle(format!("Mermaid HTML attribute error: {error}"))
                    })?;
                    if attribute.key.as_ref() == b"data-hcd-id" && source_node_id.is_none() {
                        source_node_id = Some(
                            attribute
                                .decode_and_unescape_value(reader.decoder())
                                .map_err(|error| {
                                    HcdError::InvalidBundle(format!(
                                        "Mermaid source nodeId is invalid: {error}"
                                    ))
                                })?
                                .into_owned(),
                        );
                    }
                }
            }
            Event::Text(text) => source.push_str(&text.unescape().map_err(|error| {
                HcdError::InvalidBundle(format!("Mermaid source text is invalid: {error}"))
            })?),
            Event::CData(text) => {
                source.push_str(&reader.decoder().decode(text.as_ref()).map_err(|error| {
                    HcdError::InvalidBundle(format!("Mermaid source CDATA is invalid: {error}"))
                })?)
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    let source_node_id = source_node_id.ok_or_else(|| {
        HcdError::InvalidBundle("Mermaid code has no canonical source nodeId".to_string())
    })?;
    if source.len() > MAX_MERMAID_SOURCE_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "Mermaid source exceeds {MAX_MERMAID_SOURCE_BYTES} bytes"
        )));
    }
    Ok((source_node_id, source))
}

fn render_mermaid(source: &str) -> Result<String, HcdError> {
    let first = statements(source)
        .into_iter()
        .find(|statement| !statement.is_empty() && !statement.starts_with("%%"))
        .unwrap_or_default();
    let lower = first.to_ascii_lowercase();
    if lower == "sequencediagram" {
        render_sequence(&parse_sequence(source)?)
    } else if lower.starts_with("graph ")
        || lower == "graph"
        || lower.starts_with("flowchart ")
        || lower == "flowchart"
    {
        render_graph(&parse_graph(source)?)
    } else {
        Err(HcdError::Unsupported(
            "supported Mermaid diagrams are flowchart/graph and sequenceDiagram".to_string(),
        ))
    }
}

fn statements(source: &str) -> Vec<&str> {
    source
        .lines()
        .flat_map(|line| line.split(';'))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn parse_graph(source: &str) -> Result<GraphDiagram, HcdError> {
    let mut direction = Direction::TopDown;
    let mut nodes = Vec::new();
    let mut node_index = HashMap::new();
    let mut edges = Vec::new();
    for statement in statements(source) {
        if statement.starts_with("%%") {
            continue;
        }
        let lower = statement.to_ascii_lowercase();
        if lower.starts_with("graph") || lower.starts_with("flowchart") {
            direction = match lower.split_whitespace().nth(1).unwrap_or("td") {
                "lr" => Direction::LeftRight,
                "rl" => Direction::RightLeft,
                "bt" => Direction::BottomUp,
                _ => Direction::TopDown,
            };
            continue;
        }
        if lower.starts_with("style ")
            || lower.starts_with("class ")
            || lower.starts_with("classdef ")
            || lower.starts_with("linkstyle ")
            || lower.starts_with("click ")
            || lower.starts_with("subgraph")
            || lower == "end"
            || lower.starts_with("direction ")
        {
            continue;
        }
        let (tokens, operators) = split_graph_chain(statement);
        if tokens.is_empty() {
            continue;
        }
        let indexes = tokens
            .iter()
            .map(|token| graph_node(token, &mut nodes, &mut node_index))
            .collect::<Result<Vec<_>, _>>()?;
        for (index, operator) in operators.iter().enumerate() {
            if edges.len() >= MAX_MERMAID_EDGES {
                return Err(HcdError::ResourceLimit(format!(
                    "Mermaid diagram exceeds {MAX_MERMAID_EDGES} edges"
                )));
            }
            edges.push(GraphEdge {
                from: indexes[index],
                to: indexes[index + 1],
                label: operator.label.clone(),
                dashed: operator.dashed,
                directed: operator.directed,
            });
        }
    }
    if nodes.is_empty() {
        return Err(HcdError::InvalidBundle(
            "Mermaid flowchart has no nodes".to_string(),
        ));
    }
    Ok(GraphDiagram {
        direction,
        nodes,
        edges,
    })
}

struct LinkOperator {
    label: String,
    dashed: bool,
    directed: bool,
}

fn split_graph_chain(statement: &str) -> (Vec<String>, Vec<LinkOperator>) {
    let mut tokens = Vec::new();
    let mut operators = Vec::new();
    let mut remainder = statement.trim();
    loop {
        let Some((offset, operator, dashed, directed)) = find_graph_operator(remainder) else {
            if !remainder.trim().is_empty() {
                tokens.push(remainder.trim().to_string());
            }
            break;
        };
        tokens.push(remainder[..offset].trim().to_string());
        remainder = &remainder[offset + operator.len()..];
        let mut label = String::new();
        if let Some(rest) = remainder.strip_prefix('|') {
            if let Some(end) = rest.find('|') {
                label = rest[..end].trim().to_string();
                remainder = &rest[end + 1..];
            }
        }
        operators.push(LinkOperator {
            label,
            dashed,
            directed,
        });
    }
    if tokens.len() != operators.len().saturating_add(1) {
        (Vec::new(), Vec::new())
    } else {
        (tokens, operators)
    }
}

fn find_graph_operator(value: &str) -> Option<(usize, &'static str, bool, bool)> {
    [
        ("-.->", true, true),
        ("==>", false, true),
        ("-->", false, true),
        ("---", false, false),
        ("-.-", true, false),
    ]
    .into_iter()
    .filter_map(|(operator, dashed, directed)| {
        value
            .find(operator)
            .map(|offset| (offset, operator, dashed, directed))
    })
    .min_by_key(|(offset, _, _, _)| *offset)
}

fn graph_node(
    token: &str,
    nodes: &mut Vec<GraphNode>,
    indexes: &mut HashMap<String, usize>,
) -> Result<usize, HcdError> {
    let token = token.trim().trim_matches('|').trim();
    let id_end = token
        .char_indices()
        .take_while(|(_, character)| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        })
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    if id_end == 0 {
        return Err(HcdError::InvalidBundle(format!(
            "invalid Mermaid node token {token}"
        )));
    }
    let id = &token[..id_end];
    let rest = token[id_end..].trim();
    let (label, shape) = node_label(rest).unwrap_or_else(|| (id.to_string(), NodeShape::Box));
    let label = bounded_label(&label);
    if let Some(index) = indexes.get(id).copied() {
        if !rest.is_empty() {
            nodes[index].label = label;
            nodes[index].shape = shape;
        }
        return Ok(index);
    }
    if nodes.len() >= MAX_MERMAID_NODES {
        return Err(HcdError::ResourceLimit(format!(
            "Mermaid diagram exceeds {MAX_MERMAID_NODES} nodes"
        )));
    }
    let index = nodes.len();
    indexes.insert(id.to_string(), index);
    nodes.push(GraphNode {
        id: id.to_string(),
        label,
        shape,
    });
    Ok(index)
}

fn node_label(value: &str) -> Option<(String, NodeShape)> {
    let value = value.trim();
    for (open, close, shape) in [
        ("((", "))", NodeShape::Circle),
        ("{", "}", NodeShape::Diamond),
        ("([", "])", NodeShape::Rounded),
        ("(", ")", NodeShape::Rounded),
        ("[", "]", NodeShape::Box),
    ] {
        if let Some(inner) = value
            .strip_prefix(open)
            .and_then(|rest| rest.strip_suffix(close))
        {
            return Some((inner.trim_matches('"').trim().to_string(), shape));
        }
    }
    None
}

fn render_graph(graph: &GraphDiagram) -> Result<String, HcdError> {
    let mut ranks = vec![0usize; graph.nodes.len()];
    for _ in 0..graph.nodes.len() {
        let before = ranks.clone();
        for edge in &graph.edges {
            let next = ranks[edge.from]
                .saturating_add(1)
                .min(graph.nodes.len() - 1);
            ranks[edge.to] = ranks[edge.to].max(next);
        }
        if ranks == before {
            break;
        }
    }
    let mut slots = HashMap::<usize, usize>::new();
    let node_width = 180.0f32;
    let node_height = 56.0f32;
    let main_gap = 78.0f32;
    let cross_gap = 36.0f32;
    let margin = 30.0f32;
    let mut positions = Vec::with_capacity(graph.nodes.len());
    for rank in &ranks {
        let slot = slots.entry(*rank).or_default();
        let (mut x, mut y) =
            if matches!(graph.direction, Direction::LeftRight | Direction::RightLeft) {
                (
                    margin + *rank as f32 * (node_width + main_gap),
                    margin + *slot as f32 * (node_height + cross_gap),
                )
            } else {
                (
                    margin + *slot as f32 * (node_width + cross_gap),
                    margin + *rank as f32 * (node_height + main_gap),
                )
            };
        if graph.direction == Direction::RightLeft {
            x = margin
                + (graph.nodes.len().saturating_sub(1) - *rank) as f32 * (node_width + main_gap);
        }
        if graph.direction == Direction::BottomUp {
            y = margin
                + (graph.nodes.len().saturating_sub(1) - *rank) as f32 * (node_height + main_gap);
        }
        positions.push((x, y));
        *slot += 1;
    }
    let width = positions
        .iter()
        .map(|(x, _)| x + node_width + margin)
        .fold(0.0, f32::max)
        .max(300.0);
    let height = positions
        .iter()
        .map(|(_, y)| y + node_height + margin)
        .fold(0.0, f32::max)
        .max(140.0);
    let mut body = String::new();
    for edge in &graph.edges {
        let (sx, sy) = positions[edge.from];
        let (tx, ty) = positions[edge.to];
        let (x1, y1, x2, y2) = if matches!(graph.direction, Direction::LeftRight) {
            (
                sx + node_width,
                sy + node_height / 2.0,
                tx,
                ty + node_height / 2.0,
            )
        } else if matches!(graph.direction, Direction::RightLeft) {
            (
                sx,
                sy + node_height / 2.0,
                tx + node_width,
                ty + node_height / 2.0,
            )
        } else if matches!(graph.direction, Direction::BottomUp) {
            (
                sx + node_width / 2.0,
                sy,
                tx + node_width / 2.0,
                ty + node_height,
            )
        } else {
            (
                sx + node_width / 2.0,
                sy + node_height,
                tx + node_width / 2.0,
                ty,
            )
        };
        body.push_str(&svg_line(x1, y1, x2, y2, edge.dashed, edge.directed));
        if !edge.label.is_empty() {
            body.push_str(&svg_text(
                (x1 + x2) / 2.0,
                (y1 + y2) / 2.0 - 8.0,
                &bounded_label(&edge.label),
                12,
            ));
        }
    }
    for (index, node) in graph.nodes.iter().enumerate() {
        let (x, y) = positions[index];
        body.push_str(&format!(
            "<g data-mermaid-node=\"{}\">",
            escape_attribute(&node.id)
        ));
        match node.shape {
            NodeShape::Circle => body.push_str(&format!(
                "<ellipse cx=\"{:.2}\" cy=\"{:.2}\" rx=\"{:.2}\" ry=\"{:.2}\" fill=\"#eef6ff\" stroke=\"#3b82c4\" stroke-width=\"2\"/>",
                x + node_width / 2.0,
                y + node_height / 2.0,
                node_width / 2.0,
                node_height / 2.0
            )),
            NodeShape::Diamond => body.push_str(&format!(
                "<polygon points=\"{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} {:.2},{:.2}\" fill=\"#fff8e6\" stroke=\"#b7791f\" stroke-width=\"2\"/>",
                x + node_width / 2.0,
                y,
                x + node_width,
                y + node_height / 2.0,
                x + node_width / 2.0,
                y + node_height,
                x,
                y + node_height / 2.0
            )),
            NodeShape::Rounded | NodeShape::Box => body.push_str(&format!(
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{node_width:.2}\" height=\"{node_height:.2}\" rx=\"{}\" fill=\"#eef6ff\" stroke=\"#3b82c4\" stroke-width=\"2\"/>",
                if matches!(node.shape, NodeShape::Rounded) { 18 } else { 5 }
            )),
        }
        body.push_str(&svg_text(
            x + node_width / 2.0,
            y + node_height / 2.0,
            &node.label,
            14,
        ));
        body.push_str("</g>");
    }
    Ok(svg_document(width, height, &body))
}

fn parse_sequence(source: &str) -> Result<SequenceDiagram, HcdError> {
    let mut participants = Vec::<(String, String)>::new();
    let mut indexes = HashMap::<String, usize>::new();
    let mut messages = Vec::new();
    for statement in statements(source) {
        if statement.starts_with("%%") || statement.eq_ignore_ascii_case("sequenceDiagram") {
            continue;
        }
        let lower = statement.to_ascii_lowercase();
        if lower.starts_with("participant ") || lower.starts_with("actor ") {
            let declaration = statement
                .split_once(' ')
                .map(|(_, value)| value)
                .unwrap_or("");
            let (id, label) = declaration
                .split_once(" as ")
                .unwrap_or((declaration, declaration));
            sequence_participant(id.trim(), label.trim(), &mut participants, &mut indexes)?;
            continue;
        }
        let Some((offset, operator, dashed)) = find_sequence_operator(statement) else {
            continue;
        };
        let from = statement[..offset].trim();
        let tail = &statement[offset + operator.len()..];
        let (to, label) = tail.split_once(':').unwrap_or((tail, ""));
        let from = sequence_participant(from, from, &mut participants, &mut indexes)?;
        let to = sequence_participant(to.trim(), to.trim(), &mut participants, &mut indexes)?;
        if messages.len() >= MAX_MERMAID_EDGES {
            return Err(HcdError::ResourceLimit(format!(
                "Mermaid sequence exceeds {MAX_MERMAID_EDGES} messages"
            )));
        }
        messages.push(SequenceMessage {
            from,
            to,
            label: bounded_label(label),
            dashed,
        });
    }
    if participants.is_empty() {
        return Err(HcdError::InvalidBundle(
            "Mermaid sequenceDiagram has no participants".to_string(),
        ));
    }
    Ok(SequenceDiagram {
        participants,
        messages,
    })
}

fn sequence_participant(
    id: &str,
    label: &str,
    participants: &mut Vec<(String, String)>,
    indexes: &mut HashMap<String, usize>,
) -> Result<usize, HcdError> {
    let id = id.trim();
    if id.is_empty()
        || !id
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(HcdError::InvalidBundle(format!(
            "invalid Mermaid participant {id}"
        )));
    }
    if let Some(index) = indexes.get(id).copied() {
        if label != id {
            participants[index].1 = bounded_label(label);
        }
        return Ok(index);
    }
    if participants.len() >= MAX_MERMAID_NODES {
        return Err(HcdError::ResourceLimit(format!(
            "Mermaid sequence exceeds {MAX_MERMAID_NODES} participants"
        )));
    }
    let index = participants.len();
    participants.push((id.to_string(), bounded_label(label)));
    indexes.insert(id.to_string(), index);
    Ok(index)
}

fn find_sequence_operator(value: &str) -> Option<(usize, &'static str, bool)> {
    [("-->>", true), ("->>", false), ("-->", true), ("->", false)]
        .into_iter()
        .filter_map(|(operator, dashed)| {
            value
                .find(operator)
                .map(|offset| (offset, operator, dashed))
        })
        .min_by_key(|(offset, _, _)| *offset)
}

fn render_sequence(sequence: &SequenceDiagram) -> Result<String, HcdError> {
    let margin = 28.0f32;
    let participant_width = 140.0f32;
    let participant_height = 42.0f32;
    let gap = 70.0f32;
    let message_gap = 62.0f32;
    let width = margin * 2.0
        + sequence.participants.len() as f32 * participant_width
        + sequence.participants.len().saturating_sub(1) as f32 * gap;
    let height = margin * 2.0
        + participant_height * 2.0
        + sequence.messages.len().max(1) as f32 * message_gap;
    let mut body = String::new();
    for (index, (id, label)) in sequence.participants.iter().enumerate() {
        let x = margin + index as f32 * (participant_width + gap);
        let center = x + participant_width / 2.0;
        body.push_str(&format!(
            "<line x1=\"{center:.2}\" y1=\"{:.2}\" x2=\"{center:.2}\" y2=\"{:.2}\" stroke=\"#94a3b8\" stroke-width=\"1.5\" stroke-dasharray=\"6 5\"/>",
            margin + participant_height,
            height - margin - participant_height
        ));
        for y in [margin, height - margin - participant_height] {
            body.push_str(&format!(
                "<rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{participant_width:.2}\" height=\"{participant_height:.2}\" rx=\"6\" fill=\"#eef6ff\" stroke=\"#3b82c4\" stroke-width=\"2\" data-mermaid-participant=\"{}\"/>",
                escape_attribute(id)
            ));
            body.push_str(&svg_text(center, y + participant_height / 2.0, label, 14));
        }
    }
    for (index, message) in sequence.messages.iter().enumerate() {
        let y = margin + participant_height + 38.0 + index as f32 * message_gap;
        let x1 = margin + message.from as f32 * (participant_width + gap) + participant_width / 2.0;
        let x2 = margin + message.to as f32 * (participant_width + gap) + participant_width / 2.0;
        body.push_str(&svg_line(x1, y, x2, y, message.dashed, true));
        if !message.label.is_empty() {
            body.push_str(&svg_text((x1 + x2) / 2.0, y - 12.0, &message.label, 12));
        }
    }
    Ok(svg_document(width.max(300.0), height.max(180.0), &body))
}

fn svg_document(width: f32, height: f32, body: &str) -> String {
    let display_width = width.min(760.0);
    let display_height = height * display_width / width;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{display_width:.2}\" height=\"{display_height:.2}\" viewBox=\"0 0 {width:.2} {height:.2}\" role=\"img\" data-hcd-mermaid-svg=\"true\"><rect x=\"0\" y=\"0\" width=\"{width:.2}\" height=\"{height:.2}\" fill=\"#ffffff\"/>{body}</svg>"
    )
}

fn svg_line(x1: f32, y1: f32, x2: f32, y2: f32, dashed: bool, arrow: bool) -> String {
    let dash = if dashed {
        " stroke-dasharray=\"7 5\""
    } else {
        ""
    };
    let mut output = format!(
        "<line x1=\"{x1:.2}\" y1=\"{y1:.2}\" x2=\"{x2:.2}\" y2=\"{y2:.2}\" stroke=\"#475569\" stroke-width=\"2\"{dash}/>"
    );
    if arrow {
        let dx = x2 - x1;
        let dy = y2 - y1;
        let length = (dx * dx + dy * dy).sqrt().max(1.0);
        let ux = dx / length;
        let uy = dy / length;
        let px = -uy;
        let py = ux;
        let back_x = x2 - ux * 11.0;
        let back_y = y2 - uy * 11.0;
        output.push_str(&format!(
            "<polygon points=\"{x2:.2},{y2:.2} {:.2},{:.2} {:.2},{:.2}\" fill=\"#475569\"/>",
            back_x + px * 5.0,
            back_y + py * 5.0,
            back_x - px * 5.0,
            back_y - py * 5.0
        ));
    }
    output
}

fn svg_text(x: f32, y: f32, text: &str, size: u8) -> String {
    format!(
        "<text x=\"{x:.2}\" y=\"{y:.2}\" fill=\"#172033\" font-family=\"HCDSans,HCDEmoji,sans-serif\" font-size=\"{size}\" text-anchor=\"middle\" dominant-baseline=\"middle\">{}</text>",
        escape_text(text)
    )
}

fn bounded_label(value: &str) -> String {
    let mut label = value.trim().chars().take(80).collect::<String>();
    if value.trim().chars().count() > 80 {
        label.push('…');
    }
    label
}

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attribute(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flowchart_is_derived_from_the_current_canonical_node() {
        let html = "<pre class=\"hcd-source-block hcd-markdown-code\" data-hcd-fenced=\"true\"><code class=\"language-mermaid\"><span data-hcd-id=\"n_0123456789abcdef0123456789abcdef\" data-hcd-node-hash=\"hash\">graph TD\nA[Markdown] --&gt; B{HCD}\nB --&gt; C[HTML]</span></code></pre>";
        let enhanced = enhance_fragment(html).unwrap();
        assert!(enhanced.contains("class=\"hcd-mermaid-preview\""));
        assert!(enhanced.contains("<svg"));
        assert!(enhanced.contains(">Markdown</text>"));
        assert!(enhanced.contains(">HCD</text>"));
        assert!(enhanced.contains("data-hcd-source-node-id=\"n_0123456789abcdef0123456789abcdef\""));
        assert!(enhanced.contains(html));
        assert_eq!(enhanced, enhance_fragment(html).unwrap());
    }

    #[test]
    fn sequence_diagram_is_bounded_and_escaped() {
        let svg = render_mermaid(
            "sequenceDiagram\nparticipant A as Alice\nparticipant B as Bob\nA->>B: Hello <team>\nB-->>A: Reply",
        )
        .unwrap();
        assert!(svg.contains("data-mermaid-participant=\"A\""));
        assert!(svg.contains("Hello &lt;team&gt;"));
        assert!(!svg.contains("<team>"));
        assert!(svg.contains("stroke-dasharray"));
    }

    #[test]
    fn unsupported_diagram_never_executes_source_as_markup() {
        let error = render_mermaid("gantt\n<script>alert(1)</script>").unwrap_err();
        assert!(error.to_string().contains("supported Mermaid diagrams"));
    }
}
