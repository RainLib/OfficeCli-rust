use handler_common::HandlerError;
use oxml::OxmlPackage;
use std::collections::HashMap;

struct MergeInfo {
    is_anchor: bool,
    row_span: usize,
    col_span: usize,
}

struct ParsedFont {
    name: String,
    size: f64,
    bold: bool,
    italic: bool,
    color: Option<String>,
}

struct ParsedFill {
    fill_type: String, // "solid", "gradient", "none"
    bg_color: Option<String>,
}

struct ParsedBorder {
    left: Option<(String, String)>,   // (style, color)
    right: Option<(String, String)>,
    top: Option<(String, String)>,
    bottom: Option<(String, String)>,
}

struct CellFormat {
    font_id: Option<usize>,
    fill_id: Option<usize>,
    border_id: Option<usize>,
    align_horiz: Option<String>,
    align_vert: Option<String>,
}

// 64 Default indexed colors for Excel compatibility
const DEFAULT_INDEXED_COLORS: &[&str] = &[
    "000000", "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF",
    "000000", "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF",
    "800000", "008000", "000080", "808000", "800080", "008080", "C0C0C0", "808080",
    "9999FF", "993366", "FFFFCC", "CCFFFF", "660066", "FF8080", "0066CC", "CCCCFF",
    "000080", "FF00FF", "FFFF00", "00FFFF", "800080", "800000", "008080", "0000FF",
    "00CCFF", "CCFFFF", "CCFFCC", "FFFF99", "99CCFF", "FF99CC", "CC99FF", "FFCC99",
    "3366FF", "33CCCC", "99CC00", "FFCC00", "FF9900", "FF6600", "666699", "969696",
    "2A6F97", "014F86", "012A4A", "A9D6E5", "89C2D9", "61A5C2", "468FAF", "2C7DA0"
];

fn apply_transforms(hex: &str, tint: Option<f64>) -> String {
    let hex_clean = hex.trim_start_matches('#');
    if hex_clean.len() < 6 {
        return hex.to_string();
    }
    let mut r = u8::from_str_radix(&hex_clean[0..2], 16).unwrap_or(0);
    let mut g = u8::from_str_radix(&hex_clean[2..4], 16).unwrap_or(0);
    let mut b = u8::from_str_radix(&hex_clean[4..6], 16).unwrap_or(0);

    if let Some(t) = tint {
        if t > 0.0 {
            r = (r as f64 + (255.0 - r as f64) * t).round() as u8;
            g = (g as f64 + (255.0 - g as f64) * t).round() as u8;
            b = (b as f64 + (255.0 - b as f64) * t).round() as u8;
        } else if t < 0.0 {
            let s = 1.0 + t;
            r = (r as f64 * s).round() as u8;
            g = (g as f64 * s).round() as u8;
            b = (b as f64 * s).round() as u8;
        }
    }
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

fn resolve_xml_color(
    color_node: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
    indexed_colors: &[&str],
) -> Option<String> {
    let mut base_hex = None;
    if let Some(rgb) = color_node.attribute("rgb") {
        let clean = rgb.trim_start_matches("FF"); // remove alpha if present
        base_hex = Some(clean.to_string());
    } else if let Some(indexed) = color_node.attribute("indexed").and_then(|s| s.parse::<usize>().ok()) {
        if indexed < indexed_colors.len() {
            base_hex = Some(indexed_colors[indexed].to_string());
        }
    } else if let Some(theme) = color_node.attribute("theme").and_then(|s| s.parse::<usize>().ok()) {
        let theme_names = ["lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6"];
        if theme < theme_names.len() {
            if let Some(hex) = theme_colors.get(theme_names[theme]) {
                base_hex = Some(hex.to_string());
            }
        }
    }

    if let Some(hex) = base_hex {
        let tint = color_node.attribute("tint").and_then(|s| s.parse::<f64>().ok());
        return Some(apply_transforms(&hex, tint));
    }
    None
}

/// Render the Excel workbook as HTML for browser preview.
pub fn view_as_html(package: &OxmlPackage) -> Result<String, HandlerError> {
    let model = crate::helpers::build_workbook_model(package)
        .map_err(|e| HandlerError::OperationFailed(e))?;

    // 1. Resolve Theme Colors
    let mut theme_colors = HashMap::new();
    if let Ok(theme_xml) = package.read_part_xml("xl/theme/theme1.xml") {
        if let Ok(doc) = roxmltree::Document::parse(&theme_xml) {
            if let Some(scheme) = doc.descendants().find(|n| n.has_tag_name("clrScheme")) {
                let clr_elements = ["dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4", "accent5", "accent6"];
                for elem in clr_elements {
                    if let Some(color_node) = scheme.descendants().find(|n| n.has_tag_name(elem)) {
                        if let Some(srgb) = color_node.descendants().find(|n| n.has_tag_name("srgbClr")) {
                            if let Some(val) = srgb.attribute("val") {
                                theme_colors.insert(elem.to_string(), val.to_string());
                            }
                        } else if let Some(sys) = color_node.descendants().find(|n| n.has_tag_name("sysClr")) {
                            if let Some(val) = sys.attribute("lastClr") {
                                theme_colors.insert(elem.to_string(), val.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Resolve Palette and Styles
    let mut indexed_colors = DEFAULT_INDEXED_COLORS.to_vec();
    let mut fonts = Vec::new();
    let mut fills = Vec::new();
    let mut borders = Vec::new();
    let mut cell_formats = Vec::new();

    if let Ok(styles_xml) = package.read_part_xml("xl/styles.xml") {
        if let Ok(doc) = roxmltree::Document::parse(&styles_xml) {
            // Read Custom Palette
            if let Some(colors_node) = doc.descendants().find(|n| n.has_tag_name("colors")) {
                if let Some(idx_node) = colors_node.descendants().find(|n| n.has_tag_name("indexedColors")) {
                    // Overwrite indexed palette
                    for (idx, rgb_node) in idx_node.children().filter(|n| n.has_tag_name("rgbColor")).enumerate() {
                        if let Some(rgb) = rgb_node.attribute("rgb") {
                            let clean = rgb.trim_start_matches("FF").to_string();
                            if idx < indexed_colors.len() {
                                // Keep static lifetime placeholder or parse dynamically
                                // We store them in a local string vector and point references if needed
                            }
                        }
                    }
                }
            }

            // Parse Fonts
            if let Some(fonts_node) = doc.descendants().find(|n| n.has_tag_name("fonts")) {
                for f_node in fonts_node.children().filter(|n| n.has_tag_name("font")) {
                    let name = f_node.children().find(|n| n.has_tag_name("name"))
                        .and_then(|n| n.attribute("val"))
                        .unwrap_or("Calibri").to_string();
                    let size = f_node.children().find(|n| n.has_tag_name("sz"))
                        .and_then(|n| n.attribute("val"))
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(11.0);
                    let bold = f_node.children().any(|n| n.has_tag_name("b"));
                    let italic = f_node.children().any(|n| n.has_tag_name("i"));
                    let color = f_node.children().find(|n| n.has_tag_name("color"))
                        .and_then(|c| resolve_xml_color(&c, &theme_colors, &indexed_colors));
                    fonts.push(ParsedFont { name, size, bold, italic, color });
                }
            }

            // Parse Fills
            if let Some(fills_node) = doc.descendants().find(|n| n.has_tag_name("fills")) {
                for fill in fills_node.children().filter(|n| n.has_tag_name("fill")) {
                    if let Some(pattern) = fill.children().find(|n| n.has_tag_name("patternFill")) {
                        let fill_type = pattern.attribute("patternType").unwrap_or("none").to_string();
                        let bg_color = pattern.children().find(|n| n.has_tag_name("fgColor"))
                            .and_then(|c| resolve_xml_color(&c, &theme_colors, &indexed_colors));
                        fills.push(ParsedFill { fill_type, bg_color });
                    } else {
                        fills.push(ParsedFill { fill_type: "none".to_string(), bg_color: None });
                    }
                }
            }

            // Parse Borders
            if let Some(borders_node) = doc.descendants().find(|n| n.has_tag_name("borders")) {
                for b_node in borders_node.children().filter(|n| n.has_tag_name("border")) {
                    let mut parse_edge = |edge_name: &str| -> Option<(String, String)> {
                        let edge = b_node.children().find(|n| n.has_tag_name(edge_name))?;
                        let style = edge.attribute("style")?.to_string();
                        let color = edge.children().find(|n| n.has_tag_name("color"))
                            .and_then(|c| resolve_xml_color(&c, &theme_colors, &indexed_colors))
                            .unwrap_or_else(|| "#D9D9D9".to_string());
                        Some((style, color))
                    };
                    borders.push(ParsedBorder {
                        left: parse_edge("left"),
                        right: parse_edge("right"),
                        top: parse_edge("top"),
                        bottom: parse_edge("bottom"),
                    });
                }
            }

            // Parse Cell Formats
            if let Some(xfs) = doc.descendants().find(|n| n.has_tag_name("cellXfs")) {
                for xf in xfs.children().filter(|n| n.has_tag_name("xf")) {
                    let font_id = xf.attribute("fontId").and_then(|s| s.parse::<usize>().ok());
                    let fill_id = xf.attribute("fillId").and_then(|s| s.parse::<usize>().ok());
                    let border_id = xf.attribute("borderId").and_then(|s| s.parse::<usize>().ok());
                    let align = xf.children().find(|n| n.has_tag_name("alignment"));
                    let align_horiz = align.as_ref().and_then(|a| a.attribute("horizontal")).map(|s| s.to_string());
                    let align_vert = align.as_ref().and_then(|a| a.attribute("vertical")).map(|s| s.to_string());
                    cell_formats.push(CellFormat { font_id, fill_id, border_id, align_horiz, align_vert });
                }
            }
        }
    }

    let mut sheets_html = String::new();

    for (ws_idx, ws) in model.sheets.iter().enumerate() {
        let mut merge_map: HashMap<(usize, usize), MergeInfo> = HashMap::new();
        let mut frozen_rows = 0;
        let mut frozen_cols = 0;
        let mut col_widths = HashMap::new();
        let mut row_heights = HashMap::new();
        let mut default_col_width = 64.0; // default in px
        let mut default_row_height = 20.0; // default in px

        if let Ok(ws_xml) = package.read_part_xml(&ws.part_path) {
            if let Ok(ws_doc) = roxmltree::Document::parse(&ws_xml) {
                // Parse Merge Cells
                if let Some(merge_cells) = ws_doc.descendants().find(|n| n.has_tag_name("mergeCells")) {
                    for mc in merge_cells.children().filter(|n| n.has_tag_name("mergeCell")) {
                        if let Some(range_ref) = mc.attribute("ref") {
                            if let Some((start, end)) = parse_range(range_ref) {
                                for r in start.row..=end.row {
                                    for c in start.col..=end.col {
                                        let is_anchor = r == start.row && c == start.col;
                                        let row_span = end.row - start.row + 1;
                                        let col_span = end.col - start.col + 1;
                                        merge_map.insert((r, c), MergeInfo { is_anchor, row_span, col_span });
                                    }
                                }
                            }
                        }
                    }
                }

                // Parse Frozen Panes
                if let Some(pane) = ws_doc.descendants().find(|n| n.has_tag_name("pane")) {
                    if let Some(state) = pane.attribute("state") {
                        if state == "frozen" || state == "frozenSplit" {
                            frozen_rows = pane.attribute("ySplit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                            frozen_cols = pane.attribute("xSplit").and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                        }
                    }
                }

                // Parse Format Props
                if let Some(fmt_pr) = ws_doc.descendants().find(|n| n.has_tag_name("sheetFormatPr")) {
                    default_col_width = fmt_pr.attribute("defaultColWidth").and_then(|s| s.parse::<f64>().ok()).unwrap_or(8.43) * 7.5;
                    default_row_height = fmt_pr.attribute("defaultRowHeight").and_then(|s| s.parse::<f64>().ok()).unwrap_or(15.0);
                }

                // Parse Explicit Col Widths
                if let Some(cols_node) = ws_doc.descendants().find(|n| n.has_tag_name("cols")) {
                    for col in cols_node.children().filter(|n| n.has_tag_name("col")) {
                        let min = col.attribute("min").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                        let max = col.attribute("max").and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
                        if let Some(w) = col.attribute("width").and_then(|s| s.parse::<f64>().ok()) {
                            let width_px = w * 7.5; // conversion factor
                            for c in min..=max {
                                col_widths.insert(c, width_px);
                            }
                        }
                    }
                }

                // Parse Explicit Row Heights
                if let Some(sheet_data) = ws_doc.descendants().find(|n| n.has_tag_name("sheetData")) {
                    for row in sheet_data.children().filter(|n| n.has_tag_name("row")) {
                        if let Some(r_idx) = row.attribute("r").and_then(|s| s.parse::<usize>().ok()) {
                            if let Some(h) = row.attribute("ht").and_then(|s| s.parse::<f64>().ok()) {
                                row_heights.insert(r_idx, h);
                            }
                        }
                    }
                }
            }
        }

        // Calculate frozen offsets
        let mut frozen_left_offsets = HashMap::new();
        let mut current_left = 40.0; // corner row-header width
        for c in 1..=frozen_cols {
            frozen_left_offsets.insert(c, current_left);
            let w = col_widths.get(&c).copied().unwrap_or(default_col_width);
            current_left += w;
        }

        let mut frozen_top_offsets = HashMap::new();
        let mut current_top = 26.0; // header height
        for r in 1..=frozen_rows {
            frozen_top_offsets.insert(r, current_top);
            let h = row_heights.get(&r).copied().unwrap_or(default_row_height);
            current_top += h;
        }

        let max_row = ws.max_row.min(200);
        let max_col = ws.max_col.min(30);

        let active_class = if ws_idx == 0 { " active" } else { "" };
        sheets_html.push_str(&format!(
            "<div class=\"sheet-content{}\" data-sheet=\"{}\">\n<div class=\"table-wrapper\">\n<table>\n",
            active_class, ws_idx
        ));

        // Generate Colgroup
        sheets_html.push_str("<colgroup><col style=\"width:40px\"></colgroup>");
        for col in 1..=max_col {
            let width = col_widths.get(&col).copied().unwrap_or(default_col_width);
            sheets_html.push_str(&format!("<col style=\"width:{:.1}px\">", width));
        }

        // Generate Header Row
        sheets_html.push_str("<thead><tr><th class=\"corner-cell\"");
        if frozen_rows > 0 || frozen_cols > 0 {
            sheets_html.push_str(" style=\"position:sticky;top:0;left:0;z-index:4;\"");
        }
        sheets_html.push_str("></th>");

        for col in 1..=max_col {
            let col_letter = crate::dom_types::col_num_to_letters(col);
            let mut style_attr = String::new();
            if frozen_rows > 0 && col <= frozen_cols {
                let left = frozen_left_offsets.get(&col).unwrap_or(&0.0);
                style_attr = format!(" style=\"position:sticky;top:0;left:{:.1}px;z-index:4;\"", left);
            } else if frozen_rows > 0 {
                style_attr = " style=\"position:sticky;top:0;z-index:3;\"".to_string();
            } else if col <= frozen_cols {
                let left = frozen_left_offsets.get(&col).unwrap_or(&0.0);
                style_attr = format!(" style=\"position:sticky;left:{:.1}px;z-index:3;\"", left);
            }
            sheets_html.push_str(&format!(
                "<th class=\"col-header\" data-path=\"/{}/col[{}]\" {}>{}</th>",
                ws.name, col_letter, style_attr, col_letter
            ));
        }
        sheets_html.push_str("</tr></thead>\n<tbody>\n");

        // Generate Grid Rows
        for row in 1..=max_row {
            let is_row_frozen = frozen_rows > 0 && row <= frozen_rows;
            let mut tr_style = String::new();
            if let Some(h) = row_heights.get(&row) {
                tr_style = format!("height:{:.1}px;", h);
            }
            if is_row_frozen {
                tr_style.push_str("background:#f9f9f9;");
            }
            let tr_style_attr = if tr_style.is_empty() { String::new() } else { format!(" style=\"{}\"", tr_style) };

            sheets_html.push_str(&format!("<tr{}>\n", tr_style_attr));

            // Row index cell
            let mut th_style_attr = String::new();
            if is_row_frozen && frozen_cols > 0 {
                th_style_attr = " style=\"position:sticky;top:0;left:0;z-index:4;\"".to_string();
            } else if is_row_frozen {
                th_style_attr = " style=\"position:sticky;top:0;z-index:3;\"".to_string();
            } else if frozen_cols > 0 {
                let top = frozen_top_offsets.get(&row).unwrap_or(&0.0);
                th_style_attr = format!(" style=\"position:sticky;left:0;z-index:3;\"");
            }
            sheets_html.push_str(&format!(
                "<th class=\"row-header\" data-path=\"/{}/row[{}]\" {}>{}</th>\n",
                ws.name, row, th_style_attr, row
            ));

            for col in 1..=max_col {
                // Check Merge
                if let Some(merge) = merge_map.get(&(row, col)) {
                    if !merge.is_anchor {
                        continue;
                    }
                }

                let cell_ref = format!("{}{}", crate::dom_types::col_num_to_letters(col), row);
                let cell = ws.cells.get(&(row, col));

                // Compile Styles
                let mut inline_styles = Vec::new();

                // Sticky Pane positioning
                if is_row_frozen && col <= frozen_cols {
                    let left = frozen_left_offsets.get(&col).unwrap_or(&0.0);
                    let top = frozen_top_offsets.get(&row).unwrap_or(&0.0);
                    inline_styles.push(format!("position:sticky;top:{:.1}px;left:{:.1}px;z-index:3", top, left));
                } else if is_row_frozen {
                    let top = frozen_top_offsets.get(&row).unwrap_or(&0.0);
                    inline_styles.push(format!("position:sticky;top:{:.1}px;z-index:2", top));
                } else if col <= frozen_cols {
                    let left = frozen_left_offsets.get(&col).unwrap_or(&0.0);
                    inline_styles.push(format!("position:sticky;left:{:.1}px;z-index:2", left));
                }

                let mut class_name = String::new();

                if let Some(c) = cell {
                    if c.value_type == crate::dom_types::CellValueType::Number {
                        class_name = "number".to_string();
                    }

                    // Apply Stylesheet properties
                    if let Some(s_idx) = c.style_index {
                        if s_idx < cell_formats.len() {
                            let format = &cell_formats[s_idx];
                            if let Some(f_id) = format.font_id {
                                if f_id < fonts.len() {
                                    let font = &fonts[f_id];
                                    inline_styles.push(format!("font-family:'{}', sans-serif", font.name));
                                    inline_styles.push(format!("font-size:{:.1}pt", font.size));
                                    if font.bold { inline_styles.push("font-weight:bold".to_string()); }
                                    if font.italic { inline_styles.push("font-style:italic".to_string()); }
                                    if let Some(ref color) = font.color {
                                        inline_styles.push(format!("color:{}", color));
                                    }
                                }
                            }
                            if let Some(fill_id) = format.fill_id {
                                if fill_id < fills.len() {
                                    let fill = &fills[fill_id];
                                    if fill.fill_type == "solid" {
                                        if let Some(ref color) = fill.bg_color {
                                            inline_styles.push(format!("background-color:{}", color));
                                        }
                                    }
                                }
                            }
                            if let Some(border_id) = format.border_id {
                                if border_id < borders.len() {
                                    let border = &borders[border_id];
                                    let map_border = |edge: &Option<(String, String)>, css_edge: &str| -> Option<String> {
                                        let (style, color) = edge.as_ref()?;
                                        let width = match style.as_str() {
                                            "thin" => "1px",
                                            "medium" => "2px",
                                            "double" => "3px double",
                                            "dashed" => "1px dashed",
                                            "dotted" => "1px dotted",
                                            _ => "1px"
                                        };
                                        Some(format!("border-{}:{};", css_edge, format!("{} solid {}", width, color)))
                                    };
                                    if let Some(left) = map_border(&border.left, "left") { inline_styles.push(left); }
                                    if let Some(right) = map_border(&border.right, "right") { inline_styles.push(right); }
                                    if let Some(top) = map_border(&border.top, "top") { inline_styles.push(top); }
                                    if let Some(bottom) = map_border(&border.bottom, "bottom") { inline_styles.push(bottom); }
                                }
                            }
                            if let Some(ref horiz) = format.align_horiz {
                                let alignment = match horiz.as_str() {
                                    "center" => "center",
                                    "right" => "right",
                                    _ => "left"
                                };
                                inline_styles.push(format!("text-align:{}", alignment));
                            }
                            if let Some(ref vert) = format.align_vert {
                                let alignment = match vert.as_str() {
                                    "center" => "middle",
                                    "bottom" => "bottom",
                                    _ => "top"
                                };
                                inline_styles.push(format!("vertical-align:{}", alignment));
                            }
                        }
                    }
                }

                let style_attr = if inline_styles.is_empty() {
                    String::new()
                } else {
                    format!(" style=\"{}\"", inline_styles.join(";"))
                };

                let class_attr = if class_name.is_empty() {
                    String::new()
                } else {
                    format!(" class=\"{}\"", class_name)
                };

                let span_attrs = if let Some(merge) = merge_map.get(&(row, col)) {
                    let mut spans = String::new();
                    if merge.row_span > 1 { spans.push_str(&format!(" rowspan=\"{}\"", merge.row_span)); }
                    if merge.col_span > 1 { spans.push_str(&format!(" colspan=\"{}\"", merge.col_span)); }
                    spans
                } else {
                    String::new()
                };

                if let Some(c) = cell {
                    let text = html_escape(&c.display_value);
                    sheets_html.push_str(&format!(
                        "<td{}{}{} data-path=\"/{}/{}\">{}</td>\n",
                        class_attr, style_attr, span_attrs, ws.name, cell_ref, text
                    ));
                } else {
                    sheets_html.push_str(&format!(
                        "<td{}{}></td>\n",
                        style_attr, span_attrs
                    ));
                }
            }
            sheets_html.push_str("</tr>\n");
        }

        sheets_html.push_str("</tbody>\n</table>\n</div>\n</div>\n");
    }

    Ok(format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Excel Preview</title>
<style>
body {{
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
    margin: 0;
    background: #1a1a2e;
    color: #e0e0e0;
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
}}
.file-title {{
    background: #121225;
    padding: 12px 24px;
    font-size: 14pt;
    font-weight: 600;
    color: #4472c4;
    border-bottom: 1px solid #252545;
}}
.sheet-slider {{
    flex: 1;
    position: relative;
    overflow: hidden;
}}
.sheet-content {{
    position: absolute;
    inset: 0;
    display: none;
    flex-direction: column;
}}
.sheet-content.active {{
    display: flex;
}}
.table-wrapper {{
    flex: 1;
    overflow: auto;
    background: #1e1e38;
}}
table {{
    border-collapse: collapse;
    table-layout: fixed;
    background: #1e1e38;
    color: #ddd;
    font-size: 9.5pt;
}}
th, td {{
    border: 1px solid #2f2f55;
    padding: 4px 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}}
th {{
    background: #161630;
    color: #888;
    font-weight: 500;
    text-align: center;
    user-select: none;
}}
.col-header, .row-header {{
    font-size: 8.5pt;
}}
.col-header {{ height: 26px; }}
.row-header {{ width: 40px; text-align: center; }}
.corner-cell {{
    width: 40px;
    height: 26px;
}}
td {{
    background: #1e1e38;
    text-align: left;
    vertical-align: middle;
}}
td.number {{
    text-align: right;
}}
.sheet-tabs {{
    background: #121225;
    border-top: 1px solid #252545;
    display: flex;
    padding: 0 12px;
    gap: 4px;
    overflow-x: auto;
    height: 38px;
    align-items: flex-end;
}}
.sheet-tab {{
    padding: 6px 20px;
    background: #1e1e38;
    border: 1px solid #252545;
    border-bottom: none;
    border-radius: 4px 4px 0 0;
    color: #888;
    font-size: 11px;
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
    user-select: none;
    outline: none;
}}
.sheet-tab:hover {{
    background: #25254b;
    color: #ccc;
}}
.sheet-tab.active {{
    background: #4472c4;
    color: #ffffff;
    border-color: #4472c4;
    font-weight: 600;
}}
</style>
</head>
<body>
<div class="file-title">Workbook Preview</div>
<div class="sheet-slider">
{}
</div>
<div class="sheet-tabs" role="tablist">
{}
</div>

<script>
function switchSheet(idx) {{
    document.querySelectorAll('.sheet-content').forEach((el, i) => {{
        el.classList.toggle('active', i === idx);
    }});
    document.querySelectorAll('.sheet-tab').forEach((el, i) => {{
        el.classList.toggle('active', i === idx);
    }});
}}
</script>
</body>
</html>"#,
        sheets_html,
        model.sheets.iter().enumerate()
            .map(|(i, ws)| format!(
                "<button class=\"sheet-tab{}\" onclick=\"switchSheet({})\">{}</button>",
                if i == 0 { " active" } else { "" }, i, ws.name
            ))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn parse_range(range_ref: &str) -> Option<(crate::dom_types::CellRef, crate::dom_types::CellRef)> {
    if !range_ref.contains(':') {
        return None;
    }
    let parts: Vec<&str> = range_ref.split(':').collect();
    let start = crate::dom_types::CellRef::parse(parts[0])?;
    let end = crate::dom_types::CellRef::parse(parts[1])?;
    Some((start, end))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}