use handler_common::HandlerError;
use oxml::OxmlPackage;
use std::collections::{HashMap, HashSet};

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn get_node_attr<'a, 'input>(node: &roxmltree::Node<'a, 'input>, local_name: &str) -> Option<&'a str> {
    node.attribute((W_NS, local_name))
        .or_else(|| node.attribute(("http://schemas.openxmlformats.org/officeDocument/2006/relationships", local_name)))
        .or_else(|| node.attribute(local_name))
        .or_else(|| {
            node.attributes().iter().find(|a| {
                let name = a.name();
                if name == local_name {
                    return true;
                }
                if name.len() == local_name.len() + 2 {
                    if name.starts_with("w:") && &name[2..] == local_name {
                        return true;
                    }
                    if name.starts_with("r:") && &name[2..] == local_name {
                        return true;
                    }
                }
                false
            }).map(|a| a.value())
        })
}


struct DocDefaults {
    font: String,
    size_pt: f64,
    line_height: f64,
    color: String,
    space_after_pt: f64,
    default_align: String,
}

#[derive(Clone, Debug)]
struct DocxStyle {
    style_id: String,
    based_on: Option<String>,
    is_default_paragraph: bool,
    font_ascii: Option<String>,
    font_east_asia: Option<String>,
    size_pt: Option<f64>,
    color: Option<String>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    spacing_before_pt: Option<f64>,
    spacing_after_pt: Option<f64>,
    line_spacing_mult: Option<f64>,
    line_spacing_exact: Option<f64>,
    alignment: Option<String>,
    shading_fill: Option<String>,
    num_id: Option<String>,
    ilvl: Option<usize>,
    name: Option<String>,
}

#[derive(Clone, Debug)]
struct NumLevel {
    num_fmt: String,
    lvl_text: String,
    left_pt: f64,
    hanging_pt: f64,
    start: usize,
    jc: String,
    font_name: Option<String>,
    font_size_pt: Option<f64>,
    color: Option<String>,
    bold: bool,
    italic: bool,
}

struct PageLayout {
    width_pt: f64,
    height_pt: f64,
    margin_top_pt: f64,
    margin_bottom_pt: f64,
    margin_left_pt: f64,
    margin_right_pt: f64,
    header_distance_pt: f64,
    footer_distance_pt: f64,
}

struct HeaderFooterBundle {
    first: Option<String>,
    default: Option<String>,
    even: Option<String>,
}

struct ParagraphMetrics {
    align: String,
    space_before_pt: f64,
    space_after_pt: f64,
    line_spacing_mult: Option<f64>,
    line_spacing_exact: Option<f64>,
    margin_left_pt: f64,
    text_indent_pt: f64,
    shading_fill: Option<String>,
}

fn get_theme_minor_font(package: &OxmlPackage) -> Option<String> {
    if let Ok(theme_xml) = package.read_part_xml("word/theme/theme1.xml") {
        if let Ok(theme_doc) = roxmltree::Document::parse(&theme_xml) {
            if let Some(minor) = theme_doc.descendants().find(|n| n.has_tag_name("minorFont")) {
                if let Some(latin) = minor.children().find(|n| n.has_tag_name("latin")) {
                    if let Some(typeface) = latin.attribute("typeface") {
                        return Some(typeface.to_string());
                    }
                }
                if let Some(ea) = minor.children().find(|n| n.has_tag_name("ea")) {
                    if let Some(typeface) = ea.attribute("typeface") {
                        return Some(typeface.to_string());
                    }
                }
            }
        }
    }
    None
}

fn parse_styles(package: &OxmlPackage) -> (HashMap<String, DocxStyle>, DocDefaults) {
    let mut styles_map = HashMap::new();
    let mut doc_defaults = DocDefaults {
        font: "Calibri".to_string(),
        size_pt: 11.0,
        line_height: 1.15,
        color: "#000000".to_string(),
        space_after_pt: 0.0,
        default_align: "left".to_string(),
    };

    if let Some(theme_minor) = get_theme_minor_font(package) {
        doc_defaults.font = theme_minor;
    }

    if let Ok(styles_xml) = package.read_part_xml("word/styles.xml") {
        if let Ok(styles_doc) = roxmltree::Document::parse(&styles_xml) {
            if let Some(defaults_node) = styles_doc.descendants().find(|n| n.has_tag_name("docDefaults")) {
                if let Some(r_pr) = defaults_node.descendants().find(|n| n.has_tag_name("rPrDefault"))
                    .and_then(|n| n.descendants().find(|c| c.has_tag_name("rPr")))
                {
                    if let Some(rf) = r_pr.children().find(|n| n.has_tag_name("rFonts")) {
                        if let Some(ascii) = rf.attribute((W_NS, "ascii")).or_else(|| rf.attribute("w:ascii")) {
                            doc_defaults.font = ascii.to_string();
                        }
                    }
                    if let Some(sz) = r_pr.children().find(|n| n.has_tag_name("sz")) {
                        if let Some(val) = sz.attribute((W_NS, "val")).or_else(|| sz.attribute("w:val")) {
                            if let Ok(half_pt) = val.parse::<f64>() {
                                doc_defaults.size_pt = half_pt / 2.0;
                            }
                        }
                    }
                    if let Some(color_el) = r_pr.children().find(|n| n.has_tag_name("color")) {
                        if let Some(val) = color_el.attribute((W_NS, "val")).or_else(|| color_el.attribute("w:val")) {
                            if val != "auto" && !val.is_empty() {
                                doc_defaults.color = if val.starts_with('#') { val.to_string() } else { format!("#{}", val) };
                            }
                        }
                    }
                }
                
                if let Some(p_pr) = defaults_node.descendants().find(|n| n.has_tag_name("pPrDefault"))
                    .and_then(|n| n.descendants().find(|c| c.has_tag_name("pPr")))
                {
                    if let Some(spacing) = p_pr.children().find(|n| n.has_tag_name("spacing")) {
                        if let Some(after) = spacing.attribute((W_NS, "after")).or_else(|| spacing.attribute("w:after")) {
                            if let Ok(twips) = after.parse::<f64>() {
                                doc_defaults.space_after_pt = twips / 20.0;
                            }
                        }
                        if let Some(line) = spacing.attribute((W_NS, "line")).or_else(|| spacing.attribute("w:line")) {
                            if let Ok(twips) = line.parse::<f64>() {
                                let rule = spacing.attribute((W_NS, "lineRule")).or_else(|| spacing.attribute("w:lineRule")).unwrap_or("auto");
                                if rule == "auto" {
                                    doc_defaults.line_height = twips / 240.0;
                                }
                            }
                        }
                    }
                    if let Some(jc) = p_pr.children().find(|n| n.has_tag_name("jc")) {
                        if let Some(val) = jc.attribute((W_NS, "val")).or_else(|| jc.attribute("w:val")) {
                            doc_defaults.default_align = match val {
                                "center" => "center".to_string(),
                                "right" | "end" => "right".to_string(),
                                "both" | "distribute" => "justify".to_string(),
                                _ => "left".to_string(),
                            };
                        }
                    }
                }
            }

            for style in styles_doc.descendants().filter(|n| n.has_tag_name("style")) {
                if let Some(style_id) = style.attribute((W_NS, "styleId")).or_else(|| style.attribute("w:styleId")) {
                    let based_on = style.children().find(|n| n.has_tag_name("basedOn"))
                        .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                        .map(|s| s.to_string());
                    
                    let style_type = style.attribute((W_NS, "type")).or_else(|| style.attribute("w:type")).unwrap_or("");
                    let is_default = style.attribute((W_NS, "default")).or_else(|| style.attribute("w:default")).map(|v| v == "1" || v == "true").unwrap_or(false);
                    let is_default_paragraph = style_type == "paragraph" && is_default;

                    let name = style.children().find(|n| n.has_tag_name("name"))
                        .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                        .map(|s| s.to_string());

                    let mut font_ascii = None;
                    let mut font_east_asia = None;
                    let mut size_pt = None;
                    let mut color = None;
                    let mut bold = None;
                    let mut italic = None;
                    let mut underline = None;
                    let mut strike = None;

                    if let Some(r_pr) = style.children().find(|n| n.has_tag_name("rPr")) {
                        if let Some(rf) = r_pr.children().find(|n| n.has_tag_name("rFonts")) {
                            font_ascii = rf.attribute((W_NS, "ascii")).or_else(|| rf.attribute("w:ascii")).map(|s| s.to_string());
                            font_east_asia = rf.attribute((W_NS, "eastAsia")).or_else(|| rf.attribute("w:eastAsia")).map(|s| s.to_string());
                        }
                        if let Some(sz) = r_pr.children().find(|n| n.has_tag_name("sz")) {
                            if let Some(val) = sz.attribute((W_NS, "val")).or_else(|| sz.attribute("w:val")) {
                                if let Ok(half_pt) = val.parse::<f64>() {
                                    size_pt = Some(half_pt / 2.0);
                                }
                            }
                        }
                        if let Some(color_el) = r_pr.children().find(|n| n.has_tag_name("color")) {
                            if let Some(val) = color_el.attribute((W_NS, "val")).or_else(|| color_el.attribute("w:val")) {
                                if val != "auto" && !val.is_empty() {
                                    color = Some(if val.starts_with('#') { val.to_string() } else { format!("#{}", val) });
                                }
                            }
                        }
                        bold = Some(r_pr.children().any(|n| n.has_tag_name("b")));
                        italic = Some(r_pr.children().any(|n| n.has_tag_name("i")));
                        underline = Some(r_pr.children().any(|n| n.has_tag_name("u")));
                        strike = Some(r_pr.children().any(|n| n.has_tag_name("strike")));
                    }

                    let mut spacing_before_pt = None;
                    let mut spacing_after_pt = None;
                    let mut line_spacing_mult = None;
                    let mut line_spacing_exact = None;
                    let mut alignment = None;
                    let mut shading_fill = None;
                    let mut num_id = None;
                    let mut ilvl = None;

                    if let Some(p_pr) = style.children().find(|n| n.has_tag_name("pPr")) {
                        if let Some(num_pr) = p_pr.children().find(|n| n.has_tag_name("numPr")) {
                            num_id = num_pr.children().find(|n| n.has_tag_name("numId"))
                                .and_then(|n| get_node_attr(&n, "val"))
                                .map(|s| s.to_string());
                            ilvl = num_pr.children().find(|n| n.has_tag_name("ilvl"))
                                .and_then(|n| get_node_attr(&n, "val"))
                                .and_then(|s| s.parse::<usize>().ok());
                        }
                        if let Some(spacing) = p_pr.children().find(|n| n.has_tag_name("spacing")) {
                            if let Some(before) = spacing.attribute((W_NS, "before")).or_else(|| spacing.attribute("w:before")) {
                                if let Ok(twips) = before.parse::<f64>() {
                                    spacing_before_pt = Some(twips / 20.0);
                                }
                            }
                            if let Some(after) = spacing.attribute((W_NS, "after")).or_else(|| spacing.attribute("w:after")) {
                                if let Ok(twips) = after.parse::<f64>() {
                                    spacing_after_pt = Some(twips / 20.0);
                                }
                            }
                            if let Some(line) = spacing.attribute((W_NS, "line")).or_else(|| spacing.attribute("w:line")) {
                                if let Ok(twips) = line.parse::<f64>() {
                                    let rule = spacing.attribute((W_NS, "lineRule")).or_else(|| spacing.attribute("w:lineRule")).unwrap_or("auto");
                                    if rule == "auto" {
                                        line_spacing_mult = Some(twips / 240.0);
                                    } else {
                                        line_spacing_exact = Some(twips / 20.0);
                                    }
                                }
                            }
                        }
                        if let Some(jc) = p_pr.children().find(|n| n.has_tag_name("jc")) {
                            if let Some(val) = jc.attribute((W_NS, "val")).or_else(|| jc.attribute("w:val")) {
                                alignment = Some(match val {
                                    "center" => "center".to_string(),
                                    "right" | "end" => "right".to_string(),
                                    "both" | "distribute" => "justify".to_string(),
                                    _ => "left".to_string(),
                                });
                            }
                        }
                        if let Some(shd) = p_pr.children().find(|n| n.has_tag_name("shd")) {
                            if let Some(fill) = shd.attribute((W_NS, "fill")).or_else(|| shd.attribute("w:fill")) {
                                if fill != "auto" && !fill.is_empty() {
                                    shading_fill = Some(if fill.starts_with('#') { fill.to_string() } else { format!("#{}", fill) });
                                }
                            }
                        }
                    }

                    styles_map.insert(style_id.to_string(), DocxStyle {
                        style_id: style_id.to_string(),
                        based_on,
                        is_default_paragraph,
                        font_ascii,
                        font_east_asia,
                        size_pt,
                        color,
                        bold,
                        italic,
                        underline,
                        strike,
                        spacing_before_pt,
                        spacing_after_pt,
                        line_spacing_mult,
                        line_spacing_exact,
                        alignment,
                        shading_fill,
                        num_id,
                        ilvl,
                        name,
                    });
                }
            }
        }
    }

    (styles_map, doc_defaults)
}

fn parse_numbering(
    package: &OxmlPackage,
) -> (
    HashMap<String, HashMap<String, NumLevel>>,
    HashMap<String, (String, HashMap<String, usize>)>,
) {
    let mut abstract_nums = HashMap::new();
    let mut num_instances = HashMap::new();

    if let Ok(num_xml) = package.read_part_xml("word/numbering.xml") {
        if let Ok(num_doc) = roxmltree::Document::parse(&num_xml) {
            for abs in num_doc.descendants().filter(|n| n.has_tag_name("abstractNum")) {
                if let Some(abs_id) = get_node_attr(&abs, "abstractNumId") {
                    let mut levels = HashMap::new();
                    for lvl in abs.children().filter(|n| n.has_tag_name("lvl")) {
                        if let Some(ilvl) = get_node_attr(&lvl, "ilvl") {
                            let mut num_fmt = "decimal".to_string();
                            let mut lvl_text = String::new();
                            let mut left_pt = 0.0;
                            let mut hanging_pt = 0.0;
                            let mut start = 1;
                            let mut jc = "left".to_string();
                            let mut font_name = None;
                            let mut font_size_pt = None;
                            let mut color = None;
                            let mut bold = false;
                            let mut italic = false;

                            if let Some(fmt_el) = lvl.children().find(|n| n.has_tag_name("numFmt")) {
                                if let Some(val) = get_node_attr(&fmt_el, "val") {
                                    num_fmt = val.to_string();
                                }
                            }
                            if let Some(txt_el) = lvl.children().find(|n| n.has_tag_name("lvlText")) {
                                if let Some(val) = get_node_attr(&txt_el, "val") {
                                    lvl_text = val.to_string();
                                }
                            }
                            if let Some(start_el) = lvl.children().find(|n| n.has_tag_name("start")) {
                                if let Some(val) = get_node_attr(&start_el, "val") {
                                    if let Ok(v) = val.parse::<usize>() {
                                        start = v;
                                    }
                                }
                            }
                            if let Some(jc_el) = lvl.children().find(|n| n.has_tag_name("lvlJc")) {
                                if let Some(val) = get_node_attr(&jc_el, "val") {
                                    jc = val.to_string();
                                }
                            }

                            if let Some(p_pr) = lvl.children().find(|n| n.has_tag_name("pPr")) {
                                if let Some(ind) = p_pr.children().find(|n| n.has_tag_name("ind")) {
                                    if let Some(l) = get_node_attr(&ind, "left") {
                                        if let Ok(twips) = l.parse::<f64>() {
                                            left_pt = twips / 20.0;
                                        }
                                    }
                                    if let Some(h) = get_node_attr(&ind, "hanging") {
                                        if let Ok(twips) = h.parse::<f64>() {
                                            hanging_pt = twips / 20.0;
                                        }
                                    }
                                }
                            }

                            if let Some(r_pr) = lvl.children().find(|n| n.has_tag_name("rPr")) {
                                if let Some(rf) = r_pr.children().find(|n| n.has_tag_name("rFonts")) {
                                    font_name = get_node_attr(&rf, "ascii").map(|s| s.to_string());
                                }
                                if let Some(sz) = r_pr.children().find(|n| n.has_tag_name("sz")) {
                                    if let Some(val) = get_node_attr(&sz, "val") {
                                        if let Ok(half_pt) = val.parse::<f64>() {
                                            font_size_pt = Some(half_pt / 2.0);
                                        }
                                    }
                                }
                                if let Some(color_el) = r_pr.children().find(|n| n.has_tag_name("color")) {
                                    if let Some(val) = get_node_attr(&color_el, "val") {
                                        if val != "auto" && !val.is_empty() {
                                            color = Some(if val.starts_with('#') { val.to_string() } else { format!("#{}", val) });
                                        }
                                    }
                                }
                                bold = r_pr.children().any(|n| n.has_tag_name("b"));
                                italic = r_pr.children().any(|n| n.has_tag_name("i"));
                            }

                            levels.insert(ilvl.to_string(), NumLevel {
                                num_fmt,
                                lvl_text,
                                left_pt,
                                hanging_pt,
                                start,
                                jc,
                                font_name,
                                font_size_pt,
                                color,
                                bold,
                                italic,
                            });
                        }
                    }
                    abstract_nums.insert(abs_id.to_string(), levels);
                }
            }

            for num in num_doc.descendants().filter(|n| n.has_tag_name("num")) {
                if let Some(num_id) = get_node_attr(&num, "numId") {
                    if let Some(abs_ref) = num.children().find(|n| n.has_tag_name("abstractNumId")) {
                        if let Some(abs_val) = get_node_attr(&abs_ref, "val") {
                            let mut start_overrides = HashMap::new();
                            for ovr in num.children().filter(|n| n.has_tag_name("lvlOverride")) {
                                if let Some(ilvl) = get_node_attr(&ovr, "ilvl") {
                                    if let Some(so) = ovr.children().find(|n| n.has_tag_name("startOverride")) {
                                        if let Some(val) = get_node_attr(&so, "val") {
                                            if let Ok(v) = val.parse::<usize>() {
                                                start_overrides.insert(ilvl.to_string(), v);
                                            }
                                        }
                                    }
                                }
                            }
                            num_instances.insert(num_id.to_string(), (abs_val.to_string(), start_overrides));
                        }
                    }
                }
            }
        }
    }

    (abstract_nums, num_instances)
}

fn get_page_layout_for(sect_pr: Option<&roxmltree::Node>) -> PageLayout {
    let mut layout = PageLayout {
        width_pt: 612.0,
        height_pt: 792.0,
        margin_top_pt: 72.0,
        margin_bottom_pt: 72.0,
        margin_left_pt: 72.0,
        margin_right_pt: 72.0,
        header_distance_pt: 42.55,
        footer_distance_pt: 49.6,
    };

    if let Some(sect) = sect_pr {
        if let Some(sz) = sect.children().find(|n| n.has_tag_name("pgSz")) {
            if let Some(w) = sz.attribute((W_NS, "w")).or_else(|| sz.attribute("w:w")).and_then(|s| s.parse::<f64>().ok()) {
                layout.width_pt = w / 20.0;
            }
            if let Some(h) = sz.attribute((W_NS, "h")).or_else(|| sz.attribute("w:h")).and_then(|s| s.parse::<f64>().ok()) {
                layout.height_pt = h / 20.0;
            }
        }
        if let Some(mar) = sect.children().find(|n| n.has_tag_name("pgMar")) {
            if let Some(t) = mar.attribute((W_NS, "top")).or_else(|| mar.attribute("w:top")).and_then(|s| s.parse::<f64>().ok()) {
                layout.margin_top_pt = t / 20.0;
            }
            if let Some(b) = mar.attribute((W_NS, "bottom")).or_else(|| mar.attribute("w:bottom")).and_then(|s| s.parse::<f64>().ok()) {
                layout.margin_bottom_pt = b / 20.0;
            }
            if let Some(l) = mar.attribute((W_NS, "left")).or_else(|| mar.attribute("w:left")).and_then(|s| s.parse::<f64>().ok()) {
                layout.margin_left_pt = l / 20.0;
            }
            if let Some(r) = mar.attribute((W_NS, "right")).or_else(|| mar.attribute("w:right")).and_then(|s| s.parse::<f64>().ok()) {
                layout.margin_right_pt = r / 20.0;
            }
            if let Some(hd) = mar.attribute((W_NS, "header")).or_else(|| mar.attribute("w:header")).and_then(|s| s.parse::<f64>().ok()) {
                layout.header_distance_pt = hd / 20.0;
            }
            if let Some(fd) = mar.attribute((W_NS, "footer")).or_else(|| mar.attribute("w:footer")).and_then(|s| s.parse::<f64>().ok()) {
                layout.footer_distance_pt = fd / 20.0;
            }
        }
    }
    layout
}

fn collect_sections<'a, 'input>(
    body_node: &roxmltree::Node<'a, 'input>,
) -> Vec<roxmltree::Node<'a, 'input>> {
    let mut list = Vec::new();
    for p in body_node.children().filter(|n| n.has_tag_name("p")) {
        if let Some(p_pr) = p.children().find(|n| n.has_tag_name("pPr")) {
            if let Some(sect) = p_pr.children().find(|n| n.has_tag_name("sectPr")) {
                list.push(sect);
            }
        }
    }
    if let Some(trailing) = body_node.children().find(|n| n.has_tag_name("sectPr")) {
        list.push(trailing);
    }
    list
}

fn build_section_hf_bundles(
    sections: &[roxmltree::Node],
    doc_rels: &oxml::rels::Relationships,
    package: &OxmlPackage,
    styles: &HashMap<String, DocxStyle>,
    doc_defaults: &DocDefaults,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
    is_header: bool,
) -> HashMap<usize, HeaderFooterBundle> {
    let mut result = HashMap::new();
    let ref_tag = if is_header { "headerReference" } else { "footerReference" };
    let div_class = if is_header { "doc-header" } else { "doc-footer" };

    for (i, sect) in sections.iter().enumerate() {
        let mut first = None;
        let mut default = None;
        let mut even = None;

        for ref_node in sect.children().filter(|n| n.has_tag_name(ref_tag)) {
            let r_id = ref_node.attribute((W_NS, "id"))
                .or_else(|| ref_node.attribute("r:id"))
                .unwrap_or("");
            let type_attr = ref_node.attribute((W_NS, "type"))
                .or_else(|| ref_node.attribute("w:type"))
                .unwrap_or("default");

            if !r_id.is_empty() {
                if let Some(rel) = doc_rels.get(r_id) {
                    let target = &rel.target;
                    let part_path = if target.starts_with("word/") {
                        target.to_string()
                    } else {
                        format!("word/{}", target)
                    };

                    if let Ok(part_xml) = package.read_part_xml(&part_path) {
                        if let Ok(part_doc) = roxmltree::Document::parse(&part_xml) {
                            let mut html = format!("<div class=\"{}\">", div_class);
                            let root = part_doc.root_element();
                            let mut num_counters = HashMap::new();
                            let mut temp_para_count = 0;
                            let mut temp_table_count = 0;
                            for child in root.children().filter(|n| n.is_element()) {
                                let tag = child.tag_name().name();
                                if tag == "p" {
                                    render_paragraph(
                                        &child,
                                        "",
                                        &mut html,
                                        styles,
                                        doc_defaults,
                                        num_maps,
                                        &mut num_counters,
                                        doc_rels,
                                        package,
                                        &mut Vec::new(),
                                        &mut None,
                                        &mut false,
                                        &mut HashMap::new(),
                                        &mut HashMap::new(),
                                        &mut HashMap::new(),
                                        &mut HashMap::new(),
                                        &mut temp_para_count,
                                        &mut temp_table_count,
                                        &mut None,
                                        &mut 0,
                                        &mut HashMap::new(),
                                        &HashMap::new(),
                                        &HashMap::new(),
                                    );
                                } else if tag == "tbl" {
                                    render_table(
                                        &child,
                                        "",
                                        &mut html,
                                        styles,
                                        doc_defaults,
                                        doc_rels,
                                        package,
                                    );
                                }
                            }
                            html.push_str("</div>");
                            
                            match type_attr {
                                "first" => first = Some(html),
                                "even" => even = Some(html),
                                _ => default = Some(html),
                            }
                        }
                    }
                }
            }
        }
        result.insert(i, HeaderFooterBundle { first, default, even });
    }
    result
}

fn pick_header_footer(
    bundles: &HashMap<usize, HeaderFooterBundle>,
    sections: &[roxmltree::Node],
    section_idx: usize,
    is_first_page_of_section: bool,
    page_is_even: bool,
    even_and_odd_global: bool,
    fallback_html: &str,
) -> String {
    if let Some(bundle) = bundles.get(&section_idx) {
        let sect_has_title_pg = sections.get(section_idx)
            .and_then(|s| s.children().find(|n| n.has_tag_name("titlePage")))
            .is_some();

        if is_first_page_of_section && sect_has_title_pg {
            return bundle.first.clone().unwrap_or_default();
        }
        if even_and_odd_global && page_is_even && bundle.even.is_some() {
            return bundle.even.clone().unwrap();
        }
        return bundle.default.clone().unwrap_or_else(|| fallback_html.to_string());
    }
    fallback_html.to_string()
}

fn find_last_sect_index(pc: &str) -> Option<usize> {
    if let Some(idx) = pc.rfind("<!--SECT:") {
        let sub = &pc[idx + "<!--SECT:".len()..];
        if let Some(end_idx) = sub.find("-->") {
            if let Ok(val) = sub[..end_idx].parse::<usize>() {
                return Some(val);
            }
        }
    }
    None
}

fn remove_sect_markers(pc: &str) -> String {
    let mut result = String::new();
    let mut start = 0;
    while let Some(idx) = pc[start..].find("<!--SECT:") {
        result.push_str(&pc[start..start + idx]);
        let sub = &pc[start + idx..];
        if let Some(end_idx) = sub.find("-->") {
            start = start + idx + end_idx + "-->".len();
        } else {
            start = start + idx + "<!--SECT:".len();
        }
    }
    result.push_str(&pc[start..]);
    result
}

/// Render the Word document as HTML for browser preview.
pub fn view_as_html(package: &OxmlPackage) -> Result<String, HandlerError> {
    let doc_xml = package.read_part_xml("word/document.xml").map_err(|e| {
        HandlerError::OperationFailed(format!("Failed to read word/document.xml: {}", e))
    })?;
    let doc = roxmltree::Document::parse(&doc_xml).map_err(|e| {
        HandlerError::OperationFailed(format!("XML parse error in document.xml: {}", e))
    })?;

    // 1. Parse Styles
    let (styles, doc_defaults) = parse_styles(package);

    // 2. Parse Numbering formats
    let (abstract_nums, num_instances) = parse_numbering(package);
    let mut num_maps = HashMap::new();
    let mut num_to_abs_map = HashMap::new();
    let mut num_start_overrides = HashMap::new();
    for (num_id, (abs_val, overrides)) in num_instances {
        num_to_abs_map.insert(num_id.clone(), abs_val.clone());
        let mut int_overrides = HashMap::new();
        for (ilvl_str, start_val) in &overrides {
            if let Ok(ilvl_idx) = ilvl_str.parse::<usize>() {
                int_overrides.insert(ilvl_idx, *start_val);
            }
        }
        num_start_overrides.insert(num_id.clone(), int_overrides);

        if let Some(levels) = abstract_nums.get(&abs_val) {
            let mut mapped_levels = levels.clone();
            for (ilvl, start_override) in overrides {
                if let Some(lvl) = mapped_levels.get_mut(&ilvl) {
                    lvl.start = start_override;
                }
            }
            num_maps.insert(num_id, mapped_levels);
        }
    }

    // 3. Document Relationships
    let doc_rels = package
        .part_rels("word/document.xml")
        .unwrap_or_else(|_| oxml::rels::Relationships::empty());

    let body_node = doc
        .descendants()
        .find(|n| n.has_tag_name("body"))
        .ok_or_else(|| HandlerError::OperationFailed("body element not found".to_string()))?;

    let sections = collect_sections(&body_node);
    let first_sect = sections.first();
    let page_layout = get_page_layout_for(first_sect);

    let has_math = doc_xml.contains("<m:oMath") || doc_xml.contains("<oMath");
    
    // Header/Footer pre-rendering
    let section_headers = build_section_hf_bundles(
        &sections, &doc_rels, package, &styles, &doc_defaults, &num_maps, true
    );
    let section_footers = build_section_hf_bundles(
        &sections, &doc_rels, package, &styles, &doc_defaults, &num_maps, false
    );

    let even_and_odd_global = package.read_part_xml("word/settings.xml")
        .map(|xml| xml.contains("<w:evenAndOddHeaders"))
        .unwrap_or(false);

    let mut body_html = String::new();
    let mut list_stack: Vec<String> = Vec::new();
    let mut current_list_type: Option<String> = None;
    let mut pending_li_close = false;
    let mut num_id_level_offset = HashMap::new();
    let mut ol_count_per_level = HashMap::new();
    let mut multi_level_counters = HashMap::new();
    let mut heading_counters = HashMap::new();
    let mut current_num_id: Option<String> = None;
    let mut current_list_level: usize = 0;
    let mut abs_num_level_counters: HashMap<String, HashMap<usize, usize>> = HashMap::new();
    
    let mut w_para_count = 0;
    let mut w_table_count = 0;
    
    let mut current_section_idx = 0;
    body_html.push_str(&format!("<!--SECT:{}-->", current_section_idx));

    let elements: Vec<roxmltree::Node> = body_node.children().filter(|n| n.is_element()).collect();

    for (ei, element) in elements.iter().enumerate() {
        let tag = element.tag_name().name();

        if tag == "sectPr" {
            continue;
        }

        if tag == "bookmarkStart" {
            if let Some(name) = element.attribute((W_NS, "name")).or_else(|| element.attribute("w:name")) {
                if name != "_GoBack" {
                    body_html.push_str(&format!("<a id=\"{}\"></a>", html_escape(name)));
                }
            }
            continue;
        }

        if ei > 0 {
            let prev_el = &elements[ei - 1];
            if prev_el.tag_name().name() == "p" {
                if let Some(p_pr) = prev_el.children().find(|n| n.has_tag_name("pPr")) {
                    if let Some(inline_sect) = p_pr.children().find(|n| n.has_tag_name("sectPr")) {
                        let sect_type = inline_sect.children().find(|n| n.has_tag_name("type"))
                            .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                            .unwrap_or("nextPage");
                        if sect_type == "nextPage" || sect_type == "evenPage" || sect_type == "oddPage" {
                            body_html.push_str("<!--PAGE_BREAK-->");
                        }
                        current_section_idx += 1;
                        body_html.push_str(&format!("<!--SECT:{}-->", current_section_idx));
                    }
                }
            }
        }

        if tag == "p" {
            w_para_count += 1;
            let p_pr = element.children().find(|n| n.has_tag_name("pPr"));
            let pg_bb = p_pr.as_ref()
                .and_then(|p| p.children().find(|n| n.has_tag_name("pageBreakBefore")))
                .map(|p| p.attribute((W_NS, "val")).or_else(|| p.attribute("w:val")).unwrap_or("true") != "false")
                .unwrap_or(false);
            if pg_bb {
                body_html.push_str("<!--PAGE_BREAK-->");
            }

            render_paragraph(
                element,
                &format!("/body/p[{}]", w_para_count),
                &mut body_html,
                &styles,
                &doc_defaults,
                &num_maps,
                &mut HashMap::new(),
                &doc_rels,
                package,
                &mut list_stack,
                &mut current_list_type,
                &mut pending_li_close,
                &mut num_id_level_offset,
                &mut ol_count_per_level,
                &mut multi_level_counters,
                &mut heading_counters,
                &mut w_para_count,
                &mut w_table_count,
                &mut current_num_id,
                &mut current_list_level,
                &mut abs_num_level_counters,
                &num_to_abs_map,
                &num_start_overrides,
            );
        } else if tag == "tbl" {
            w_table_count += 1;
            close_all_lists(&mut body_html, &mut list_stack, &mut current_list_type, &mut pending_li_close);
            render_table(
                element,
                &format!("/body/table[{}]", w_table_count),
                &mut body_html,
                &styles,
                &doc_defaults,
                &doc_rels,
                package,
            );
        }
    }

    close_all_lists(&mut body_html, &mut list_stack, &mut current_list_type, &mut pending_li_close);

    let pages: Vec<&str> = body_html.split("<!--PAGE_BREAK-->").collect();
    let mut page_list = Vec::new();
    for (i, p_content) in pages.iter().enumerate() {
        let pc = p_content.trim();
        if pc.is_empty() && i == pages.len() - 1 {
            continue;
        }
        page_list.push(pc.to_string());
    }

    let mut html_pages = String::new();
    let mut active_layout = get_page_layout_for(first_sect);
    let mut active_section_idx = 0;
    let mut prev_active_section_idx = -1;

    for (i, mut pg_content) in page_list.into_iter().enumerate() {
        if let Some(idx) = find_last_sect_index(&pg_content) {
            if idx < sections.len() {
                active_layout = get_page_layout_for(Some(&sections[idx]));
                active_section_idx = idx;
            }
        }
        pg_content = remove_sect_markers(&pg_content);

        let is_first_page_of_section = active_section_idx != prev_active_section_idx as usize;
        prev_active_section_idx = active_section_idx as i32;

        let page_is_even = (i + 1) % 2 == 0;
        let page_style = format!(
            "width:{:.1}pt; min-height:{:.1}pt; padding:{:.1}pt {:.1}pt {:.1}pt {:.1}pt;",
            active_layout.width_pt,
            active_layout.height_pt,
            active_layout.margin_top_pt,
            active_layout.margin_right_pt,
            active_layout.margin_bottom_pt,
            active_layout.margin_left_pt
        );

        let per_page_header = pick_header_footer(
            &section_headers,
            &sections,
            active_section_idx,
            is_first_page_of_section,
            page_is_even,
            even_and_odd_global,
            "",
        );

        let per_page_footer = pick_header_footer(
            &section_footers,
            &sections,
            active_section_idx,
            is_first_page_of_section,
            page_is_even,
            even_and_odd_global,
            "",
        );

        let page_num_str = (i + 1).to_string();
        let num_pages_str = pages.len().to_string();

        let header_html = if per_page_header.is_empty() {
            "".to_string()
        } else {
            per_page_header
                .replace("<!--PAGE_NUM-->", &page_num_str)
                .replace("<!--NUM_PAGES-->", &num_pages_str)
        };

        let footer_html = if per_page_footer.is_empty() {
            "".to_string()
        } else {
            per_page_footer
                .replace("<!--PAGE_NUM-->", &page_num_str)
                .replace("<!--NUM_PAGES-->", &num_pages_str)
        };

        html_pages.push_str(&format!(
            r#"<div class="page-wrapper" data-section="{}" data-section-idx="{}">
  <div class="page" data-page="{}" style="{}">
    {}
    <div class="page-body">
      {}
    </div>
    {}
  </div>
</div>
"#,
            i + 1,
            active_section_idx,
            i + 1,
            page_style,
            header_html,
            pg_content,
            footer_html
        ));
    }

    let mut head_injections = String::new();
    if has_math {
        head_injections.push_str("<link rel=\"stylesheet\" href=\"https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.css\" media=\"print\" onload=\"this.media='all'\" onerror=\"this.remove()\">\n");
        head_injections.push_str("<script defer src=\"https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.js\" onerror=\"document.querySelectorAll('.katex-formula').forEach(function(el){el.textContent=el.dataset.formula;el.style.fontFamily='monospace';el.style.color='#666'})\"></script>\n");
    }

    let font_fallback = match doc_defaults.font.to_lowercase().as_str() {
        "calibri" | "arial" => ", -apple-system, sans-serif".to_string(),
        "times new roman" => ", Georgia, serif".to_string(),
        _ => ", 'Songti SC', 'STSong', sans-serif".to_string(),
    };
    let font_css = format!("'{}'{}", doc_defaults.font, font_fallback);

    let body_height_pt = page_layout.height_pt - page_layout.margin_top_pt - page_layout.margin_bottom_pt;
    let paginate_js = format!(
        r#"
function _wordInit(){{
  if(typeof katex!=='undefined'){{
    document.querySelectorAll('.katex-formula:not(.katex-rendered)').forEach(function(el){{
      try{{katex.render(el.dataset.formula,el,{{throwOnError:false,displayMode:!!el.dataset.display}});}}catch(e){{el.textContent=el.dataset.formula+' (Error: '+e.message+')';}}
      el.classList.add('katex-rendered');
    }});
  }}else{{
    document.querySelectorAll('.katex-formula:not(.katex-rendered)').forEach(function(el){{el.textContent=el.dataset.formula;el.style.fontFamily='monospace';el.style.color='#666';}});
  }}
  // CJK punctuation compression
  (function(){{
    var re=/([\u3000-\u303F\uFF01-\uFF60\uFE30-\uFE4F\u2014\u2015\u2026\u2018\u2019\u201C\u201D])/;
    document.querySelectorAll('.page-body').forEach(function(body){{
      var w=document.createTreeWalker(body,NodeFilter.SHOW_TEXT);
      var nodes=[];while(w.nextNode())nodes.push(w.currentNode);
      nodes.forEach(function(nd){{
        if(!re.test(nd.textContent))return;
        var parts=nd.textContent.split(re);
        if(parts.length<=1)return;
        var frag=document.createDocumentFragment();
        for(var i=0;i<parts.length;i++){{
          if(!parts[i])continue;
          if(re.test(parts[i])){{
            var sp=document.createElement('span');
            sp.textContent=parts[i];
            sp.style.marginRight='-0.2em';
            frag.appendChild(sp);
          }}else frag.appendChild(document.createTextNode(parts[i]));
        }}
        nd.parentNode.replaceChild(frag,nd);
      }});
    }});
  }})();
  
  var maxBodyH = {:.1} * 96 / 72;
  var ftpl = "";
  var htpl = "";
  
  function paginate(){{
    var pages=document.querySelectorAll('.page');
    var loopLim=pages.length;
    for(var pi=0;pi<loopLim;pi++){{
      var page=pages[pi];
      var body=page.querySelector('.page-body');
      if(!body)continue;
      var fnEl=body.querySelector('.footnotes');
      var fnH=fnEl?fnEl.offsetHeight:0;
      var availH=maxBodyH-fnH;
      var contentH=0;
      Array.from(body.children).forEach(function(c){{
        if(c.classList.contains('footnotes'))return;
        var b=c.offsetTop+c.offsetHeight-body.offsetTop;
        if(b>contentH)contentH=b;
      }});
      if(contentH<=availH+2)continue;
      
      var children=Array.from(body.children);
      var splitIdx=-1;
      for(var ci=0;ci<children.length;ci++){{
        if(children[ci].classList.contains('footnotes'))continue;
        var bot=children[ci].offsetTop+children[ci].offsetHeight-body.offsetTop;
        if(bot>availH+2){{splitIdx=ci;break;}}
      }}
      if(splitIdx<0)continue;
      
      var firstOverflow=children[splitIdx];
      if(firstOverflow&&firstOverflow.tagName==='TABLE'){{
        var table=firstOverflow;
        var tableTop=table.offsetTop-body.offsetTop;
        var trs=Array.from(table.querySelectorAll('tr')).filter(function(tr){{
          return tr.closest('table')===table;
        }});
        var rowSplit=-1;
        for(var ri=0;ri<trs.length;ri++){{
          var rowBot=trs[ri].offsetTop+trs[ri].offsetHeight-body.offsetTop;
          if(rowBot>availH){{rowSplit=ri;break;}}
        }}
        if(rowSplit>0){{
          var cont=table.cloneNode(false);
          var tbodies=table.querySelectorAll('tbody');
          var contBody=tbodies.length?document.createElement('tbody'):cont;
          if(tbodies.length)cont.appendChild(contBody);
          for(var rj=rowSplit;rj<trs.length;rj++){{
            contBody.appendChild(trs[rj]);
          }}
          table.parentNode.insertBefore(cont,table.nextSibling);
          children=Array.from(body.children);
          splitIdx=children.indexOf(cont);
        }}
      }}
      
      if(splitIdx===0)splitIdx=1;
      
      var toMove=[];
      for(var mi=splitIdx;mi<children.length;mi++){{
        if(!children[mi].classList.contains('footnotes'))toMove.push(children[mi]);
      }}
      if(toMove.length===0)continue;
      
      var nw=document.createElement('div');
      nw.className='page-wrapper';
      var np=document.createElement('div');
      np.className='page';
      np.style.cssText=page.style.cssText;
      var nb=document.createElement('div');
      nb.className='page-body page-body-cont';
      for(var mi=0;mi<toMove.length;mi++){{
        nb.appendChild(toMove[mi]);
      }}
      np.appendChild(nb);
      nw.appendChild(np);
      var parentWrapper=page.closest('.page-wrapper');
      if(parentWrapper)parentWrapper.after(nw);
      else page.after(nw);
    }}
    
    // Renumber pages
    var allPages=document.querySelectorAll('.page');
    allPages.forEach(function(p,i){{
      p.querySelectorAll('.page-num-field').forEach(function(s){{s.textContent=(i+1);}});
      p.querySelectorAll('.num-pages-field').forEach(function(s){{s.textContent=allPages.length;}});
    }});
    
    var again=false;
    var rcAll=document.querySelectorAll('.page');
    for(var rci=0;rci<rcAll.length;rci++){{
      var p=rcAll[rci];
      var b=p.querySelector('.page-body');
      if(!b)continue;
      var f=b.querySelector('.footnotes');
      var fh=f?f.offsetHeight:0;
      var ch=0;
      var visibleCount=0;
      Array.from(b.children).forEach(function(c){{
        if(c.classList.contains('footnotes'))return;
        var bt=c.offsetTop+c.offsetHeight-b.offsetTop;
        if(bt>ch)ch=bt;
        if(c.offsetHeight>0)visibleCount++;
      }});
      if(ch>maxBodyH-fh+2 && visibleCount>1){{again=true;break;}}
    }}
    if(again){{setTimeout(paginate,0);}}
    else{{
      setTimeout(positionFootnotes,0);
      setTimeout(scalePages,0);
    }}
  }}
  
  function positionFootnotes(){{
    document.querySelectorAll('.page').forEach(function(page){{
      var body=page.querySelector('.page-body');
      if(!body)return;
      var fn=body.querySelector('.footnotes');
      if(!fn)return;
      var lastBot=0;
      Array.from(body.children).forEach(function(c){{
        if(c===fn)return;
        var b=c.offsetTop+c.offsetHeight-body.offsetTop;
        if(b>lastBot)lastBot=b;
      }});
      var gap=maxBodyH-lastBot-fn.offsetHeight;
      if(gap>0)fn.style.marginTop=gap+'px';
    }});
  }}
  
  function scalePages(){{
    var bs=getComputedStyle(document.body);
    var availW=document.body.clientWidth-parseFloat(bs.paddingLeft)-parseFloat(bs.paddingRight);
    document.querySelectorAll('.page-wrapper').forEach(function(wrapper){{
      var page=wrapper.querySelector('.page');
      if(!page||page.style.display==='none')return;
      var pageW=page.offsetWidth;
      var pageH=page.offsetHeight;
      var s=Math.min(availW/pageW,1);
      page.style.transform='scale('+s+')';
      wrapper.style.height=(pageH*s)+'px';
      wrapper.style.width=(pageW*s)+'px';
    }});
  }}
  
  window.addEventListener('resize', scalePages);
  setTimeout(paginate, 100);
}}
if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',_wordInit);
else _wordInit();
"#,
        body_height_pt
    );

    let marker_css = build_list_marker_css(&body_node, &styles, &num_maps);

    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Word Preview</title>
{head_injections}<style>
{marker_css}
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
body {{
    font-family: {font_css};
    background: #f0f2f5;
    padding: 30px 10px;
    display: flex;
    flex-direction: column;
    align-items: center;
    color: {color};
}}
.page-container {{
    display: flex;
    flex-direction: column;
    gap: 20px;
    width: 100%;
    align-items: center;
}}
.page-wrapper {{
    margin: 0 auto 20px;
    transition: width 0.15s ease, height 0.15s ease;
}}
.page {{
    background: white;
    box-shadow: 0 4px 20px rgba(0,0,0,0.15);
    box-sizing: border-box;
    position: relative;
    overflow-x: auto;
    display: flex;
    flex-direction: column;
    transform-origin: left top;
    transition: transform 0.15s ease;
}}
.page-body {{
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow-wrap: anywhere;
}}
.page-body-cont > :first-child {{
    margin-top: 0 !important;
}}
.doc-header, .doc-footer {{
    font-size: {sz}pt;
}}
.doc-header {{
    position: absolute;
    top: {header_dist}pt;
    left: {margin_left}pt;
    right: {margin_right}pt;
    padding-bottom: 0.3em;
}}
.doc-footer {{
    position: absolute;
    bottom: {footer_dist}pt;
    left: {margin_left}pt;
    right: {margin_right}pt;
    padding-top: 0.3em;
}}
p, h1, h2, h3, h4, h5, h6 {{
    margin: 0;
    margin-bottom: {space_after}pt;
    line-height: {lh};
    word-wrap: break-word;
}}
table {{
    border-collapse: collapse;
    font-size: {sz}pt;
    width: 100%;
    margin-bottom: 12px;
}}
td, th {{
    border: none;
    padding: 0 5.4pt;
    vertical-align: top;
    text-align: inherit;
}}
th {{
    font-weight: 600;
}}
a {{
    color: #1a73e8;
    text-decoration: none;
}}
a:hover {{
    text-decoration: underline;
}}
.equation {{
    text-align: center;
    padding: 0.5em 0;
    overflow-x: auto;
}}
</style>
</head>
<body>
<div class="page-container">
  {}
</div>
<script>
{}
</script>
</body>
</html>"#,
        html_pages,
        paginate_js,
        head_injections = head_injections,
        font_css = font_css,
        color = doc_defaults.color,
        sz = doc_defaults.size_pt,
        space_after = doc_defaults.space_after_pt,
        lh = doc_defaults.line_height,
        marker_css = marker_css,
        header_dist = page_layout.header_distance_pt,
        footer_dist = page_layout.footer_distance_pt,
        margin_left = page_layout.margin_left_pt,
        margin_right = page_layout.margin_right_pt
    ))
}

fn resolve_run_background(r_pr: &roxmltree::Node) -> Option<String> {
    if let Some(hl) = r_pr.children().find(|n| n.has_tag_name("highlight")) {
        if let Some(val) = hl.attribute("val").or_else(|| hl.attribute((W_NS, "val"))).or_else(|| hl.attribute("w:val")) {
            let hl_color = match val.to_lowercase().as_str() {
                "yellow" => "#FFFF00",
                "green" => "#00FF00",
                "cyan" => "#00FFFF",
                "magenta" => "#FF00FF",
                "blue" => "#0000FF",
                "red" => "#FF0000",
                "darkblue" => "#00008B",
                "darkcyan" => "#008B8B",
                "darkgreen" => "#006400",
                "darkmagenta" => "#8B008B",
                "darkred" => "#8B0000",
                "darkyellow" => "#808000",
                "darkgray" => "#A9A9A9",
                "lightgray" => "#D3D3D3",
                "black" => "#000000",
                "white" => "#FFFFFF",
                _ => "",
            };
            if !hl_color.is_empty() {
                return Some(hl_color.to_string());
            }
        }
    }
    if let Some(shd) = r_pr.children().find(|n| n.has_tag_name("shd")) {
        if let Some(fill) = shd.attribute("fill").or_else(|| shd.attribute((W_NS, "fill"))).or_else(|| shd.attribute("w:fill")) {
            if fill != "auto" && !fill.is_empty() && is_hex_color(fill) {
                return Some(if fill.starts_with('#') { fill.to_string() } else { format!("#{}", fill) });
            }
        }
    }
    None
}

fn is_hex_color(s: &str) -> bool {
    let s = s.trim_start_matches('#');
    (s.len() == 6 || s.len() == 3 || s.len() == 8) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn render_drawing_html(
    output: &mut String,
    drawing_node: &roxmltree::Node,
    rels: &oxml::rels::Relationships,
    package: &OxmlPackage,
) {
    let blip = drawing_node.descendants().find(|n| n.has_tag_name("blip"));
    if let Some(blip_node) = blip {
        let embed_r_id = blip_node.attribute((W_NS, "embed"))
            .or_else(|| blip_node.attribute("r:embed"))
            .or_else(|| blip_node.attribute("embed"))
            .unwrap_or("");
        if !embed_r_id.is_empty() {
            if let Some(rel) = rels.get(embed_r_id) {
                let target = &rel.target;
                let part_path = if target.starts_with("word/") {
                    target.to_string()
                } else {
                    format!("word/{}", target)
                };

                if let Ok(bytes) = package.read_part_bytes(&part_path) {
                    let b64 = base64_encode(bytes);
                    let ext = std::path::Path::new(&part_path)
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("png")
                        .to_lowercase();
                    let mime = match ext.as_str() {
                        "jpg" | "jpeg" => "image/jpeg",
                        "gif" => "image/gif",
                        "svg" => "image/svg+xml",
                        "webp" => "image/webp",
                        _ => "image/png",
                    };

                    let mut cx_px = 300.0;
                    let mut cy_px = 200.0;
                    if let Some(extent) = drawing_node.descendants().find(|n| n.has_tag_name("extent")) {
                        let cx_str = extent.attribute("cx").unwrap_or("");
                        let cy_str = extent.attribute("cy").unwrap_or("");
                        if let (Ok(cx_emu), Ok(cy_emu)) = (cx_str.parse::<f64>(), cy_str.parse::<f64>()) {
                            cx_px = cx_emu / 9525.0;
                            cy_px = cy_emu / 9525.0;
                        }
                    }

                    output.push_str(&format!(
                        "<img src=\"data:{};base64,{}\" style=\"max-width:100%; width:{:.1}px; height:{:.1}px; object-fit:contain;\" />",
                        mime, b64, cx_px, cy_px
                    ));
                    return;
                }
            }
        }
    }
    output.push_str("<span class=\"img-error\">[Drawing]</span>");
}

fn render_run(
    node: &roxmltree::Node,
    path: &str,
    output: &mut String,
    styles: &HashMap<String, DocxStyle>,
    doc_defaults: &DocDefaults,
    _para_style_id: &str,
    package: &OxmlPackage,
    rels: &oxml::rels::Relationships,
) {
    if let Some(drawing) = node.descendants().find(|n| n.has_tag_name("drawing")) {
        render_drawing_html(output, &drawing, rels, package);
        return;
    }

    if let Some(pict) = node.children().find(|n| n.has_tag_name("pict")) {
        let mut text = String::new();
        for t in pict.descendants().filter(|n| n.has_tag_name("t")) {
            if let Some(val) = t.text() {
                if !val.trim().is_empty() {
                    if !text.is_empty() { text.push(' '); }
                    text.push_str(val);
                }
            }
        }
        for tp in pict.descendants().filter(|n| n.has_tag_name("textpath")) {
            if let Some(val) = tp.attribute("string").or_else(|| tp.attribute("v:string")) {
                if !val.trim().is_empty() {
                    if !text.is_empty() { text.push(' '); }
                    text.push_str(val);
                }
            }
        }
        if !text.trim().is_empty() {
            output.push_str(&format!(
                "<span class=\"vml-fallback\" style=\"color:#666;font-style:italic\">{}</span>",
                html_escape(&text)
            ));
        }
        return;
    }

    if let Some(fld_char) = node.children().find(|n| n.has_tag_name("fldChar")) {
        if fld_char.attribute((W_NS, "fldCharType")).or_else(|| fld_char.attribute("w:fldCharType")) == Some("begin") {
            if let Some(ff_data) = fld_char.children().find(|n| n.has_tag_name("ffData")) {
                if let Some(check_box) = ff_data.children().find(|n| n.has_tag_name("checkBox")) {
                    let default_checked = check_box.children().find(|n| n.has_tag_name("default"))
                        .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                        .map(|v| v == "true" || v == "1")
                        .unwrap_or(false);
                    let current_checked = check_box.children().find(|n| n.has_tag_name("checked"))
                        .and_then(|n| n.attribute((W_NS, "val")).or_else(|| n.attribute("w:val")))
                        .map(|v| v == "true" || v == "1")
                        .unwrap_or(false);
                    let is_checked = current_checked || default_checked;
                    output.push_str(if is_checked { "☑" } else { "☐" });
                    return;
                }
            }
        }
    }

    let has_content = node.children().any(|c| {
        let name = c.tag_name().name();
        name == "br" || name == "tab" || name == "sym" || name == "cr" || name == "noBreakHyphen" || name == "softHyphen"
        || (name == "t" && !c.text().unwrap_or("").is_empty())
    });

    if !has_content {
        return;
    }

    let p_node = node.parent().unwrap_or(*node);
    let (size_pt, color, font, bold, italic, underline, strike, bg_color) =
        resolve_run_properties(node, &p_node, styles, doc_defaults);

    let mut css_parts = Vec::new();
    if let Some(sz) = size_pt {
        css_parts.push(format!("font-size:{}pt", sz));
    }
    if let Some(ref c) = color {
        css_parts.push(format!("color:{}", c));
    }
    if let Some(ref f) = font {
        css_parts.push(format!("font-family:'{}'", f));
    }
    if bold {
        css_parts.push("font-weight:bold".to_string());
    }
    if italic {
        css_parts.push("font-style:italic".to_string());
    }
    if underline {
        css_parts.push("text-decoration:underline".to_string());
    }
    if strike {
        css_parts.push("text-decoration:line-through".to_string());
    }
    if let Some(ref bg) = bg_color {
        css_parts.push(format!("background-color:{}", bg));
    }

    let style_attr = if css_parts.is_empty() {
        String::new()
    } else {
        format!(" style=\"{}\"", css_parts.join("; "))
    };

    output.push_str(&format!("<span data-path=\"{}\"{}>", path, style_attr));

    for child in node.children() {
        if !child.is_element() {
            continue;
        }
        let tag = child.tag_name().name();
        if tag == "t" {
            output.push_str(&html_escape(child.text().unwrap_or("")));
        } else if tag == "tab" {
            output.push_str("<span style=\"display:inline-block; width:36pt;\"></span>");
        } else if tag == "br" {
            output.push_str("<br>");
        } else if tag == "cr" {
            output.push_str("<br>");
        } else if tag == "noBreakHyphen" {
            output.push_str("\u{2011}");
        } else if tag == "softHyphen" {
            output.push_str("&shy;");
        } else if tag == "sym" {
            let char_code = child.attribute((W_NS, "char")).or_else(|| child.attribute("w:char")).unwrap_or("");
            let sym_font = child.attribute((W_NS, "font")).or_else(|| child.attribute("w:font")).unwrap_or("");
            if !char_code.is_empty() {
                if let Ok(code) = u32::from_str_radix(char_code, 16) {
                    if let Some(ch) = std::char::from_u32(code) {
                        if !sym_font.is_empty() {
                            output.push_str(&format!(
                                "<span style=\"font-family:'{}'\">{}</span>",
                                sym_font, ch
                            ));
                        } else {
                            output.push_str(&ch.to_string());
                        }
                    } else {
                        output.push_str("\u{25A1}");
                    }
                } else {
                    output.push_str("\u{25A1}");
                }
            } else {
                output.push_str("\u{25A1}");
            }
        }
    }

    output.push_str("</span>");
}

fn resolve_run_properties(
    run_node: &roxmltree::Node,
    para_node: &roxmltree::Node,
    styles: &HashMap<String, DocxStyle>,
    doc_defaults: &DocDefaults,
) -> (Option<f64>, Option<String>, Option<String>, bool, bool, bool, bool, Option<String>) {
    let mut size_pt = Some(doc_defaults.size_pt);
    let mut color = Some(doc_defaults.color.clone());
    let mut font = Some(doc_defaults.font.clone());
    let mut bold = false;
    let mut italic = false;
    let mut underline = false;
    let mut strike = false;
    let mut bg_color = None;

    let p_pr = para_node.children().find(|n| n.has_tag_name("pPr"));
    let mut p_style_id = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("pStyle")))
        .and_then(|s| get_node_attr(&s, "val").or_else(|| get_node_attr(&s, "styleId")))
        .unwrap_or("")
        .to_string();

    if p_style_id.is_empty() {
        if let Some(def_style) = styles.values().find(|s| s.is_default_paragraph) {
            p_style_id = def_style.style_id.clone();
        }
    }

    let mut p_style_chain = Vec::new();
    let mut curr = p_style_id.clone();
    let mut visited = std::collections::HashSet::new();
    while !curr.is_empty() && visited.insert(curr.clone()) {
        if let Some(style) = styles.get(&curr) {
            p_style_chain.push(style);
            curr = style.based_on.clone().unwrap_or_default();
        } else {
            break;
        }
    }
    for style in p_style_chain.iter().rev() {
        if let Some(sz) = style.size_pt { size_pt = Some(sz); }
        if let Some(ref col) = style.color { color = Some(col.clone()); }
        if let Some(ref f) = style.font_ascii { font = Some(f.clone()); }
        if let Some(b) = style.bold { bold = b; }
        if let Some(it) = style.italic { italic = it; }
        if let Some(u) = style.underline { underline = u; }
        if let Some(s) = style.strike { strike = s; }
    }

    let r_pr = run_node.children().find(|n| n.has_tag_name("rPr"));
    let r_style_id = r_pr.as_ref()
        .and_then(|r| r.children().find(|n| n.has_tag_name("rStyle")))
        .and_then(|s| get_node_attr(&s, "val"))
        .unwrap_or("");
    if !r_style_id.is_empty() {
        let mut r_style_chain = Vec::new();
        let mut curr = r_style_id.to_string();
        let mut visited = std::collections::HashSet::new();
        while !curr.is_empty() && visited.insert(curr.clone()) {
            if let Some(style) = styles.get(&curr) {
                r_style_chain.push(style);
                curr = style.based_on.clone().unwrap_or_default();
            } else {
                break;
            }
        }
        for style in r_style_chain.iter().rev() {
            if let Some(sz) = style.size_pt { size_pt = Some(sz); }
            if let Some(ref col) = style.color { color = Some(col.clone()); }
            if let Some(ref f) = style.font_ascii { font = Some(f.clone()); }
            if let Some(b) = style.bold { bold = b; }
            if let Some(it) = style.italic { italic = it; }
            if let Some(u) = style.underline { underline = u; }
            if let Some(s) = style.strike { strike = s; }
        }
    }

    if let Some(rp) = r_pr {
        if let Some(sz) = rp.children().find(|n| n.has_tag_name("sz")) {
            if let Some(val) = get_node_attr(&sz, "val") {
                if let Ok(half_pt) = val.parse::<f64>() {
                    size_pt = Some(half_pt / 2.0);
                }
            }
        }
        if let Some(color_el) = rp.children().find(|n| n.has_tag_name("color")) {
            if let Some(val) = get_node_attr(&color_el, "val") {
                if val != "auto" && !val.is_empty() {
                    color = Some(if val.starts_with('#') { val.to_string() } else { format!("#{}", val) });
                }
            }
        }
        if let Some(rf_el) = rp.children().find(|n| n.has_tag_name("rFonts")) {
            if let Some(ascii) = get_node_attr(&rf_el, "ascii") {
                font = Some(ascii.to_string());
            }
        }
        if rp.children().any(|n| n.has_tag_name("b")) { bold = true; }
        if rp.children().any(|n| n.has_tag_name("i")) { italic = true; }
        if rp.children().any(|n| n.has_tag_name("u")) { underline = true; }
        if rp.children().any(|n| n.has_tag_name("strike")) { strike = true; }
        
        if let Some(bg) = resolve_run_background(&rp) {
            bg_color = Some(bg);
        }
    }

    (size_pt, color, font, bold, italic, underline, strike, bg_color)
}

fn resolve_paragraph_metrics(
    para_node: &roxmltree::Node,
    styles: &HashMap<String, DocxStyle>,
    doc_defaults: &DocDefaults,
) -> ParagraphMetrics {
    let mut align = doc_defaults.default_align.clone();
    let mut space_before_pt = 0.0;
    let mut space_after_pt = doc_defaults.space_after_pt;
    let mut line_spacing_mult = Some(doc_defaults.line_height);
    let mut line_spacing_exact = None;
    let mut margin_left_pt = 0.0;
    let mut text_indent_pt = 0.0;
    let mut shading_fill = None;

    let p_pr = para_node.children().find(|n| n.has_tag_name("pPr"));
    let mut p_style_id = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("pStyle")))
        .and_then(|s| get_node_attr(&s, "val"))
        .unwrap_or("")
        .to_string();

    if p_style_id.is_empty() {
        if let Some(def_style) = styles.values().find(|s| s.is_default_paragraph) {
            p_style_id = def_style.style_id.clone();
        }
    }

    let mut p_style_chain = Vec::new();
    let mut curr = p_style_id;
    let mut visited = std::collections::HashSet::new();
    while !curr.is_empty() && visited.insert(curr.clone()) {
        if let Some(style) = styles.get(&curr) {
            p_style_chain.push(style);
            curr = style.based_on.clone().unwrap_or_default();
        } else {
            break;
        }
    }

    for style in p_style_chain.iter().rev() {
        if let Some(ref al) = style.alignment { align = al.clone(); }
        if let Some(sb) = style.spacing_before_pt { space_before_pt = sb; }
        if let Some(sa) = style.spacing_after_pt { space_after_pt = sa; }
        if let Some(lm) = style.line_spacing_mult {
            line_spacing_mult = Some(lm);
            line_spacing_exact = None;
        }
        if let Some(le) = style.line_spacing_exact {
            line_spacing_exact = Some(le);
            line_spacing_mult = None;
        }
        if let Some(ref sh) = style.shading_fill { shading_fill = Some(sh.clone()); }
    }

    if let Some(pp) = p_pr {
        if let Some(jc) = pp.children().find(|n| n.has_tag_name("jc")) {
            if let Some(val) = get_node_attr(&jc, "val") {
                align = match val {
                    "center" => "center".to_string(),
                    "right" | "end" => "right".to_string(),
                    "both" | "distribute" => "justify".to_string(),
                    _ => "left".to_string(),
                };
            }
        }
        if let Some(spacing) = pp.children().find(|n| n.has_tag_name("spacing")) {
            if let Some(before) = get_node_attr(&spacing, "before") {
                if let Ok(twips) = before.parse::<f64>() {
                    space_before_pt = twips / 20.0;
                }
            }
            if let Some(after) = get_node_attr(&spacing, "after") {
                if let Ok(twips) = after.parse::<f64>() {
                    space_after_pt = twips / 20.0;
                }
            }
            if let Some(line) = get_node_attr(&spacing, "line") {
                if let Ok(twips) = line.parse::<f64>() {
                    let rule = get_node_attr(&spacing, "lineRule").unwrap_or("auto");
                    if rule == "auto" {
                        line_spacing_mult = Some(twips / 240.0);
                        line_spacing_exact = None;
                    } else {
                        line_spacing_exact = Some(twips / 20.0);
                        line_spacing_mult = None;
                    }
                }
            }
        }
        if let Some(ind) = pp.children().find(|n| n.has_tag_name("ind")) {
            let mut left = 0.0;
            let mut hanging = 0.0;
            let mut first_line = 0.0;
            if let Some(l) = get_node_attr(&ind, "left") {
                if let Ok(twips) = l.parse::<f64>() {
                    left = twips / 20.0;
                }
            }
            if let Some(h) = get_node_attr(&ind, "hanging") {
                if let Ok(twips) = h.parse::<f64>() {
                    hanging = twips / 20.0;
                }
            }
            if let Some(fl) = get_node_attr(&ind, "firstLine") {
                if let Ok(twips) = fl.parse::<f64>() {
                    first_line = twips / 20.0;
                }
            }
            
            if hanging > 0.0 && left == 0.0 {
                left = hanging;
            }
            margin_left_pt = left;
            if first_line > 0.0 {
                text_indent_pt = first_line;
            } else if hanging > 0.0 {
                text_indent_pt = -hanging;
            }
        }
        if let Some(shd) = pp.children().find(|n| n.has_tag_name("shd")) {
            if let Some(fill) = get_node_attr(&shd, "fill") {
                if fill != "auto" && !fill.is_empty() {
                    shading_fill = Some(if fill.starts_with('#') { fill.to_string() } else { format!("#{}", fill) });
                }
            }
        }
    }

    ParagraphMetrics {
        align,
        space_before_pt,
        space_after_pt,
        line_spacing_mult,
        line_spacing_exact,
        margin_left_pt,
        text_indent_pt,
        shading_fill,
    }
}

fn get_style_name(style_id: &str, styles: &HashMap<String, DocxStyle>) -> String {
    if style_id.is_empty() {
        return "Normal".to_string();
    }
    if let Some(style) = styles.get(style_id) {
        if let Some(ref name) = style.name {
            return name.clone();
        }
    }
    style_id.to_string()
}

fn get_heading_level_from_name(style_name: &str) -> usize {
    let name_lower = style_name.to_lowercase();
    if name_lower.contains("heading") || name_lower.contains("标题") {
        for c in style_name.chars().rev() {
            if c.is_ascii_digit() {
                if let Some(d) = c.to_digit(10) {
                    let val = d as usize;
                    if val >= 1 && val <= 6 {
                        return val;
                    }
                }
            }
        }
        return 1;
    }
    if style_name == "Title" {
        return 1;
    }
    if style_name == "Subtitle" {
        return 2;
    }
    0
}

fn get_paragraph_text(node: &roxmltree::Node) -> String {
    let mut text = String::new();
    for descendant in node.descendants() {
        if descendant.has_tag_name("t") {
            if let Some(txt) = descendant.text() {
                text.push_str(txt);
            }
        }
    }
    text
}

fn get_font_ratio(font_name: &str) -> f64 {
    match font_name.to_lowercase().as_str() {
        "calibri" => 1.25,
        "times new roman" => 1.15,
        "arial" => 1.15,
        "simsun" | "宋体" => 1.3,
        "microsoft yahei" | "微软雅黑" => 1.3,
        "dengxian" | "等线" => 1.2,
        _ => 1.15,
    }
}

fn get_marker_inline_css(lvl: &NumLevel) -> String {
    let mut parts = Vec::new();
    if let Some(ref color) = lvl.color {
        if color != "auto" && !color.is_empty() {
            let clean_color = if color.starts_with('#') { color.to_string() } else { format!("#{}", color) };
            parts.push(format!("color:{}", clean_color));
        }
    }
    if let Some(ref font_name) = lvl.font_name {
        if !font_name.is_empty() {
            parts.push(format!("font-family:'{}'", font_name));
        }
    }
    if let Some(size) = lvl.font_size_pt {
        parts.push(format!("font-size:{:.2}pt", size));
        let font_name = lvl.font_name.as_deref().unwrap_or("Calibri");
        let ratio = get_font_ratio(font_name);
        if ratio > 0.0 {
            parts.push(format!("line-height:{:.4}", ratio));
        }
    }
    if lvl.bold {
        parts.push("font-weight:bold".to_string());
    }
    if lvl.italic {
        parts.push("font-style:italic".to_string());
    }
    parts.join(";")
}

fn count_percent_digits(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut count = 0;
    if bytes.len() < 2 {
        return 0;
    }
    for i in 0..bytes.len() - 1 {
        if bytes[i] == b'%' && bytes[i + 1].is_ascii_digit() {
            count += 1;
        }
    }
    count
}

fn get_custom_list_style_string(
    num_id: &str,
    ilvl: usize,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> Option<String> {
    let fmt = get_numbering_format(num_id, ilvl, num_maps);
    if !fmt.eq_ignore_ascii_case("bullet") {
        return None;
    }
    let text = get_level_text(num_id, ilvl, num_maps);
    if text.is_empty() {
        return None;
    }
    if text == "•" || text == "o" || text == "▪" || text == "◦" || text == "" {
        return None;
    }
    let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
    Some(format!("'{} '", escaped))
}

fn build_list_marker_css(
    body_node: &roxmltree::Node,
    styles: &HashMap<String, DocxStyle>,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> String {
    let mut seen = HashSet::new();
    for descendant in body_node.descendants() {
        if descendant.has_tag_name("p") {
            if let Some((num_id, ilvl_raw)) = resolve_num_pr(&descendant, styles) {
                if num_id != "0" {
                    let mut ilvl = ilvl_raw;
                    if ilvl > 8 { ilvl = 8; }
                    seen.insert((num_id, ilvl));
                }
            }
        }
    }
    
    if seen.is_empty() {
        return String::new();
    }
    
    let mut sb = String::new();
    let mut sorted_seen: Vec<_> = seen.into_iter().collect();
    sorted_seen.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    
    for (num_id, ilvl) in sorted_seen {
        let lvl_opt = num_maps.get(&num_id)
            .and_then(|levels| levels.get(&ilvl.to_string()));
        if let Some(lvl) = lvl_opt {
            let list_style_str = get_custom_list_style_string(&num_id, ilvl, num_maps);
            let marker_props = build_marker_css_properties(lvl, list_style_str.is_some());
            
            if marker_props.is_empty() && list_style_str.is_none() {
                continue;
            }
            
            if let Some(ref list_style) = list_style_str {
                sb.push_str(&format!("li.marker-{}-{} {{ list-style-type: {}; }}\n", num_id, ilvl, list_style));
            }
            if !marker_props.is_empty() {
                sb.push_str(&format!("li.marker-{}-{}::marker {{ {} }}\n", num_id, ilvl, marker_props));
            }
        }
    }
    sb
}

fn build_marker_css_properties(lvl: &NumLevel, include_font_family: bool) -> String {
    let mut parts = Vec::new();
    if let Some(ref color) = lvl.color {
        if color != "auto" && !color.is_empty() {
            let clean_color = if color.starts_with('#') { color.to_string() } else { format!("#{}", color) };
            parts.push(format!("color:{}", clean_color));
        }
    }
    if include_font_family {
        if let Some(ref font_name) = lvl.font_name {
            if !font_name.is_empty() {
                parts.push(format!("font-family:'{}'", font_name));
            }
        }
    }
    if let Some(size) = lvl.font_size_pt {
        parts.push(format!("font-size:{:.2}pt", size));
        let font_name = lvl.font_name.as_deref().unwrap_or("Calibri");
        let ratio = get_font_ratio(font_name);
        if ratio > 0.0 {
            parts.push(format!("line-height:{:.4}", ratio));
        }
    }
    if lvl.bold {
        parts.push("font-weight:bold".to_string());
    }
    if lvl.italic {
        parts.push("font-style:italic".to_string());
    }
    parts.join(";")
}

fn get_paragraph_list_style(
    node: &roxmltree::Node,
    styles: &HashMap<String, DocxStyle>,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> Option<String> {
    let p_pr = node.children().find(|n| n.has_tag_name("pPr"));
    
    // Check if numbering is suppressed
    if let Some(pp) = p_pr.as_ref() {
        if let Some(num_pr) = pp.children().find(|n| n.has_tag_name("numPr")) {
            let num_id = num_pr.children().find(|n| n.has_tag_name("numId"))
                .and_then(|n| get_node_attr(&n, "val"))
                .unwrap_or("");
            if num_id == "0" {
                return None;
            }
        }
    }

    // Direct numPr always wins
    if let Some(pp) = p_pr.as_ref() {
        if let Some(num_pr) = pp.children().find(|n| n.has_tag_name("numPr")) {
            let num_id = num_pr.children().find(|n| n.has_tag_name("numId"))
                .and_then(|n| get_node_attr(&n, "val"));
            if let Some(nid) = num_id {
                if nid != "0" && !nid.is_empty() {
                    let ilvl = num_pr.children().find(|n| n.has_tag_name("ilvl"))
                        .and_then(|n| get_node_attr(&n, "val"))
                        .unwrap_or("0")
                        .parse::<usize>()
                        .unwrap_or(0);
                    let num_fmt = get_numbering_format(nid, ilvl, num_maps);
                    return Some(if num_fmt == "bullet" { "bullet".to_string() } else { "ordered".to_string() });
                }
            }
        }
    }

    // Style-inherited numPr: skip when the paragraph is itself a heading
    let style_id = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("pStyle")))
        .and_then(|s| get_node_attr(&s, "val"))
        .unwrap_or("");
    
    let style_name = get_style_name(style_id, styles);
    if !style_name.is_empty() {
        if style_name.contains("Heading") || style_name.contains("标题")
            || style_name.to_lowercase().starts_with("heading")
            || style_name == "Title" || style_name == "Subtitle"
        {
            return None;
        }
    }

    let resolved = resolve_num_pr(node, styles);
    if let Some((num_id, ilvl_r)) = resolved {
        if num_id == "0" {
            return None;
        }
        let num_fmt_r = get_numbering_format(&num_id, ilvl_r, num_maps);
        return Some(if num_fmt_r == "bullet" { "bullet".to_string() } else { "ordered".to_string() });
    }

    None
}

fn resolve_num_pr(
    node: &roxmltree::Node,
    styles: &HashMap<String, DocxStyle>,
) -> Option<(String, usize)> {
    let p_pr = node.children().find(|n| n.has_tag_name("pPr"));
    if let Some(pp) = p_pr.as_ref() {
        if let Some(num_pr) = pp.children().find(|n| n.has_tag_name("numPr")) {
            let num_id = num_pr.children().find(|n| n.has_tag_name("numId"))
                .and_then(|n| get_node_attr(&n, "val"))
                .unwrap_or("");
            let ilvl = num_pr.children().find(|n| n.has_tag_name("ilvl"))
                .and_then(|n| get_node_attr(&n, "val"))
                .unwrap_or("0")
                .parse::<usize>()
                .unwrap_or(0);
            if num_id == "0" {
                return None;
            }
            if !num_id.is_empty() {
                return Some((num_id.to_string(), ilvl));
            }
        }
    }

    let style_id = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("pStyle")))
        .and_then(|s| get_node_attr(&s, "val"))
        .unwrap_or("");

    if !style_id.is_empty() {
        let mut curr = style_id.to_string();
        let mut visited = HashSet::new();
        while !curr.is_empty() && visited.insert(curr.clone()) {
            if let Some(style) = styles.get(&curr) {
                if let Some(ref nid) = style.num_id {
                    return Some((nid.clone(), style.ilvl.unwrap_or(0)));
                }
                curr = style.based_on.clone().unwrap_or_default();
            } else {
                break;
            }
        }
    }

    None
}

fn get_numbering_format(
    num_id: &str,
    ilvl: usize,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> String {
    num_maps.get(num_id)
        .and_then(|levels| levels.get(&ilvl.to_string()))
        .map(|lvl| lvl.num_fmt.clone())
        .unwrap_or_else(|| "decimal".to_string())
}

fn get_level_text(
    num_id: &str,
    ilvl: usize,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> String {
    num_maps.get(num_id)
        .and_then(|levels| levels.get(&ilvl.to_string()))
        .map(|lvl| lvl.lvl_text.clone())
        .unwrap_or_default()
}

fn get_list_level_indent_full(
    num_id: &str,
    ilvl: usize,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> (f64, f64) {
    num_maps.get(num_id)
        .and_then(|levels| levels.get(&ilvl.to_string()))
        .map(|lvl| (lvl.left_pt, lvl.hanging_pt))
        .unwrap_or((0.0, 0.0))
}

fn get_list_level_indent(
    num_id: &str,
    ilvl: usize,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> f64 {
    get_list_level_indent_full(num_id, ilvl, num_maps).0
}

fn get_start_value(
    num_id: &str,
    ilvl: usize,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> usize {
    num_maps.get(num_id)
        .and_then(|levels| levels.get(&ilvl.to_string()))
        .map(|lvl| lvl.start)
        .unwrap_or(1)
}

fn get_level_suffix(
    _num_id: &str,
    _ilvl: usize,
    _num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> String {
    "tab".to_string()
}

fn get_level_jc(
    num_id: &str,
    ilvl: usize,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
) -> String {
    num_maps.get(num_id)
        .and_then(|levels| levels.get(&ilvl.to_string()))
        .map(|lvl| lvl.jc.clone())
        .unwrap_or_else(|| "left".to_string())
}

fn close_all_lists(
    output: &mut String,
    list_stack: &mut Vec<String>,
    current_list_type: &mut Option<String>,
    pending_li_close: &mut bool,
) {
    if *pending_li_close {
        output.push_str("</li>\n");
        *pending_li_close = false;
    }
    while let Some(tag) = list_stack.pop() {
        output.push_str(&format!("</{}>\n", tag));
        if !list_stack.is_empty() {
            output.push_str("</li>\n");
        }
    }
    *current_list_type = None;
}

fn render_paragraph_content(
    node: &roxmltree::Node,
    para_path: &str,
    output: &mut String,
    styles: &HashMap<String, DocxStyle>,
    doc_defaults: &DocDefaults,
    rels: &oxml::rels::Relationships,
    package: &OxmlPackage,
) {
    let p_pr = node.children().find(|n| n.has_tag_name("pPr"));
    let p_style_id = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("pStyle")))
        .and_then(|s| s.attribute("val").or_else(|| s.attribute((W_NS, "val"))).or_else(|| s.attribute("w:val")))
        .unwrap_or("");

    let mut child_counts = HashMap::new();
    for child in node.children() {
        if !child.is_element() {
            continue;
        }
        let tag = child.tag_name().name();
        let idx = child_counts.entry(tag.to_string()).or_insert(0);
        *idx += 1;
        let child_path = format!("{}/{}[{}]", para_path, tag, idx);

        if tag == "r" {
            render_run(
                &child,
                &child_path,
                output,
                styles,
                doc_defaults,
                p_style_id,
                package,
                rels,
            );
        } else if tag == "oMath" || tag == "oMathPara" {
            let latex = omml_to_latex(&child);
            output.push_str(&format!(
                "<span class=\"katex-formula\" data-formula=\"{}\"></span>",
                html_escape(&latex)
            ));
        } else if tag == "hyperlink" {
            let rel_id = child
                .attribute((W_NS, "id"))
                .or_else(|| child.attribute("r:id"))
                .unwrap_or("");
            let anchor = child
                .attribute((W_NS, "anchor"))
                .or_else(|| child.attribute("w:anchor"))
                .unwrap_or("");
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
            let mut hyperlink_child_counts = HashMap::new();
            for run_child in child.children() {
                if !run_child.is_element() {
                    continue;
                }
                let r_tag = run_child.tag_name().name();
                let r_idx = hyperlink_child_counts.entry(r_tag.to_string()).or_insert(0);
                *r_idx += 1;
                let run_child_path = format!("{}/r[{}]", child_path, r_idx);
                if r_tag == "r" {
                    render_run(
                        &run_child,
                        &run_child_path,
                        output,
                        styles,
                        doc_defaults,
                        p_style_id,
                        package,
                        rels,
                    );
                } else if r_tag == "oMath" || r_tag == "oMathPara" {
                    let latex = omml_to_latex(&run_child);
                    output.push_str(&format!(
                        "<span class=\"katex-formula\" data-formula=\"{}\"></span>",
                        html_escape(&latex)
                    ));
                }
            }
            if !url.is_empty() {
                output.push_str("</a>");
            }
        }
    }
}

fn render_paragraph(
    node: &roxmltree::Node,
    path: &str,
    output: &mut String,
    styles: &HashMap<String, DocxStyle>,
    doc_defaults: &DocDefaults,
    num_maps: &HashMap<String, HashMap<String, NumLevel>>,
    _num_counters: &mut HashMap<String, usize>,
    rels: &oxml::rels::Relationships,
    package: &OxmlPackage,
    list_stack: &mut Vec<String>,
    current_list_type: &mut Option<String>,
    pending_li_close: &mut bool,
    num_id_level_offset: &mut HashMap<String, usize>,
    ol_count_per_level: &mut HashMap<usize, usize>,
    multi_level_counters: &mut HashMap<usize, usize>,
    heading_counters: &mut HashMap<usize, usize>,
    _w_para_count: &mut usize,
    _w_table_count: &mut usize,
    current_num_id: &mut Option<String>,
    current_list_level: &mut usize,
    abs_num_level_counters: &mut HashMap<String, HashMap<usize, usize>>,
    num_to_abs_map: &HashMap<String, String>,
    num_start_overrides: &HashMap<String, HashMap<usize, usize>>,
) {
    let p_pr = node.children().find(|n| n.has_tag_name("pPr"));
    let metrics = resolve_paragraph_metrics(node, styles, doc_defaults);

    let style_id = p_pr.as_ref()
        .and_then(|p| p.children().find(|n| n.has_tag_name("pStyle")))
        .and_then(|s| s.attribute((W_NS, "val")).or_else(|| s.attribute("w:val")))
        .unwrap_or("");

    let style_name = get_style_name(style_id, styles);
    let heading_level = get_heading_level_from_name(&style_name);

    let list_style = get_paragraph_list_style(node, styles, num_maps);
    
    if let Some(list_style_str) = list_style {
        let (num_id, ilvl_raw) = resolve_num_pr(node, styles).unwrap_or(("0".to_string(), 0));
        let mut ilvl = ilvl_raw;
        if ilvl > 8 { ilvl = 8; }
        
        let num_fmt = get_numbering_format(&num_id, ilvl, num_maps);
        let lvl_text = get_level_text(&num_id, ilvl, num_maps);
        let tag = if num_fmt == "bullet" { "ul" } else { "ol" };

        if let Some(ref cur_nid) = *current_num_id {
            if cur_nid != &num_id {
                if !list_stack.is_empty() && !num_id_level_offset.contains_key(&num_id) {
                    let cur_indent = get_list_level_indent(cur_nid, *current_list_level, num_maps);
                    let new_indent = get_list_level_indent(&num_id, ilvl, num_maps);
                    if new_indent > cur_indent {
                        let offset = *current_list_level + 1 - ilvl;
                        num_id_level_offset.insert(num_id.clone(), offset);
                    } else {
                        close_all_lists(output, list_stack, current_list_type, pending_li_close);
                        ol_count_per_level.clear();
                        multi_level_counters.clear();
                    }
                } else if list_stack.is_empty() {
                    ol_count_per_level.clear();
                    multi_level_counters.clear();
                    num_id_level_offset.clear();
                }
            }
        }

        let ilvl_ooxml = ilvl;
        if let Some(&offset) = num_id_level_offset.get(&num_id) {
            ilvl += offset;
        }
        if ilvl > 8 { ilvl = 8; }

        if *pending_li_close && ilvl + 1 <= list_stack.len() {
            output.push_str("</li>\n");
            *pending_li_close = false;
        }

        while list_stack.len() > ilvl + 1 {
            let close_tag = list_stack.pop().unwrap();
            output.push_str(&format!("</{}>\n</li>\n", close_tag));
        }

        if *pending_li_close {
            *pending_li_close = false;
        }

        let (lvl_left, lvl_hanging) = get_list_level_indent_full(&num_id, ilvl, num_maps);
        let parent_left = if ilvl > 0 { get_list_level_indent(&num_id, ilvl - 1, num_maps) } else { 0.0 };
        let is_multi_level = count_percent_digits(&lvl_text) > 1;
        let indent_pt = if is_multi_level {
            (lvl_left - lvl_hanging - parent_left) / 20.0
        } else {
            (lvl_left - parent_left) / 20.0
        };
        let indent_pt = if indent_pt < 18.0 { 18.0 } else { indent_pt };
        let hanging_pt = lvl_hanging / 20.0;

        let mut list_style_parts = format!("padding-left:{:.1}pt;margin:0", indent_pt);
        if tag == "ol" {
            list_style_parts.push_str(";list-style-type:none");
        } else if tag == "ul" {
            list_style_parts.push_str(";list-style-image:none");
            let bullet_type = match lvl_text.as_str() {
                "o" => "circle",
                "▪" | "\u{f0a7}" => "square",
                _ => "disc",
            };
            list_style_parts.push_str(&format!(";list-style-type:{}", bullet_type));
        }

        let indent_style = format!(" style=\"{}\"", list_style_parts);

        while list_stack.len() < ilvl + 1 {
            let mut nested_style = indent_style.clone();
            if list_stack.len() > 0 {
                if let Some(prev_sibling) = node.prev_sibling() {
                    if prev_sibling.has_tag_name("p") {
                        let prev_metrics = resolve_paragraph_metrics(&prev_sibling, styles, doc_defaults);
                        if prev_metrics.space_after_pt > 0.0 {
                            let style_trimmed = nested_style.trim_end_matches('"');
                            nested_style = format!("{};margin-top:{:.1}pt\"", style_trimmed, prev_metrics.space_after_pt);
                        }
                    }
                }
            }
            output.push_str(&format!("<{}{}>\n", tag, nested_style));
            list_stack.push(tag.to_string());
        }

        if list_stack.len() > 0 && list_stack.last().unwrap() != tag {
            let old_tag = list_stack.pop().unwrap();
            output.push_str(&format!("</{}>\n<{}{}>\n", old_tag, tag, indent_style));
            list_stack.push(tag.to_string());
        }

        let seed_abs_id = num_to_abs_map.get(&num_id).cloned();
        let seed_start = |for_ilvl: usize,
                          ol_count_per_level: &HashMap<usize, usize>,
                          num_start_overrides: &HashMap<String, HashMap<usize, usize>>,
                          seed_abs_id: &Option<String>,
                          abs_num_level_counters: &HashMap<String, HashMap<usize, usize>>,
                          num_maps: &HashMap<String, HashMap<String, NumLevel>>,
                          num_id: &str| -> usize {
            if let Some(&prev) = ol_count_per_level.get(&for_ilvl) {
                if prev > 0 {
                    return prev;
                }
            }
            if let Some(overrides) = num_start_overrides.get(num_id) {
                if let Some(&ovr) = overrides.get(&for_ilvl) {
                    return ovr.saturating_sub(1);
                }
            }
            if let Some(ref abs_id) = *seed_abs_id {
                if let Some(by_ilvl) = abs_num_level_counters.get(abs_id) {
                    if let Some(&running) = by_ilvl.get(&for_ilvl) {
                        if running > 0 {
                            return running;
                        }
                    }
                }
            }
            get_start_value(num_id, for_ilvl, num_maps).saturating_sub(1)
        };

        if tag == "ol" {
            let seed = seed_start(
                ilvl,
                ol_count_per_level,
                num_start_overrides,
                &seed_abs_id,
                abs_num_level_counters,
                num_maps,
                &num_id,
            );
            
            let count = ol_count_per_level.entry(ilvl).or_insert(seed);
            *count += 1;
            let current_count = *count;
            multi_level_counters.insert(ilvl, current_count);
            
            for lk in ilvl + 1..=8 {
                ol_count_per_level.remove(&lk);
                multi_level_counters.remove(&lk);
            }
            
            if let Some(ref abs_id) = seed_abs_id {
                let by_ilvl = abs_num_level_counters.entry(abs_id.clone()).or_insert_with(HashMap::new);
                by_ilvl.insert(ilvl, current_count);
                for lk in ilvl + 1..=8 {
                    by_ilvl.remove(&lk);
                }
            }
        }

        *current_list_type = Some(list_style_str);
        *current_list_level = ilvl;
        *current_num_id = Some(num_id.clone());
        
        output.push_str(&format!("<li class=\"marker-{}-{}\" data-path=\"{}\"", num_id, ilvl_ooxml, path));
        
        let mut li_styles = Vec::new();
        if metrics.align != "left" {
            li_styles.push(format!("text-align:{}", metrics.align));
        }
        if metrics.space_before_pt > 0.0 {
            li_styles.push(format!("margin-top:{:.1}pt", metrics.space_before_pt));
        }
        if metrics.space_after_pt > 0.0 {
            li_styles.push(format!("margin-bottom:{:.1}pt", metrics.space_after_pt));
        }
        if let Some(exact) = metrics.line_spacing_exact {
            li_styles.push(format!("line-height:{:.1}pt", exact));
        } else if let Some(mult) = metrics.line_spacing_mult {
            li_styles.push(format!("line-height:{:.2}", mult));
        }
        if let Some(ref sh) = metrics.shading_fill {
            li_styles.push(format!("background-color:{}", sh));
        }
        if !li_styles.is_empty() {
            output.push_str(&format!(" style=\"{}\"", li_styles.join("; ")));
        }
        output.push_str(">");

        if tag == "ol" {
            let template = if lvl_text.is_empty() { format!("%{}", ilvl + 1) } else { lvl_text.clone() };
            let mut marker_str = template;
            for k in 0..=8 {
                let pattern = format!("%{}", k + 1);
                if marker_str.contains(&pattern) {
                    let counter = multi_level_counters.get(&k).copied().unwrap_or(0);
                    let lvl_fmt = get_numbering_format(&num_id, k, num_maps);
                    let glyph = match lvl_fmt.as_str() {
                        "lowerRoman" => to_lower_roman(counter),
                        "upperRoman" => to_lower_roman(counter).to_uppercase(),
                        "lowerLetter" => {
                            if counter >= 1 && counter <= 26 {
                                ((b'a' + (counter - 1) as u8) as char).to_string()
                            } else {
                                counter.to_string()
                            }
                        }
                        "upperLetter" => {
                            if counter >= 1 && counter <= 26 {
                                ((b'A' + (counter - 1) as u8) as char).to_string()
                            } else {
                                counter.to_string()
                            }
                        }
                        _ => counter.to_string(),
                    };
                    marker_str = marker_str.replace(&pattern, &glyph);
                }
            }

            let suff = get_level_suffix(&num_id, ilvl, num_maps);
            let jc = get_level_jc(&num_id, ilvl, num_maps);
            let marker_width = if hanging_pt > 0.0 { format!("{:.1}pt", hanging_pt) } else { "3em".to_string() };
            let marker_padding = match suff.as_str() {
                "nothing" => "0",
                "space" => "0.25em",
                _ => "0.5em",
            };
            let align = match jc.as_str() {
                "right" => "right",
                "center" => "center",
                _ => "left",
            };
            
            let lvl_opt = num_maps.get(&num_id).and_then(|levels| levels.get(&ilvl_ooxml.to_string()));
            let mut marker_inline_css = String::new();
            if let Some(lvl) = lvl_opt {
                marker_inline_css = get_marker_inline_css(lvl);
            }

            let mut marker_style = format!(
                "display:inline-block;min-width:{};padding-right:{};text-align:{}",
                marker_width, marker_padding, align
            );
            if !marker_inline_css.is_empty() {
                marker_style = format!("{};{}", marker_inline_css, marker_style);
            }
            output.push_str(&format!("<span style=\"{}\">{}</span>", marker_style, html_escape(&marker_str)));
        }

        render_paragraph_content(node, path, output, styles, doc_defaults, rels, package);
        *pending_li_close = true;
    } else {
        close_all_lists(output, list_stack, current_list_type, pending_li_close);
        ol_count_per_level.clear();
        multi_level_counters.clear();
        num_id_level_offset.clear();
        *current_num_id = None;

        let tag = if heading_level > 0 {
            format!("h{}", heading_level)
        } else {
            "p".to_string()
        };

        let mut inline_styles = Vec::new();
        if metrics.align != "left" {
            inline_styles.push(format!("text-align:{}", metrics.align));
        }
        if metrics.space_before_pt > 0.0 {
            inline_styles.push(format!("margin-top:{:.1}pt", metrics.space_before_pt));
        }
        if metrics.space_after_pt > 0.0 {
            inline_styles.push(format!("margin-bottom:{:.1}pt", metrics.space_after_pt));
        }
        if let Some(exact) = metrics.line_spacing_exact {
            inline_styles.push(format!("line-height:{:.1}pt", exact));
        } else if let Some(mult) = metrics.line_spacing_mult {
            inline_styles.push(format!("line-height:{:.2}", mult));
        }
        if metrics.margin_left_pt > 0.0 {
            inline_styles.push(format!("margin-left:{:.1}pt", metrics.margin_left_pt));
        }
        if metrics.text_indent_pt != 0.0 {
            inline_styles.push(format!("text-indent:{:.1}pt", metrics.text_indent_pt));
        }
        if let Some(ref sh) = metrics.shading_fill {
            inline_styles.push(format!("background-color:{}", sh));
        }

        let style_attr = if inline_styles.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", inline_styles.join("; "))
        };

        let mut heading_num_html = String::new();
        if heading_level > 0 {
            let num_suppressed = if let Some(pp) = p_pr.as_ref() {
                if let Some(num_pr) = pp.children().find(|n| n.has_tag_name("numPr")) {
                    let num_id = num_pr.children().find(|n| n.has_tag_name("numId"))
                        .and_then(|n| get_node_attr(&n, "val"))
                        .unwrap_or("");
                    num_id == "0"
                } else {
                    false
                }
            } else {
                false
            };

            if !num_suppressed {
                if let Some((hn_num_id, hn_ilvl)) = resolve_num_pr(node, styles) {
                    let count = heading_counters.entry(hn_ilvl).or_insert(0);
                    *count += 1;
                    
                    for lk in hn_ilvl + 1..=8 {
                        heading_counters.remove(&lk);
                    }
                    
                    let lvl_text = get_level_text(&hn_num_id, hn_ilvl, num_maps);
                    if !lvl_text.is_empty() {
                        let mut num_str = lvl_text;
                        for k in 0..=8 {
                            let pattern = format!("%{}", k + 1);
                            if num_str.contains(&pattern) {
                                let counter = heading_counters.get(&k).copied().unwrap_or(0);
                                let lvl_fmt = get_numbering_format(&hn_num_id, k, num_maps);
                                let glyph = match lvl_fmt.as_str() {
                                    "lowerRoman" => to_lower_roman(counter),
                                    "upperRoman" => to_lower_roman(counter).to_uppercase(),
                                    "lowerLetter" => {
                                        if counter >= 1 && counter <= 26 {
                                            ((b'a' + (counter - 1) as u8) as char).to_string()
                                        } else {
                                            counter.to_string()
                                        }
                                    }
                                    "upperLetter" => {
                                        if counter >= 1 && counter <= 26 {
                                            ((b'A' + (counter - 1) as u8) as char).to_string()
                                        } else {
                                            counter.to_string()
                                        }
                                    }
                                    _ => counter.to_string(),
                                };
                                num_str = num_str.replace(&pattern, &glyph);
                            }
                        }
                        
                        let para_text = get_paragraph_text(node);
                        let para_text_trimmed = para_text.trim_start();
                        if !para_text_trimmed.starts_with(&num_str) {
                            heading_num_html = format!("<span class=\"heading-num\" style=\"margin-right:0.5em\">{}</span>", html_escape(&num_str));
                        }
                    }
                }
            }
        }

        output.push_str(&format!("<{} data-path=\"{}\"{}>", tag, path, style_attr));
        output.push_str(&heading_num_html);
        
        let len_before = output.len();
        render_paragraph_content(node, path, output, styles, doc_defaults, rels, package);
        if output.len() == len_before {
            output.push_str("&nbsp;");
        }
        output.push_str(&format!("</{}>\n", tag));
    }
}

fn render_table(
    node: &roxmltree::Node,
    path: &str,
    output: &mut String,
    styles: &HashMap<String, DocxStyle>,
    doc_defaults: &DocDefaults,
    rels: &oxml::rels::Relationships,
    package: &OxmlPackage,
) {
    output.push_str(&format!("<table data-path=\"{}\">\n", path));
    let mut r_idx = 0;
    for row in node.children().filter(|n| n.has_tag_name("tr")) {
        r_idx += 1;
        let row_path = format!("{}/tr[{}]", path, r_idx);
        output.push_str(&format!("<tr data-path=\"{}\">\n", row_path));
        let mut c_idx = 0;
        for cell in row.children().filter(|n| n.has_tag_name("tc")) {
            c_idx += 1;
            let cell_path = format!("{}/tc[{}]", row_path, c_idx);
            let tc_pr = cell.children().find(|n| n.has_tag_name("tcPr"));
            let mut span_attrs = String::new();

            if let Some(tp) = tc_pr.as_ref() {
                if let Some(gs) = tp.children().find(|n| n.has_tag_name("gridSpan")) {
                    if let Some(val) = gs
                        .attribute((W_NS, "val"))
                        .or_else(|| gs.attribute("w:val"))
                    {
                        span_attrs.push_str(&format!(" colspan=\"{}\"", val));
                    }
                }
            }

            output.push_str(&format!("<td data-path=\"{}\"{}>", cell_path, span_attrs));
            
            let mut cell_child_counts = HashMap::new();
            for child in cell.children() {
                if !child.is_element() {
                    continue;
                }
                let tag = child.tag_name().name();
                let idx = cell_child_counts.entry(tag.to_string()).or_insert(0);
                *idx += 1;
                let child_path = format!("{}/{}[{}]", cell_path, tag, *idx);
                if tag == "p" {
                    let _text = get_paragraph_text(&child);
                    let metrics = resolve_paragraph_metrics(&child, styles, doc_defaults);
                    let mut p_styles = Vec::new();
                    if metrics.align != "left" {
                        p_styles.push(format!("text-align:{}", metrics.align));
                    }
                    if metrics.space_before_pt > 0.0 {
                        p_styles.push(format!("margin-top:{:.1}pt", metrics.space_before_pt));
                    }
                    if metrics.space_after_pt > 0.0 {
                        p_styles.push(format!("margin-bottom:{:.1}pt", metrics.space_after_pt));
                    }
                    if let Some(exact) = metrics.line_spacing_exact {
                        p_styles.push(format!("line-height:{:.1}pt", exact));
                    } else if let Some(mult) = metrics.line_spacing_mult {
                        p_styles.push(format!("line-height:{:.2}", mult));
                    }
                    if metrics.margin_left_pt > 0.0 {
                        p_styles.push(format!("margin-left:{:.1}pt", metrics.margin_left_pt));
                    }
                    if metrics.text_indent_pt != 0.0 {
                        p_styles.push(format!("text-indent:{:.1}pt", metrics.text_indent_pt));
                    }
                    if let Some(ref sh) = metrics.shading_fill {
                        p_styles.push(format!("background-color:{}", sh));
                    }

                    let style_attr = if p_styles.is_empty() {
                        String::new()
                    } else {
                        format!(" style=\"{}\"", p_styles.join("; "))
                    };

                    output.push_str(&format!("<div data-path=\"{}\"{}>", child_path, style_attr));
                    let len_before = output.len();
                    render_paragraph_content(&child, &child_path, output, styles, doc_defaults, rels, package);
                    if output.len() == len_before {
                        output.push_str("&nbsp;");
                    }
                    output.push_str("</div>\n");
                } else if tag == "tbl" {
                    render_table(
                        &child,
                        &child_path,
                        output,
                        styles,
                        doc_defaults,
                        rels,
                        package,
                    );
                }
            }
            output.push_str("</td>\n");
        }
        output.push_str("</tr>\n");
    }
    output.push_str("</table>\n");
}


fn to_lower_roman(mut num: usize) -> String {
    if num == 0 || num > 3999 {
        return num.to_string();
    }
    let mut sb = String::new();
    let map = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
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

fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as usize;
        let b1 = if i + 1 < data.len() {
            data[i + 1] as usize
        } else {
            0
        };
        let b2 = if i + 2 < data.len() {
            data[i + 2] as usize
        } else {
            0
        };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[(triple >> 18) & 63] as char);
        result.push(ALPHABET[(triple >> 12) & 63] as char);

        if i + 1 < data.len() {
            result.push(ALPHABET[(triple >> 6) & 63] as char);
        } else {
            result.push('=');
        }

        if i + 2 < data.len() {
            result.push(ALPHABET[triple & 63] as char);
        } else {
            result.push('=');
        }

        i += 3;
    }
    result
}

const UPRIGHT_FUNCTION_NAMES: &[&str] = &[
    "lim", "sin", "cos", "tan", "log", "ln", "exp", "min", "max",
    "sup", "inf", "det", "gcd", "dim", "ker", "hom", "deg",
    "arg", "sec", "csc", "cot", "sinh", "cosh", "tanh",
];

const SYMBOL_TO_COMMAND_MAP: &[(&str, &str)] = &[
    ("→", "\\rightarrow "),
    ("←", "\\leftarrow "),
    ("↑", "\\uparrow "),
    ("↓", "\\downarrow "),
    ("⇒", "\\Rightarrow "),
    ("⇐", "\\Leftarrow "),
    ("±", "\\pm "),
    ("×", "\\times "),
    ("÷", "\\div "),
    ("·", "\\cdot "),
    ("≤", "\\leq "),
    ("≥", "\\geq "),
    ("≠", "\\neq "),
    ("≈", "\\approx "),
    ("≡", "\\equiv "),
    ("∈", "\\in "),
    ("∀", "\\forall "),
    ("∃", "\\exists "),
    ("∞", "\\infty "),
    ("△", "\\triangle "),
    ("′", "\\prime "),
    ("ℏ", "\\hbar "),
    ("⇌", "\\rightleftharpoons "),
    ("α", "\\alpha "),
    ("β", "\\beta "),
    ("γ", "\\gamma "),
    ("δ", "\\delta "),
    ("ε", "\\epsilon "),
    ("θ", "\\theta "),
    ("λ", "\\lambda "),
    ("μ", "\\mu "),
    ("π", "\\pi "),
    ("σ", "\\sigma "),
    ("φ", "\\phi "),
    ("ω", "\\omega "),
    ("Σ", "\\Sigma "),
    ("Π", "\\Pi "),
    ("Δ", "\\Delta "),
    ("Ω", "\\Omega "),
];

fn escape_latex(text: &str) -> String {
    let mut s = text.to_string();
    for &(symbol, cmd) in SYMBOL_TO_COMMAND_MAP {
        s = s.replace(symbol, cmd);
    }
    s
}

fn nary_char_to_command(chr: &str) -> &str {
    match chr {
        "∑" => "\\sum",
        "∫" => "\\int",
        "∬" => "\\iint",
        "∭" => "\\iiint",
        "∏" => "\\prod",
        "∐" => "\\coprod",
        "⋃" => "\\bigcup",
        "⋂" => "\\bigcap",
        _ => chr,
    }
}

fn needs_braces(text: &str) -> bool {
    text.chars().count() != 1
}

fn is_latex_hex(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.len() != 3 && s.len() != 6 && s.len() != 8 {
        return false;
    }
    s.chars().all(|c| c.is_ascii_hexdigit())
}

fn join_children_math(node: &roxmltree::Node) -> String {
    let mut sb = String::new();
    for child in node.children() {
        if child.is_element() {
            let part = omml_to_latex(&child);
            if !sb.is_empty()
                && !part.is_empty()
                && sb.chars().last().unwrap().is_whitespace()
                && part.chars().next().unwrap().is_whitespace()
            {
                let trimmed_part = part.trim_start();
                sb.push_str(trimmed_part);
            } else {
                sb.push_str(&part);
            }
        }
    }
    sb
}

fn arg_to_latex(arg: Option<&roxmltree::Node>) -> String {
    if let Some(node) = arg {
        join_children_math(node)
    } else {
        String::new()
    }
}

fn omml_to_latex(node: &roxmltree::Node) -> String {
    let name = node.tag_name().name();
    match name {
        "oMathPara" | "oMath" => join_children_math(node),
        "r" => {
            let t_elem = node.children().find(|n| n.has_tag_name("t"));
            let text = t_elem.map(|n| n.text().unwrap_or("")).unwrap_or("");

            // Check for math style in run properties (mathbf, mathrm, etc.)
            let r_pr = node.children().find(|n| n.has_tag_name("rPr"));

            let mut color_hex = None;
            if let Some(r_pr_node) = r_pr {
                if let Some(color_el) = r_pr_node.children().find(|n| n.has_tag_name("color")) {
                    color_hex = color_el
                        .attribute("val")
                        .or_else(|| color_el.attribute((W_NS, "val")))
                        .map(|s| s.to_string());
                }
            }

            let mut result = escape_latex(text);
            if let Some(r_pr_node) = r_pr {
                let sty = r_pr_node.children().find(|n| n.has_tag_name("sty"));
                let sty_val = sty
                    .and_then(|n| n.attribute("val").or_else(|| n.attribute((W_NS, "val"))))
                    .unwrap_or("");
                let has_nor = r_pr_node.children().any(|n| n.has_tag_name("nor"));

                if has_nor {
                    if UPRIGHT_FUNCTION_NAMES.contains(&text) {
                        result = format!("\\{}", text);
                    } else {
                        result = format!("\\text{{{}}}", result);
                    }
                } else if sty_val == "b" {
                    result = format!("\\mathbf{{{}}}", result);
                } else if sty_val == "bi" {
                    result = format!("\\boldsymbol{{{}}}", result);
                } else if sty_val == "p" {
                    result = format!("\\mathrm{{{}}}", result);
                }
            }

            if let Some(hex) = color_hex {
                if is_latex_hex(&hex) {
                    result = format!("\\textcolor{{#{}}}{{{}}}", hex, result);
                }
            }
            result
        }
        "sSub" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let sub_text = arg_to_latex(node.children().find(|n| n.has_tag_name("sub")).as_ref());
            if needs_braces(&sub_text) {
                format!("{}_{{{}}}", base_text, sub_text)
            } else {
                format!("{}_{}", base_text, sub_text)
            }
        }
        "sSup" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let sup_text = arg_to_latex(node.children().find(|n| n.has_tag_name("sup")).as_ref());
            if needs_braces(&sup_text) {
                format!("{}^{{{}}}", base_text, sup_text)
            } else {
                format!("{}^{}", base_text, sup_text)
            }
        }
        "sSubSup" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let sub_text = arg_to_latex(node.children().find(|n| n.has_tag_name("sub")).as_ref());
            let sup_text = arg_to_latex(node.children().find(|n| n.has_tag_name("sup")).as_ref());
            let sub_part = if needs_braces(&sub_text) {
                format!("_{{{}}}", sub_text)
            } else {
                format!("_{}", sub_text)
            };
            let sup_part = if needs_braces(&sup_text) {
                format!("^{{{}}}", sup_text)
            } else {
                format!("^{}", sup_text)
            };
            format!("{}{}{}", base_text, sub_part, sup_part)
        }
        "f" => {
            let num = arg_to_latex(node.children().find(|n| n.has_tag_name("num")).as_ref());
            let den = arg_to_latex(node.children().find(|n| n.has_tag_name("den")).as_ref());
            format!("\\frac{{{}}}{{{}}}", num, den)
        }
        "rad" => {
            let deg = node.children().find(|n| n.has_tag_name("deg"));
            let base_elem = node.children().find(|n| n.has_tag_name("e"));
            let base_text = arg_to_latex(base_elem.as_ref());

            let rad_pr = node.children().find(|n| n.has_tag_name("radPr"));
            let hide_deg = rad_pr.and_then(|n| n.children().find(|c| c.has_tag_name("degHide")));
            let is_hidden = hide_deg
                .and_then(|n| n.attribute("val"))
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false);

            let deg_text = if is_hidden {
                String::new()
            } else {
                arg_to_latex(deg.as_ref())
            };
            if deg_text.is_empty() {
                format!("\\sqrt{{{}}}", base_text)
            } else {
                format!("\\sqrt[{}]{{{}}}", deg_text, base_text)
            }
        }
        "nary" => {
            let nary_pr = node.children().find(|n| n.has_tag_name("naryPr"));
            let chr_elem = nary_pr.and_then(|n| n.children().find(|c| c.has_tag_name("chr")));
            let chr = chr_elem.and_then(|n| n.attribute("val")).unwrap_or("∑");
            let cmd = nary_char_to_command(chr);

            let sub_text = arg_to_latex(node.children().find(|n| n.has_tag_name("sub")).as_ref());
            let sup_text = arg_to_latex(node.children().find(|n| n.has_tag_name("sup")).as_ref());
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());

            let mut result = cmd.to_string();
            if !sub_text.is_empty() {
                if needs_braces(&sub_text) {
                    result.push_str(&format!("_{{{}}}", sub_text));
                } else {
                    result.push_str(&format!("_{}", sub_text));
                }
            }
            if !sup_text.is_empty() {
                if needs_braces(&sup_text) {
                    result.push_str(&format!("^{{{}}}", sup_text));
                } else {
                    result.push_str(&format!("^{}", sup_text));
                }
            }
            if !base_text.is_empty() {
                result.push_str(&format!(" {}", base_text));
            }
            result
        }
        "d" => {
            let d_pr = node.children().find(|n| n.has_tag_name("dPr"));
            let beg_chr = d_pr.and_then(|n| n.children().find(|c| c.has_tag_name("begChr")));
            let end_chr = d_pr.and_then(|n| n.children().find(|c| c.has_tag_name("endChr")));
            let begin = beg_chr.and_then(|n| n.attribute("val")).unwrap_or("(");
            let end = end_chr.and_then(|n| n.attribute("val")).unwrap_or(")");

            let bases: Vec<roxmltree::Node> =
                node.children().filter(|n| n.has_tag_name("e")).collect();
            if bases.len() == 1 {
                let inner = bases[0].children().find(|n| n.has_tag_name("m"));
                if let Some(inner_matrix) = inner {
                    let env_name = match (begin, end) {
                        ("(", ")") => Some("pmatrix"),
                        ("[", "]") => Some("bmatrix"),
                        ("{", "}") => Some("Bmatrix"),
                        ("|", "|") => Some("vmatrix"),
                        _ => None,
                    };
                    let matrix_content = omml_to_latex(&inner_matrix);
                    if let Some(env) = env_name {
                        return format!("\\begin{{{}}}{}\\end{{{}}}", env, matrix_content, env);
                    } else {
                        return format!(
                            "\\left{}\\begin{{matrix}}{}\\end{{matrix}}\\right{}",
                            begin, matrix_content, end
                        );
                    }
                }
            }
            let content: String = bases.iter().map(|b| arg_to_latex(Some(b))).collect();
            format!("{}{}{}", begin, content, end)
        }
        "limUpp" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let lim_text = arg_to_latex(node.children().find(|n| n.has_tag_name("lim")).as_ref());
            format!("\\overset{{{}}}{{{}}}", lim_text, base_text)
        }
        "limLow" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let lim_text = arg_to_latex(node.children().find(|n| n.has_tag_name("lim")).as_ref());
            format!("\\underset{{{}}}{{{}}}", lim_text, base_text)
        }
        "bar" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let bar_pr = node.children().find(|n| n.has_tag_name("barPr"));
            let pos_elem = bar_pr.and_then(|n| n.children().find(|c| c.has_tag_name("pos")));
            let pos_val = pos_elem.and_then(|n| n.attribute("val")).unwrap_or("");
            if pos_val == "bot" {
                format!("\\underline{{{}}}", base_text)
            } else {
                format!("\\overline{{{}}}", base_text)
            }
        }
        "acc" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let acc_pr = node.children().find(|n| n.has_tag_name("accPr"));
            let chr_elem = acc_pr.and_then(|n| n.children().find(|c| c.has_tag_name("chr")));
            let chr = chr_elem
                .and_then(|n| n.attribute("val"))
                .unwrap_or("\u{0302}");
            let cmd = match chr {
                "\u{0302}" => "hat",
                "\u{0304}" => "bar",
                "\u{20D7}" => "vec",
                "\u{0307}" => "dot",
                "\u{0308}" => "ddot",
                "\u{0303}" => "tilde",
                _ => "hat",
            };
            format!("\\{}{{{}}}", cmd, base_text)
        }
        "m" => {
            let matrix_rows: Vec<roxmltree::Node> =
                node.children().filter(|n| n.has_tag_name("mr")).collect();
            let row_strings: Vec<String> = matrix_rows
                .iter()
                .map(|mr| {
                    let cell_strings: Vec<String> = mr
                        .children()
                        .filter(|n| n.has_tag_name("e"))
                        .map(|e| arg_to_latex(Some(&e)).trim().to_string())
                        .collect();
                    cell_strings.join(" & ")
                })
                .collect();
            let content = row_strings.join(" \\\\ ");

            let is_in_del = node
                .parent()
                .map(|p| {
                    p.tag_name().name() == "e"
                        && p.parent()
                            .map(|gp| gp.tag_name().name() == "d")
                            .unwrap_or(false)
                })
                .unwrap_or(false);
            if !is_in_del {
                format!("\\begin{{matrix}}{}\\end{{matrix}}", content)
            } else {
                content
            }
        }
        "borderBox" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let bb_pr = node.children().find(|n| n.has_tag_name("borderBoxPr"));
            let has_strike_tlbr = bb_pr
                .map(|n| n.children().any(|c| c.has_tag_name("strikeTLBR")))
                .unwrap_or(false);
            let has_strike_bltr = bb_pr
                .map(|n| n.children().any(|c| c.has_tag_name("strikeBLTR")))
                .unwrap_or(false);
            let has_strike_h = bb_pr
                .map(|n| n.children().any(|c| c.has_tag_name("strikeH")))
                .unwrap_or(false);

            if has_strike_tlbr || has_strike_bltr || has_strike_h {
                format!("\\cancel{{{}}}", base_text)
            } else {
                format!("\\boxed{{{}}}", base_text)
            }
        }
        "groupChr" => {
            let base_text = arg_to_latex(node.children().find(|n| n.has_tag_name("e")).as_ref());
            let gc_pr = node.children().find(|n| n.has_tag_name("groupChrPr"));
            let chr_el = gc_pr.and_then(|n| n.children().find(|c| c.has_tag_name("chr")));
            let chr = chr_el.and_then(|n| n.attribute("val")).unwrap_or("");
            let pos_el = gc_pr.and_then(|n| n.children().find(|c| c.has_tag_name("pos")));
            let pos = pos_el.and_then(|n| n.attribute("val")).unwrap_or("");

            if chr == "\u{23DF}" || pos == "bot" {
                format!("\\underbrace{{{}}}", base_text)
            } else if chr == "\u{23DE}" || pos == "top" {
                format!("\\overbrace{{{}}}", base_text)
            } else {
                base_text
            }
        }
        _ => {
            let mut result = String::new();
            for child in node.children() {
                if child.is_element() {
                    result.push_str(&omml_to_latex(&child));
                }
            }
            result
        }
    }
}
