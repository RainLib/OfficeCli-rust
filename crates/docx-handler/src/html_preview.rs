use handler_common::HandlerError;
use oxml::OxmlPackage;
use std::collections::HashMap;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// Render the Word document as HTML for browser preview.
/// Overwritten to use direct roxmltree parsing on OxmlPackage parts for premium style layout representation.
pub fn view_as_html(package: &OxmlPackage) -> Result<String, HandlerError> {
    let doc_xml = package.read_part_xml("word/document.xml")
        .map_err(|e| HandlerError::OperationFailed(format!("Failed to read word/document.xml: {}", e)))?;
    let doc = roxmltree::Document::parse(&doc_xml)
        .map_err(|e| HandlerError::OperationFailed(format!("XML parse error in document.xml: {}", e)))?;

    // 1. Parse Styles
    let mut style_font_sizes = HashMap::new();
    let mut style_colors = HashMap::new();
    if let Ok(styles_xml) = package.read_part_xml("word/styles.xml") {
        if let Ok(styles_doc) = roxmltree::Document::parse(&styles_xml) {
            for style in styles_doc.descendants().filter(|n| n.has_tag_name("style")) {
                if let Some(style_id) = style.attribute((W_NS, "styleId")).or_else(|| style.attribute("w:styleId")) {
                    if let Some(r_pr) = style.children().find(|n| n.has_tag_name("rPr")) {
                        if let Some(sz) = r_pr.children().find(|n| n.has_tag_name("sz")) {
                            if let Some(val) = sz.attribute((W_NS, "val")).or_else(|| sz.attribute("w:val")) {
                                if let Ok(half_pt) = val.parse::<f64>() {
                                    style_font_sizes.insert(style_id.to_string(), half_pt / 2.0);
                                }
                            }
                        }
                        if let Some(color) = r_pr.children().find(|n| n.has_tag_name("color")) {
                            if let Some(val) = color.attribute((W_NS, "val")).or_else(|| color.attribute("w:val")) {
                                style_colors.insert(style_id.to_string(), format!("#{}", val));
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Parse Numbering formats
    let mut num_maps = HashMap::new();
    if let Ok(num_xml) = package.read_part_xml("word/numbering.xml") {
        if let Ok(num_doc) = roxmltree::Document::parse(&num_xml) {
            let mut abs_nums = HashMap::new();
            for abs in num_doc.descendants().filter(|n| n.has_tag_name("abstractNum")) {
                if let Some(id) = abs.attribute((W_NS, "abstractNumId")).or_else(|| abs.attribute("w:abstractNumId")) {
                    let mut lvls = HashMap::new();
                    for lvl in abs.children().filter(|n| n.has_tag_name("lvl")) {
                        if let Some(ilvl) = lvl.attribute((W_NS, "ilvl")).or_else(|| lvl.attribute("w:ilvl")) {
                            let fmt = lvl.children().find(|n| n.has_tag_name("numFmt"))
                                .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                                .unwrap_or("decimal").to_string();
                            let text = lvl.children().find(|n| n.has_tag_name("lvlText"))
                                .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                                .unwrap_or("").to_string();
                            let indent = lvl.children().find(|n| n.has_tag_name("pPr"))
                                .and_then(|p| p.children().find(|n| n.has_tag_name("ind")))
                                .and_then(|i| i.attribute((W_NS, "left")).or_else(|| i.attribute("w:left")))
                                .and_then(|s| s.parse::<f64>().ok())
                                .unwrap_or(0.0) / 20.0;
                            lvls.insert(ilvl.to_string(), (fmt, text, indent));
                        }
                    }
                    abs_nums.insert(id.to_string(), lvls);
                }
            }
            for num in num_doc.descendants().filter(|n| n.has_tag_name("num")) {
                if let Some(id) = num.attribute((W_NS, "numId")).or_else(|| num.attribute("w:numId")) {
                    if let Some(abs_ref) = num.children().find(|n| n.has_tag_name("abstractNumId")) {
                        if let Some(val) = abs_ref.attribute((W_NS, "val")).or_else(|| abs_ref.attribute("w:val")) {
                            if let Some(levels) = abs_nums.get(val) {
                                num_maps.insert(id.to_string(), levels.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    // 3. Document Relationships (for Hyperlinks)
    let doc_rels = package.part_rels("word/document.xml")
        .unwrap_or_else(|_| oxml::rels::Relationships::empty());

    // 4. Default dimensions
    let mut page_width = 612.0;  // Letter default (8.5 * 72)
    let mut page_height = 792.0; // Letter default (11 * 72)
    let mut margin_left = 72.0;
    let mut margin_right = 72.0;
    let mut margin_top = 72.0;
    let mut margin_bottom = 72.0;

    let body_node = doc.descendants().find(|n| n.has_tag_name("body"))
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    if let Some(sect) = body_node.children().find(|n| n.has_tag_name("sectPr")) {
        if let Some(sz) = sect.children().find(|n| n.has_tag_name("pgSz")) {
            if let Some(w) = sz.attribute((W_NS, "w")).or_else(|| sz.attribute("w:w")).and_then(|s| s.parse::<f64>().ok()) {
                page_width = w / 20.0;
            }
            if let Some(h) = sz.attribute((W_NS, "h")).or_else(|| sz.attribute("w:h")).and_then(|s| s.parse::<f64>().ok()) {
                page_height = h / 20.0;
            }
        }
        if let Some(mar) = sect.children().find(|n| n.has_tag_name("pgMar")) {
            margin_left = mar.attribute((W_NS, "left")).or_else(|| mar.attribute("w:left")).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1440.0) / 20.0;
            margin_right = mar.attribute((W_NS, "right")).or_else(|| mar.attribute("w:right")).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1440.0) / 20.0;
            margin_top = mar.attribute((W_NS, "top")).or_else(|| mar.attribute("w:top")).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1440.0) / 20.0;
            margin_bottom = mar.attribute((W_NS, "bottom")).or_else(|| mar.attribute("w:bottom")).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1440.0) / 20.0;
        }
    }

    let mut html_body = String::new();
    let mut num_counters: HashMap<String, usize> = HashMap::new();

    // Render paragraphs and tables
    for child in body_node.children() {
        let tag = child.tag_name().name();
        if tag == "p" {
            render_paragraph(&child, &mut html_body, &style_font_sizes, &style_colors, &num_maps, &mut num_counters, &doc_rels);
        } else if tag == "tbl" {
            render_table(&child, &mut html_body, &style_font_sizes, &style_colors);
        }
    }

    let body_width = page_width - margin_left - margin_right;

    Ok(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Word Preview</title>
<style>
body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Times New Roman", Times, serif;
    margin: 0;
    background: #f0f2f5;
    padding: 30px 10px;
    display: flex;
    flex-direction: column;
    align-items: center;
}}
.page-container {{
    display: flex;
    flex-direction: column;
    gap: 20px;
    width: 100%;
    align-items: center;
}}
.page {{
    width: {:.1}pt;
    min-height: {:.1}pt;
    padding: {:.1}pt {:.1}pt {:.1}pt {:.1}pt;
    background: white;
    box-shadow: 0 4px 20px rgba(0,0,0,0.15);
    box-sizing: border-box;
    position: relative;
}}
p, h1, h2, h3, h4, h5, h6 {{
    margin: 0 0 8px 0;
    line-height: 1.5;
    word-wrap: break-word;
}}
p.has-num {{
    position: relative;
}}
.list-number {{
    position: absolute;
    right: 100%;
    margin-right: 6px;
    text-align: right;
    color: #444;
}}
table {{
    border-collapse: collapse;
    width: 100%;
    margin-bottom: 12px;
}}
td, th {{
    border: 1px solid #ccc;
    padding: 6px 10px;
    vertical-align: top;
}}
.track-ins {{
    text-decoration: underline;
    color: #2E7D32;
}}
.track-del {{
    text-decoration: line-through;
    color: #C62828;
}}
a {{
    color: #1a73e8;
    text-decoration: none;
}}
a:hover {{
    text-decoration: underline;
}}
</style>
</head>
<body>
<div class="page-container">
  <div class="page">
    <div style="width:{:.1}pt; margin:0 auto;">
    {}
    </div>
  </div>
</div>
</body>
</html>"#, page_width, page_height, margin_top, margin_right, margin_bottom, margin_left, body_width, html_body))
}

fn render_paragraph(
    node: &roxmltree::Node,
    output: &mut String,
    style_font_sizes: &HashMap<String, f64>,
    style_colors: &HashMap<String, String>,
    num_maps: &HashMap<String, HashMap<String, (String, String, f64)>>,
    num_counters: &mut HashMap<String, usize>,
    rels: &oxml::rels::Relationships,
) {
    let p_pr = node.children().find(|n| n.has_tag_name("pPr"));
    let style_id = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("pStyle")))
        .and_then(|s| s.attribute((W_NS, "val")).or_else(|| s.attribute("w:val")))
        .unwrap_or("");

    // Alignments
    let align = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("jc")))
        .and_then(|j| j.attribute((W_NS, "val")).or_else(|| j.attribute("w:val")))
        .unwrap_or("left");

    // Line numbering / numbering list
    let mut num_prefix = String::new();
    let mut num_indent = 0.0;
    if let Some(pp) = p_pr.as_ref() {
        if let Some(num_pr) = pp.children().find(|n| n.has_tag_name("numPr")) {
            let num_id = num_pr.children().find(|n| n.has_tag_name("numId"))
                .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                .unwrap_or("");
            let ilvl = num_pr.children().find(|n| n.has_tag_name("ilvl"))
                .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                .unwrap_or("0");

            if let Some(levels) = num_maps.get(num_id) {
                if let Some((fmt, text, left_pt)) = levels.get(ilvl) {
                    let counter_key = format!("{}-{}", num_id, ilvl);
                    let count = num_counters.entry(counter_key).or_insert(0);
                    *count += 1;

                    let glyph = match fmt.as_str() {
                        "lowerRoman" => to_lower_roman(*count),
                        "upperRoman" => to_lower_roman(*count).to_uppercase(),
                        "lowerLetter" => if *count >= 1 && *count <= 26 { ((b'a' + (*count - 1) as u8) as char).to_string() } else { count.to_string() },
                        "upperLetter" => if *count >= 1 && *count <= 26 { ((b'A' + (*count - 1) as u8) as char).to_string() } else { count.to_string() },
                        _ => count.to_string()
                    };
                    num_prefix = text.replace(&format!("%{}", ilvl.parse::<usize>().unwrap_or(0) + 1), &glyph);
                    num_indent = *left_pt;
                }
            }
        }
    }

    let is_heading = style_id.starts_with("Heading");
    let tag = if is_heading {
        match style_id.chars().last().and_then(|c| c.to_digit(10)) {
            Some(1) => "h1",
            Some(2) => "h2",
            Some(3) => "h3",
            Some(4) => "h4",
            Some(5) => "h5",
            Some(6) => "h6",
            _ => "p"
        }
    } else {
        "p"
    };

    let mut styles = Vec::new();
    if align != "left" {
        styles.push(format!("text-align:{}", align));
    }
    if num_indent > 0.0 {
        styles.push(format!("margin-left:{:.1}pt; text-indent:-15pt;", num_indent));
    }

    let style_attr = if styles.is_empty() { String::new() } else { format!(" style=\"{}\"", styles.join("; ")) };
    let class_attr = if !num_prefix.is_empty() { " class=\"has-num\"" } else { "" };

    output.push_str(&format!("<{}{}{}>", tag, class_attr, style_attr));

    if !num_prefix.is_empty() {
        output.push_str(&format!("<span class=\"list-number\">{}</span>", num_prefix));
    }

    // Traverse runs and hyperlinks
    for child in node.children() {
        if child.has_tag_name("r") {
            render_run(&child, output, style_font_sizes, style_colors, style_id);
        } else if child.has_tag_name("hyperlink") {
            let rel_id = child.attribute((W_NS, "id")).or_else(|| child.attribute("r:id")).unwrap_or("");
            let anchor = child.attribute((W_NS, "anchor")).or_else(|| child.attribute("w:anchor")).unwrap_or("");
            let mut url = String::new();
            if !rel_id.is_empty() {
                if let Some(rel) = rels.get(rel_id) {
                    url = rel.target.clone();
                }
            } else if !anchor.is_empty() {
                url = format!("#{}", anchor);
            }

            if !url.is_empty() {
                output.push_str(&format!("<a href=\"{}\">", url));
            }
            for run_child in child.children().filter(|n| n.has_tag_name("r")) {
                render_run(&run_child, output, style_font_sizes, style_colors, style_id);
            }
            if !url.is_empty() {
                output.push_str("</a>");
            }
        }
    }

    output.push_str(&format!("</{}>\n", tag));
}

fn render_run(
    node: &roxmltree::Node,
    output: &mut String,
    style_font_sizes: &HashMap<String, f64>,
    style_colors: &HashMap<String, String>,
    para_style_id: &str,
) {
    let r_pr = node.children().find(|n| n.has_tag_name("rPr"));
    let mut styles = Vec::new();
    let mut is_bold = false;
    let mut is_italic = false;
    let mut is_underline = false;
    let mut is_strike = false;

    if let Some(rp) = r_pr.as_ref() {
        is_bold = rp.children().any(|n| n.has_tag_name("b"));
        is_italic = rp.children().any(|n| n.has_tag_name("i"));
        is_underline = rp.children().any(|n| n.has_tag_name("u"));
        is_strike = rp.children().any(|n| n.has_tag_name("strike"));

        if let Some(sz) = rp.children().find(|n| n.has_tag_name("sz")) {
            if let Some(val) = sz.attribute((W_NS, "val")).or_else(|| sz.attribute("w:val")) {
                if let Ok(half_pt) = val.parse::<f64>() {
                    styles.push(format!("font-size:{:.1}pt", half_pt / 2.0));
                }
            }
        } else if let Some(p_sz) = style_font_sizes.get(para_style_id) {
            styles.push(format!("font-size:{:.1}pt", p_sz));
        }

        if let Some(color) = rp.children().find(|n| n.has_tag_name("color")) {
            if let Some(val) = color.attribute((W_NS, "val")).or_else(|| color.attribute("w:val")) {
                styles.push(format!("color:#{}", val));
            }
        } else if let Some(p_col) = style_colors.get(para_style_id) {
            styles.push(format!("color:{}", p_col));
        }
    }

    if is_bold { styles.push("font-weight:bold".to_string()); }
    if is_italic { styles.push("font-style:italic".to_string()); }
    if is_underline { styles.push("text-decoration:underline".to_string()); }
    if is_strike { styles.push("text-decoration:line-through".to_string()); }

    let style_attr = if styles.is_empty() { String::new() } else { format!(" style=\"{}\"", styles.join("; ")) };
    let has_span = !style_attr.is_empty();

    if has_span {
        output.push_str(&format!("<span{}>", style_attr));
    }

    // Traverse texts and symbols
    for child in node.children() {
        if child.has_tag_name("t") {
            output.push_str(&html_escape(child.text().unwrap_or("")));
        } else if child.has_tag_name("tab") {
            output.push_str("<span style=\"display:inline-block; width:36pt;\"></span>");
        } else if child.has_tag_name("br") {
            output.push_str("<br>");
        }
    }

    if has_span {
        output.push_str("</span>");
    }
}

fn render_table(
    node: &roxmltree::Node,
    output: &mut String,
    style_font_sizes: &HashMap<String, f64>,
    style_colors: &HashMap<String, String>,
) {
    output.push_str("<table>\n");
    for row in node.children().filter(|n| n.has_tag_name("tr")) {
        output.push_str("<tr>\n");
        for cell in row.children().filter(|n| n.has_tag_name("tc")) {
            let tc_pr = cell.children().find(|n| n.has_tag_name("tcPr"));
            let mut span_attrs = String::new();

            // Row / Col merge calculation (vMerge / gridSpan)
            if let Some(tp) = tc_pr.as_ref() {
                if let Some(gs) = tp.children().find(|n| n.has_tag_name("gridSpan")) {
                    if let Some(val) = gs.attribute((W_NS, "val")).or_else(|| gs.attribute("w:val")) {
                        span_attrs.push_str(&format!(" colspan=\"{}\"", val));
                    }
                }
            }

            output.push_str(&format!("<td{}>", span_attrs));
            let mut cell_body = String::new();
            let mut dummy_counters = HashMap::new();
            let dummy_rels = oxml::rels::Relationships::empty();
            let dummy_num = HashMap::new();
            for child in cell.children() {
                let tag = child.tag_name().name();
                if tag == "p" {
                    render_paragraph(&child, &mut cell_body, style_font_sizes, style_colors, &dummy_num, &mut dummy_counters, &dummy_rels);
                } else if tag == "tbl" {
                    render_table(&child, &mut cell_body, style_font_sizes, style_colors);
                }
            }
            output.push_str(&cell_body);
            output.push_str("</td>\n");
        }
        output.push_str("</tr>\n");
    }
    output.push_str("</table>\n");
}

fn to_lower_roman(mut num: usize) -> String {
    if num == 0 || num > 3999 { return num.to_string(); }
    let mut sb = String::new();
    let map = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"),
        (100, "c"), (90, "xc"), (50, "l"), (40, "xl"),
        (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")
    ];
    for &(value, roman) in &map {
        while num >= value {
            sb.push_str(roman);
            num -= value;
        }
    }
    sb
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
