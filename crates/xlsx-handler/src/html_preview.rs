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
    left: Option<(String, String)>, // (style, color)
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
    num_fmt_id: Option<usize>,
}

// 64 Default indexed colors for Excel compatibility
const DEFAULT_INDEXED_COLORS: &[&str] = &[
    "000000", "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "000000",
    "FFFFFF", "FF0000", "00FF00", "0000FF", "FFFF00", "FF00FF", "00FFFF", "800000", "008000",
    "000080", "808000", "800080", "008080", "C0C0C0", "808080", "9999FF", "993366", "FFFFCC",
    "CCFFFF", "660066", "FF8080", "0066CC", "CCCCFF", "000080", "FF00FF", "FFFF00", "00FFFF",
    "800080", "800000", "008080", "0000FF", "00CCFF", "CCFFFF", "CCFFCC", "FFFF99", "99CCFF",
    "FF99CC", "CC99FF", "FFCC99", "3366FF", "33CCCC", "99CC00", "FFCC00", "FF9900", "FF6600",
    "666699", "969696", "2A6F97", "014F86", "012A4A", "A9D6E5", "89C2D9", "61A5C2", "468FAF",
    "2C7DA0",
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
    } else if let Some(indexed) = color_node
        .attribute("indexed")
        .and_then(|s| s.parse::<usize>().ok())
    {
        if indexed < indexed_colors.len() {
            base_hex = Some(indexed_colors[indexed].to_string());
        }
    } else if let Some(theme) = color_node
        .attribute("theme")
        .and_then(|s| s.parse::<usize>().ok())
    {
        let theme_names = [
            "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5",
            "accent6",
        ];
        if theme < theme_names.len() {
            if let Some(hex) = theme_colors.get(theme_names[theme]) {
                base_hex = Some(hex.to_string());
            }
        }
    }

    if let Some(hex) = base_hex {
        let tint = color_node
            .attribute("tint")
            .and_then(|s| s.parse::<f64>().ok());
        return Some(apply_transforms(&hex, tint));
    }
    None
}

/// Render the Excel workbook as HTML for browser preview.
pub fn view_as_html(package: &OxmlPackage) -> Result<String, HandlerError> {
    let model =
        crate::helpers::build_workbook_model(package).map_err(HandlerError::OperationFailed)?;

    // 1. Resolve Theme Colors
    let mut theme_colors = HashMap::new();
    if let Ok(theme_xml) = package.read_part_xml("xl/theme/theme1.xml") {
        if let Ok(doc) = roxmltree::Document::parse(&theme_xml) {
            if let Some(scheme) = doc.descendants().find(|n| n.has_tag_name("clrScheme")) {
                let clr_elements = [
                    "dk1", "lt1", "dk2", "lt2", "accent1", "accent2", "accent3", "accent4",
                    "accent5", "accent6",
                ];
                for elem in clr_elements {
                    if let Some(color_node) = scheme.descendants().find(|n| n.has_tag_name(elem)) {
                        if let Some(srgb) =
                            color_node.descendants().find(|n| n.has_tag_name("srgbClr"))
                        {
                            if let Some(val) = srgb.attribute("val") {
                                theme_colors.insert(elem.to_string(), val.to_string());
                            }
                        } else if let Some(sys) =
                            color_node.descendants().find(|n| n.has_tag_name("sysClr"))
                        {
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
    let indexed_colors = DEFAULT_INDEXED_COLORS.to_vec();
    let mut fonts = Vec::new();
    let mut fills = Vec::new();
    let mut borders = Vec::new();
    let mut cell_formats = Vec::new();
    let mut custom_num_fmts = HashMap::new();

    if let Ok(styles_xml) = package.read_part_xml("xl/styles.xml") {
        if let Ok(doc) = roxmltree::Document::parse(&styles_xml) {
            // Read Custom Numbering Formats
            if let Some(num_fmts_node) = doc.descendants().find(|n| n.has_tag_name("numFmts")) {
                for num_fmt in num_fmts_node.children().filter(|n| n.has_tag_name("numFmt")) {
                    let id_str = num_fmt.attribute("numFmtId").unwrap_or("");
                    let code_str = num_fmt.attribute("formatCode").unwrap_or("");
                    if let Ok(id) = id_str.parse::<usize>() {
                        custom_num_fmts.insert(id, code_str.to_string());
                    }
                }
            }
            // Read Custom Palette
            if let Some(colors_node) = doc.descendants().find(|n| n.has_tag_name("colors")) {
                if let Some(idx_node) = colors_node
                    .descendants()
                    .find(|n| n.has_tag_name("indexedColors"))
                {
                    // Overwrite indexed palette
                    for (idx, rgb_node) in idx_node
                        .children()
                        .filter(|n| n.has_tag_name("rgbColor"))
                        .enumerate()
                    {
                        if let Some(rgb) = rgb_node.attribute("rgb") {
                            let _clean = rgb.trim_start_matches("FF").to_string();
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
                    let name = f_node
                        .children()
                        .find(|n| n.has_tag_name("name"))
                        .and_then(|n| n.attribute("val"))
                        .unwrap_or("Calibri")
                        .to_string();
                    let size = f_node
                        .children()
                        .find(|n| n.has_tag_name("sz"))
                        .and_then(|n| n.attribute("val"))
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(11.0);
                    let bold = f_node.children().any(|n| n.has_tag_name("b"));
                    let italic = f_node.children().any(|n| n.has_tag_name("i"));
                    let color = f_node
                        .children()
                        .find(|n| n.has_tag_name("color"))
                        .and_then(|c| resolve_xml_color(&c, &theme_colors, &indexed_colors));
                    fonts.push(ParsedFont {
                        name,
                        size,
                        bold,
                        italic,
                        color,
                    });
                }
            }

            // Parse Fills
            if let Some(fills_node) = doc.descendants().find(|n| n.has_tag_name("fills")) {
                for fill in fills_node.children().filter(|n| n.has_tag_name("fill")) {
                    if let Some(pattern) = fill.children().find(|n| n.has_tag_name("patternFill")) {
                        let fill_type = pattern
                            .attribute("patternType")
                            .unwrap_or("none")
                            .to_string();
                        let bg_color = pattern
                            .children()
                            .find(|n| n.has_tag_name("fgColor"))
                            .and_then(|c| resolve_xml_color(&c, &theme_colors, &indexed_colors));
                        fills.push(ParsedFill {
                            fill_type,
                            bg_color,
                        });
                    } else {
                        fills.push(ParsedFill {
                            fill_type: "none".to_string(),
                            bg_color: None,
                        });
                    }
                }
            }

            // Parse Borders
            if let Some(borders_node) = doc.descendants().find(|n| n.has_tag_name("borders")) {
                for b_node in borders_node.children().filter(|n| n.has_tag_name("border")) {
                    let parse_edge = |edge_name: &str| -> Option<(String, String)> {
                        let edge = b_node.children().find(|n| n.has_tag_name(edge_name))?;
                        let style = edge.attribute("style")?.to_string();
                        let color = edge
                            .children()
                            .find(|n| n.has_tag_name("color"))
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
                    let border_id = xf
                        .attribute("borderId")
                        .and_then(|s| s.parse::<usize>().ok());
                    let num_fmt_id = xf
                        .attribute("numFmtId")
                        .and_then(|s| s.parse::<usize>().ok());
                    let align = xf.children().find(|n| n.has_tag_name("alignment"));
                    let align_horiz = align
                        .as_ref()
                        .and_then(|a| a.attribute("horizontal"))
                        .map(|s| s.to_string());
                    let align_vert = align
                        .as_ref()
                        .and_then(|a| a.attribute("vertical"))
                        .map(|s| s.to_string());
                    cell_formats.push(CellFormat {
                        font_id,
                        fill_id,
                        border_id,
                        align_horiz,
                        align_vert,
                        num_fmt_id,
                    });
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
        let mut hidden_rows = std::collections::HashSet::new();
        let mut hidden_cols = std::collections::HashSet::new();

        if let Ok(ws_xml) = package.read_part_xml(&ws.part_path) {
            if let Ok(ws_doc) = roxmltree::Document::parse(&ws_xml) {
                // Parse Merge Cells
                if let Some(merge_cells) =
                    ws_doc.descendants().find(|n| n.has_tag_name("mergeCells"))
                {
                    for mc in merge_cells
                        .children()
                        .filter(|n| n.has_tag_name("mergeCell"))
                    {
                        if let Some(range_ref) = mc.attribute("ref") {
                            if let Some((start, end)) = parse_range(range_ref) {
                                for r in start.row..=end.row {
                                    for c in start.col..=end.col {
                                        let is_anchor = r == start.row && c == start.col;
                                        let row_span = end.row - start.row + 1;
                                        let col_span = end.col - start.col + 1;
                                        merge_map.insert(
                                            (r, c),
                                            MergeInfo {
                                                is_anchor,
                                                row_span,
                                                col_span,
                                            },
                                        );
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
                            frozen_rows = pane
                                .attribute("ySplit")
                                .and_then(|s| s.parse::<usize>().ok())
                                .unwrap_or(0);
                            frozen_cols = pane
                                .attribute("xSplit")
                                .and_then(|s| s.parse::<usize>().ok())
                                .unwrap_or(0);
                        }
                    }
                }

                // Parse Format Props
                if let Some(fmt_pr) = ws_doc
                    .descendants()
                    .find(|n| n.has_tag_name("sheetFormatPr"))
                {
                    default_col_width = fmt_pr
                        .attribute("defaultColWidth")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(8.43)
                        * 7.5;
                    default_row_height = fmt_pr
                        .attribute("defaultRowHeight")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(15.0);
                }

                // Parse Explicit Col Widths and Visibility
                if let Some(cols_node) = ws_doc.descendants().find(|n| n.has_tag_name("cols")) {
                    for col in cols_node.children().filter(|n| n.has_tag_name("col")) {
                        let min = col
                            .attribute("min")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(1);
                        let max = col
                            .attribute("max")
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(1);
                        let is_hidden = col.attribute("hidden") == Some("1");
                        if is_hidden {
                            for c in min..=max {
                                hidden_cols.insert(c);
                            }
                        }
                        if let Some(w) = col.attribute("width").and_then(|s| s.parse::<f64>().ok())
                        {
                            let width_px = if is_hidden || w <= 0.0 {
                                0.0
                            } else {
                                w * 7.5
                            };
                            for c in min..=max {
                                col_widths.insert(c, width_px);
                                if width_px <= 0.0 {
                                    hidden_cols.insert(c);
                                }
                            }
                        }
                    }
                }

                // Parse Explicit Row Heights and Visibility
                if let Some(sheet_data) = ws_doc.descendants().find(|n| n.has_tag_name("sheetData"))
                {
                    for row in sheet_data.children().filter(|n| n.has_tag_name("row")) {
                        if let Some(r_idx) =
                            row.attribute("r").and_then(|s| s.parse::<usize>().ok())
                        {
                            if let Some(h) = row.attribute("ht").and_then(|s| s.parse::<f64>().ok())
                            {
                                row_heights.insert(r_idx, h);
                            }
                            if row.attribute("hidden") == Some("1") {
                                hidden_rows.insert(r_idx);
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
            if hidden_cols.contains(&c) {
                continue;
            }
            frozen_left_offsets.insert(c, current_left);
            let w = col_widths.get(&c).copied().unwrap_or(default_col_width);
            current_left += w;
        }

        let mut frozen_top_offsets = HashMap::new();
        let mut current_top = 26.0; // header height
        for r in 1..=frozen_rows {
            if hidden_rows.contains(&r) {
                continue;
            }
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
            if hidden_cols.contains(&col) {
                continue;
            }
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
            if hidden_cols.contains(&col) {
                continue;
            }
            let col_letter = crate::dom_types::col_num_to_letters(col);
            let mut style_attr = String::new();
            if frozen_rows > 0 && col <= frozen_cols {
                let left = frozen_left_offsets.get(&col).unwrap_or(&0.0);
                style_attr = format!(
                    " style=\"position:sticky;top:0;left:{:.1}px;z-index:4;\"",
                    left
                );
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
            if hidden_rows.contains(&row) {
                sheets_html.push_str(&format!("<tr style=\"display:none\">\n<th class=\"row-header\">{}</th>\n</tr>\n", row));
                continue;
            }
            let is_row_frozen = frozen_rows > 0 && row <= frozen_rows;
            let mut tr_style = String::new();
            if let Some(h) = row_heights.get(&row) {
                tr_style = format!("height:{:.1}px;", h);
            }
            if is_row_frozen {
                tr_style.push_str("background:#f9f9f9;");
            }
            let tr_style_attr = if tr_style.is_empty() {
                String::new()
            } else {
                format!(" style=\"{}\"", tr_style)
            };
            let frozen_attr = if is_row_frozen { " data-frozen" } else { "" };
            sheets_html.push_str(&format!("<tr{}{}>\n", frozen_attr, tr_style_attr));

            // Row index cell
            let mut th_style_attr = String::new();
            if is_row_frozen && frozen_cols > 0 {
                th_style_attr = " style=\"position:sticky;top:0;left:0;z-index:4;\"".to_string();
            } else if is_row_frozen {
                th_style_attr = " style=\"position:sticky;top:0;z-index:3;\"".to_string();
            } else if frozen_cols > 0 {
                th_style_attr = " style=\"position:sticky;left:0;z-index:3;\"".to_string();
            }
            sheets_html.push_str(&format!(
                "<th class=\"row-header\" data-path=\"/{}/row[{}]\" {}>{}</th>\n",
                ws.name, row, th_style_attr, row
            ));

            for col in 1..=max_col {
                if hidden_cols.contains(&col) {
                    continue;
                }
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
                    inline_styles.push(format!(
                        "position:sticky;top:{:.1}px;left:{:.1}px;z-index:3",
                        top, left
                    ));
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
                                    inline_styles
                                        .push(format!("font-family:'{}', sans-serif", font.name));
                                    inline_styles.push(format!("font-size:{:.1}pt", font.size));
                                    if font.bold {
                                        inline_styles.push("font-weight:bold".to_string());
                                    }
                                    if font.italic {
                                        inline_styles.push("font-style:italic".to_string());
                                    }
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
                                            inline_styles
                                                .push(format!("background-color:{}", color));
                                        }
                                    }
                                }
                            }
                            if let Some(border_id) = format.border_id {
                                if border_id < borders.len() {
                                    let border = &borders[border_id];
                                    let map_border =
                                        |edge: &Option<(String, String)>,
                                         css_edge: &str|
                                         -> Option<String> {
                                            let (style, color) = edge.as_ref()?;
                                            let width = match style.as_str() {
                                                "thin" => "1px",
                                                "medium" => "2px",
                                                "double" => "3px double",
                                                "dashed" => "1px dashed",
                                                "dotted" => "1px dotted",
                                                _ => "1px",
                                            };
                                            Some(format!(
                                                "border-{}:{} solid {};",
                                                css_edge, width, color
                                            ))
                                        };
                                    if let Some(left) = map_border(&border.left, "left") {
                                        inline_styles.push(left);
                                    }
                                    if let Some(right) = map_border(&border.right, "right") {
                                        inline_styles.push(right);
                                    }
                                    if let Some(top) = map_border(&border.top, "top") {
                                        inline_styles.push(top);
                                    }
                                    if let Some(bottom) = map_border(&border.bottom, "bottom") {
                                        inline_styles.push(bottom);
                                    }
                                }
                            }
                            if let Some(ref horiz) = format.align_horiz {
                                let alignment = match horiz.as_str() {
                                    "center" => "center",
                                    "right" => "right",
                                    _ => "left",
                                };
                                inline_styles.push(format!("text-align:{}", alignment));
                            }
                            if let Some(ref vert) = format.align_vert {
                                let alignment = match vert.as_str() {
                                    "center" => "middle",
                                    "bottom" => "bottom",
                                    _ => "top",
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
                    if merge.row_span > 1 {
                        spans.push_str(&format!(" rowspan=\"{}\"", merge.row_span));
                    }
                    let mut adj_col_span = merge.col_span;
                    if adj_col_span > 1 && !hidden_cols.is_empty() {
                        for hc in (col + 1)..(col + merge.col_span) {
                            if hidden_cols.contains(&hc) {
                                adj_col_span -= 1;
                            }
                        }
                    }
                    if adj_col_span > 1 {
                        spans.push_str(&format!(" colspan=\"{}\"", adj_col_span));
                    }
                    spans
                } else {
                    String::new()
                };

                if let Some(c) = cell {
                    let num_fmt_id = c.style_index
                        .and_then(|s_idx| cell_formats.get(s_idx))
                        .and_then(|fmt| fmt.num_fmt_id)
                        .unwrap_or(0);
                    let raw_val = c.raw_value.as_deref().unwrap_or(&c.display_value);
                    let formatted_val = format_cell_value(raw_val, num_fmt_id, &custom_num_fmts);
                    let text = html_escape(&formatted_val);
                    sheets_html.push_str(&format!(
                        "<td{}{}{} data-path=\"/{}/{}\">{}</td>\n",
                        class_attr, style_attr, span_attrs, ws.name, cell_ref, text
                    ));
                } else {
                    sheets_html.push_str(&format!("<td{}{}></td>\n", style_attr, span_attrs));
                }
            }
            sheets_html.push_str("</tr>\n");
        }

        sheets_html.push_str("</tbody>\n</table>\n</div>\n</div>\n");
    }

    let mut tabs_html = String::new();
    for (i, ws) in model.sheets.iter().enumerate() {
        let mut tab_color_style = String::new();
        if let Ok(sheet_xml) = package.read_part_xml(&ws.part_path) {
            if let Ok(sheet_doc) = roxmltree::Document::parse(&sheet_xml) {
                if let Some(sheet_pr) = sheet_doc.descendants().find(|n| n.has_tag_name("sheetPr")) {
                    if let Some(tab_color_el) = sheet_pr.children().find(|n| n.has_tag_name("tabColor")) {
                        if let Some(rgb) = tab_color_el.attribute("rgb") {
                            let clean = if rgb.len() == 8 { &rgb[2..] } else { rgb };
                            tab_color_style = format!(" style=\"--tab-color:#{};\"", clean);
                        }
                    }
                }
            }
        }
        let active_class = if i == 0 { " active" } else { "" };
        tabs_html.push_str(&format!(
            "  <div class=\"sheet-tab{}\"{} data-sheet=\"{}\" role=\"tab\" tabindex=\"0\" onclick=\"switchSheet({})\" onkeydown=\"if(event.key==='Enter'||event.key===' ')switchSheet({})\">{}</div>\n",
            active_class,
            tab_color_style,
            i,
            i,
            i,
            html_escape(&ws.name)
        ));
    }

    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Excel Preview</title>
<style>
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
html, body {{ height: 100%; }}
body {{
    font-family: 'Segoe UI', -apple-system, BlinkMacSystemFont, sans-serif;
    background: #f0f0f0;
    color: #333;
    display: flex;
    flex-direction: column;
    min-height: 100vh;
}}
.file-title {{
    padding: 12px 20px;
    font-size: 14px;
    font-weight: 600;
    background: #217346;
    color: #fff;
}}
.sheet-tabs {{
    display: flex;
    background: #e0e0e0;
    border-top: 1px solid #ccc;
    overflow-x: auto;
    padding: 0 8px;
    flex-shrink: 0;
    position: sticky;
    bottom: 0;
    z-index: 10;
}}
.sheet-tab {{
    --tab-color: #e8e8e8;
    padding: 8px 16px;
    font-size: 12px;
    cursor: pointer;
    border: 1px solid #bbb;
    border-top: none;
    background: var(--tab-color);
    color: #fff;
    margin-bottom: 0;
    border-radius: 0 0 3px 3px;
    white-space: nowrap;
    user-select: none;
    position: relative;
    transition: background 0.15s, color 0.15s;
}}
.sheet-tab[style*="--tab-color:#e8e8e8"], .sheet-tab:not([style*="--tab-color"]) {{
    color: #333;
}}
.sheet-tab:hover {{ opacity: 0.85; }}
.sheet-tab.active {{
    background: linear-gradient(to bottom, #fff 60%, color-mix(in srgb, var(--tab-color) 30%, #fff)) !important;
    color: #333 !important;
    border-color: #aaa;
    border-bottom: 3px solid var(--tab-color);
    font-weight: 600;
}}
.sheet-slider {{ flex: 1; position: relative; overflow: hidden; display: flex; flex-direction: column; min-height: 0; }}
.sheet-content {{ background: #fff; display: none; flex: 1; min-height: 0; }}
.sheet-content.active {{ display: flex; flex-direction: column; }}
.table-wrapper {{
    flex: 1;
    overflow: auto;
    min-height: 0;
    background: #fff;
}}
table {{
    border-collapse: collapse;
    font-size: 11px;
    font-family: 'Segoe UI', -apple-system, BlinkMacSystemFont, sans-serif;
    table-layout: fixed;
}}
.row-header-col {{ width: 30pt; }}
th {{
    background: #f8f8f8;
    border: 1px solid #e0e0e0;
    font-weight: normal;
    color: #666;
    font-size: 10px;
    text-align: center;
    padding: 2px 4px;
}}
.corner-cell {{ background: #f0f0f0; z-index: 4; }}
.col-header {{
    position: sticky;
    top: 0;
    z-index: 3;
    background: #f8f8f8;
    min-width: 50px;
    cursor: s-resize;
}}
.row-header {{
    position: sticky;
    left: 0;
    z-index: 2;
    background: #f8f8f8;
    min-width: 40px;
    cursor: e-resize;
    border-right: none;
}}
td {{
    box-shadow: inset -1px -1px 0 #e0e0e0;
    padding: 2px 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    vertical-align: bottom;
    max-width: 500px;
    word-break: break-all;
}}
tbody tr:first-child td {{ box-shadow: inset -1px -1px 0 #e0e0e0, inset 0 1px 0 #e0e0e0; }}
tr td:first-of-type {{ box-shadow: inset -1px -1px 0 #e0e0e0, inset 1px 0 0 #e0e0e0; }}
tbody tr:first-child td:first-of-type {{ box-shadow: inset -1px -1px 0 #e0e0e0, inset 1px 1px 0 #e0e0e0; }}
.empty-sheet {{
    padding: 40px;
    text-align: center;
    color: #999;
    font-size: 14px;
}}
.chart-container {{
    margin: 16px auto;
    background: #fff;
    border: 1px solid #e0e0e0;
    border-radius: 6px;
    padding: 12px;
    box-shadow: 0 1px 3px rgba(0,0,0,0.08);
}}
.chart-container svg {{ display: block; }}
.truncation-warning {{
    padding: 8px 16px;
    background: #FFF3CD;
    color: #856404;
    border: 1px solid #FFEEBA;
    font-size: 12px;
    text-align: center;
    margin: 4px 0;
}}
.sr-only {{ position:absolute; clip:rect(0 0 0 0); width:1px; height:1px; overflow:hidden; }}
@media print {{
    .file-title, .sheet-tabs {{ display: none !important; }}
    .table-wrapper {{ max-height: none !important; overflow: visible !important; flex: none !important; }}
    body {{ background: #fff !important; min-height: auto !important; }}
    .sheet-content {{ display: block !important; flex: none !important; }}
    td {{ max-width: none !important; white-space: normal !important; overflow: visible !important; }}
}}
</style>
</head>
<body>
<div class="file-title">Workbook Preview</div>
<div class="sheet-slider">
{sheets_html}</div>
<div class="sheet-tabs" role="tablist">
{tabs_html}</div>

<script>
function switchSheet(idx) {{
    document.querySelectorAll('.sheet-tab').forEach(function(t) {{
        t.classList.toggle('active', parseInt(t.getAttribute('data-sheet')) === idx);
    }});
    document.querySelectorAll('.sheet-content').forEach(function(c) {{
        c.classList.toggle('active', parseInt(c.getAttribute('data-sheet')) === idx);
    }});
    window.scrollTo(0, 0);
    adjustStickyHeights();
}}
function adjustStickyHeights() {{
    document.querySelectorAll('.sheet-content.active .table-wrapper table').forEach(function(table) {{
        var thead = table.querySelector('thead');
        if (!thead) return;
        var theadH = thead.offsetHeight;
        var cumTop = theadH;
        var frozen = table.querySelectorAll('tr[data-frozen]');
        frozen.forEach(function(tr) {{
            tr.querySelectorAll('th, td').forEach(function(cell) {{
                if (cell.style.position === 'sticky') cell.style.top = cumTop + 'px';
            }});
            cumTop += tr.offsetHeight;
        }});
    }});
}}
// Initial run
adjustStickyHeights();
</script>
</body>
</html>"#,
        sheets_html = sheets_html,
        tabs_html = tabs_html
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

fn oa_date_to_date(oa_date: f64) -> Option<(i32, i32, i32, i32, i32, i32)> {
    if oa_date < 0.0 || oa_date > 2958465.0 {
        return None;
    }
    let days = oa_date.floor() as i32;
    let frac = oa_date - oa_date.floor();

    let (y, m, d) = if days == 60 {
        (1900, 2, 29)
    } else {
        let mut adj_days = days;
        if days > 60 {
            adj_days -= 1;
        }
        let mut y = 1900;
        let mut d_left = adj_days - 1;
        loop {
            let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
            let days_in_year = if is_leap { 366 } else { 365 };
            if d_left >= days_in_year {
                d_left -= days_in_year;
                y += 1;
            } else {
                break;
            }
        }
        let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        let month_days = if is_leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut m = 1;
        for &md in month_days.iter() {
            if d_left >= md {
                d_left -= md;
                m += 1;
            } else {
                break;
            }
        }
        (y, m, d_left + 1)
    };

    let total_seconds = (frac * 86400.0 + 0.5).floor() as i32;
    let hour = total_seconds / 3600;
    let minute = (total_seconds % 3600) / 60;
    let second = total_seconds % 60;

    Some((y, m, d, hour, minute, second))
}

fn is_date_format(num_fmt_id: usize, format_code: Option<&str>) -> bool {
    let built_in_dates = [14, 15, 16, 17, 18, 19, 20, 21, 22, 45, 46, 47];
    if built_in_dates.contains(&num_fmt_id) {
        return true;
    }
    if let Some(code) = format_code {
        let code_lower = code.to_lowercase();
        let mut stripped = String::new();
        let mut in_bracket = false;
        let mut in_quote = false;
        for c in code_lower.chars() {
            if c == '"' {
                in_quote = !in_quote;
            } else if c == '[' && !in_quote {
                in_bracket = true;
            } else if c == ']' && !in_quote {
                in_bracket = false;
            } else if !in_bracket && !in_quote {
                stripped.push(c);
            }
        }
        stripped.contains('y') || stripped.contains('d') || stripped.contains('m')
    } else {
        false
    }
}

fn format_date(value: f64, format_code: Option<&str>) -> String {
    if let Some((y, m, d, hh, mm, ss)) = oa_date_to_date(value) {
        let mut has_time = false;
        if let Some(code) = format_code {
            let code_lower = code.to_lowercase();
            let mut stripped = String::new();
            let mut in_bracket = false;
            let mut in_quote = false;
            for c in code_lower.chars() {
                if c == '"' {
                    in_quote = !in_quote;
                } else if c == '[' && !in_quote {
                    in_bracket = true;
                } else if c == ']' && !in_quote {
                    in_bracket = false;
                } else if !in_bracket && !in_quote {
                    stripped.push(c);
                }
            }
            has_time = stripped.contains('h') || stripped.contains('s');
        }

        if has_time {
            if ss == 0 {
                format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, hh, mm)
            } else {
                format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, hh, mm, ss)
            }
        } else {
            format!("{:04}-{:02}-{:02}", y, m, d)
        }
    } else {
        value.to_string()
    }
}

fn is_percent_format(format_code: Option<&str>) -> bool {
    if let Some(code) = format_code {
        let mut in_quote = false;
        for c in code.chars() {
            if c == '"' {
                in_quote = !in_quote;
            } else if c == '%' && !in_quote {
                return true;
            }
        }
    }
    false
}

fn format_percent(value: f64, format_code: &str) -> String {
    let mut decimals = 0;
    if let Some(dot_idx) = format_code.find('.') {
        if let Some(pct_idx) = format_code.find('%') {
            if pct_idx > dot_idx {
                let sub = &format_code[dot_idx + 1..pct_idx];
                decimals = sub.chars().filter(|&c| c == '0' || c == '#').count();
            }
        }
    }
    format!("{:.*}%", decimals, value * 100.0)
}

fn get_format_code(num_fmt_id: usize, custom_num_fmts: &HashMap<usize, String>) -> Option<String> {
    if let Some(custom) = custom_num_fmts.get(&num_fmt_id) {
        return Some(custom.clone());
    }
    let built_in = match num_fmt_id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    };
    Some(built_in.to_string())
}

fn format_cell_value(value_str: &str, num_fmt_id: usize, custom_num_fmts: &HashMap<usize, String>) -> String {
    if let Ok(val) = value_str.parse::<f64>() {
        let format_code = get_format_code(num_fmt_id, custom_num_fmts);
        let format_ref = format_code.as_deref();
        if is_date_format(num_fmt_id, format_ref) {
            return format_date(val, format_ref);
        }
        if is_percent_format(format_ref) {
            return format_percent(val, format_ref.unwrap());
        }
        if let Some(code) = format_ref {
            if code == "0.00" {
                return format!("{:.2}", val);
            } else if code == "0" {
                return format!("{:.0}", val);
            }
        }
    }
    value_str.to_string()
}
