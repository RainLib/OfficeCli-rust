//! Bounded, package-independent rendering for cached OOXML chart data.
//!
//! The renderer intentionally consumes only the chart part XML. Callers remain
//! responsible for resolving worksheet -> drawing -> chart relationships.
//! Cached series values are sufficient for a useful offline preview and keep
//! the implementation independent from Excel/LibreOffice.

const WIDTH: f64 = 640.0;
const HEIGHT: f64 = 360.0;
const LEFT: f64 = 54.0;
const TOP: f64 = 48.0;
const RIGHT: f64 = 22.0;
const BOTTOM: f64 = 62.0;
const MAX_SERIES: usize = 32;
const MAX_POINTS: usize = 2_048;
const PALETTE: [&str; 10] = [
    "#4472C4", "#ED7D31", "#A5A5A5", "#FFC000", "#5B9BD5", "#70AD47", "#264478", "#9E480E",
    "#636363", "#997300",
];

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSeries {
    pub name: String,
    pub name_formula: Option<String>,
    pub categories: Vec<String>,
    pub categories_formula: Option<String>,
    pub x_values: Vec<f64>,
    pub x_values_formula: Option<String>,
    pub values: Vec<f64>,
    pub values_formula: Option<String>,
    pub bubble_sizes: Vec<f64>,
    pub bubble_sizes_formula: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartPreview {
    pub title: String,
    pub chart_kinds: Vec<String>,
    pub series: Vec<ChartSeries>,
}

pub fn parse_chart_preview(xml: &str) -> Result<ChartPreview, String> {
    if xml.len() > 16 * 1024 * 1024 {
        return Err("chart XML exceeds 16 MiB preview limit".to_string());
    }
    let doc = roxmltree::Document::parse(xml).map_err(|error| error.to_string())?;
    let chart = doc
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "chart")
        .ok_or_else(|| "chart part has no chart element".to_string())?;
    let title = chart
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "title")
        .map(collect_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Chart".to_string());
    let plot_area = chart
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "plotArea")
        .ok_or_else(|| "chart part has no plotArea".to_string())?;
    let mut chart_kinds = Vec::new();
    let mut series = Vec::new();
    for chart_node in plot_area.children().filter(|node| {
        node.is_element()
            && node.tag_name().name().ends_with("Chart")
            && node.tag_name().name() != "chart"
    }) {
        let kind = chart_node.tag_name().name().trim_end_matches("Chart");
        if !chart_kinds.iter().any(|existing| existing == kind) {
            chart_kinds.push(kind.to_string());
        }
        for series_node in chart_node
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "ser")
        {
            if series.len() >= MAX_SERIES {
                break;
            }
            series.push(parse_series(series_node, series.len()));
        }
    }
    if chart_kinds.is_empty() {
        parse_extended_chart(chart, plot_area, &mut chart_kinds, &mut series);
    }
    Ok(ChartPreview {
        title,
        chart_kinds,
        series,
    })
}

pub fn render_chart_svg(xml: &str) -> Result<String, String> {
    let chart = parse_chart_preview(xml)?;
    Ok(render_preview_svg(&chart))
}

pub fn render_preview_svg(chart: &ChartPreview) -> String {
    let mut body = String::new();
    body.push_str(&format!(
        "<rect width=\"{WIDTH}\" height=\"{HEIGHT}\" fill=\"#fff\"/><text x=\"{:.1}\" y=\"27\" text-anchor=\"middle\" font-family=\"Arial,sans-serif\" font-size=\"17\" font-weight=\"600\" fill=\"#222\">{}</text>",
        WIDTH / 2.0,
        escape_xml(&chart.title)
    ));
    if chart.series.is_empty() {
        body.push_str("<rect x=\"54\" y=\"48\" width=\"564\" height=\"250\" fill=\"#fafafa\" stroke=\"#d9d9d9\"/><text x=\"320\" y=\"178\" text-anchor=\"middle\" font-family=\"Arial,sans-serif\" font-size=\"14\" fill=\"#777\">No cached chart data</text>");
    } else {
        let primary = chart
            .chart_kinds
            .first()
            .map(String::as_str)
            .unwrap_or("line");
        match primary {
            "pie" | "doughnut" | "ofPie" => render_pie(chart, &mut body),
            "radar" => render_radar(chart, &mut body),
            "boxWhisker" => render_box_whisker(chart, &mut body),
            "bar" => render_bars(chart, &mut body),
            "scatter" | "bubble" => render_cartesian(chart, &mut body, true),
            _ => render_cartesian(chart, &mut body, false),
        }
        render_legend(chart, &mut body);
    }
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {WIDTH} {HEIGHT}\" role=\"img\" aria-label=\"{}\">{body}</svg>",
        escape_xml(&chart.title)
    )
}

fn parse_extended_chart(
    chart: roxmltree::Node<'_, '_>,
    plot_area: roxmltree::Node<'_, '_>,
    chart_kinds: &mut Vec<String>,
    series: &mut Vec<ChartSeries>,
) {
    let Some(chart_space) = chart
        .ancestors()
        .find(|node| node.is_element() && node.tag_name().name() == "chartSpace")
    else {
        return;
    };
    let mut data_sets = std::collections::HashMap::<String, (Vec<String>, Vec<f64>)>::new();
    for data in chart_space
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "data")
    {
        let Some(id) = data.attribute("id") else {
            continue;
        };
        let categories = data
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "strDim")
            .map(extended_points)
            .unwrap_or_default();
        let values = data
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "numDim")
            .map(extended_points)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .take(MAX_POINTS)
            .collect::<Vec<_>>();
        data_sets.insert(id.to_string(), (categories, values));
    }
    for series_node in plot_area
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "series")
        .take(MAX_SERIES)
    {
        let layout = series_node.attribute("layoutId").unwrap_or("line");
        let kind = match layout {
            "boxWhisker" => "boxWhisker",
            "pie" | "sunburst" => "pie",
            "radar" => "radar",
            "scatter" | "bubble" => "scatter",
            "line" => "line",
            _ => "bar",
        };
        if !chart_kinds.iter().any(|existing| existing == kind) {
            chart_kinds.push(kind.to_string());
        }
        let data_id = series_node
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "dataId")
            .and_then(|node| node.attribute("val"));
        let (mut categories, mut values) = data_id
            .and_then(|id| data_sets.get(id))
            .cloned()
            .unwrap_or_default();
        if series_node
            .descendants()
            .any(|node| node.is_element() && node.tag_name().name() == "binning")
            && !values.is_empty()
        {
            (categories, values) = histogram(&values);
        }
        let name = series_node
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "tx")
            .map(collect_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Series {}", series.len() + 1));
        series.push(ChartSeries {
            name,
            name_formula: None,
            categories,
            categories_formula: None,
            x_values: Vec::new(),
            x_values_formula: None,
            values,
            values_formula: None,
            bubble_sizes: Vec::new(),
            bubble_sizes_formula: None,
        });
    }
}

fn extended_points(container: roxmltree::Node<'_, '_>) -> Vec<String> {
    let mut points = container
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "pt")
        .filter_map(|point| {
            let index = point.attribute("idx")?.parse::<usize>().ok()?;
            Some((index, point.text().unwrap_or_default().to_string()))
        })
        .take(MAX_POINTS)
        .collect::<Vec<_>>();
    points.sort_by_key(|(index, _)| *index);
    points.into_iter().map(|(_, value)| value).collect()
}

fn histogram(values: &[f64]) -> (Vec<String>, Vec<f64>) {
    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !minimum.is_finite() || !maximum.is_finite() || maximum <= minimum {
        return (vec![format!("{minimum:.2}")], vec![values.len() as f64]);
    }
    let bin_count = (values.len() as f64).sqrt().round().clamp(4.0, 24.0) as usize;
    let width = (maximum - minimum) / bin_count as f64;
    let mut counts = vec![0.0; bin_count];
    for value in values {
        let index = (((value - minimum) / width) as usize).min(bin_count - 1);
        counts[index] += 1.0;
    }
    let labels = (0..bin_count)
        .map(|index| {
            let start = minimum + index as f64 * width;
            let end = start + width;
            format!("{start:.1}–{end:.1}")
        })
        .collect();
    (labels, counts)
}

fn parse_series(node: roxmltree::Node<'_, '_>, ordinal: usize) -> ChartSeries {
    let name = node
        .children()
        .find(|child| child.is_element() && child.tag_name().name() == "tx")
        .map(collect_text)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("Series {}", ordinal + 1));
    ChartSeries {
        name,
        name_formula: series_formula(node, "tx"),
        categories: series_strings(node, "cat"),
        categories_formula: series_formula(node, "cat"),
        x_values: series_numbers(node, "xVal"),
        x_values_formula: series_formula(node, "xVal"),
        values: series_numbers(node, "val")
            .into_iter()
            .chain(series_numbers(node, "yVal"))
            .take(MAX_POINTS)
            .collect(),
        values_formula: series_formula(node, "val").or_else(|| series_formula(node, "yVal")),
        bubble_sizes: series_numbers(node, "bubbleSize"),
        bubble_sizes_formula: series_formula(node, "bubbleSize"),
    }
}

fn series_formula(node: roxmltree::Node<'_, '_>, container: &str) -> Option<String> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == container)?
        .descendants()
        .find(|child| child.is_element() && child.tag_name().name() == "f")?
        .text()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn series_strings(node: roxmltree::Node<'_, '_>, container: &str) -> Vec<String> {
    let Some(container) = node
        .children()
        .find(|child| child.is_element() && child.tag_name().name() == container)
    else {
        return Vec::new();
    };
    cached_points(container)
}

fn series_numbers(node: roxmltree::Node<'_, '_>, container: &str) -> Vec<f64> {
    series_strings(node, container)
        .into_iter()
        .filter_map(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .take(MAX_POINTS)
        .collect()
}

fn cached_points(container: roxmltree::Node<'_, '_>) -> Vec<String> {
    let mut indexed = container
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "pt")
        .filter_map(|point| {
            let index = point.attribute("idx")?.parse::<usize>().ok()?;
            let value = point
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "v")?
                .text()?
                .to_string();
            Some((index, value))
        })
        .take(MAX_POINTS)
        .collect::<Vec<_>>();
    if indexed.is_empty() {
        indexed = container
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "v")
            .filter_map(|node| node.text().map(ToOwned::to_owned))
            .take(MAX_POINTS)
            .enumerate()
            .collect();
    }
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, value)| value).collect()
}

fn collect_text(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants()
        .filter(|child| child.is_element() && matches!(child.tag_name().name(), "t" | "v"))
        .filter_map(|child| child.text())
        .collect::<Vec<_>>()
        .join("")
}

fn render_axes(body: &mut String) {
    let plot_width = WIDTH - LEFT - RIGHT;
    let plot_height = HEIGHT - TOP - BOTTOM;
    for step in 0..=5 {
        let y = TOP + plot_height * f64::from(step) / 5.0;
        body.push_str(&format!(
            "<line x1=\"{LEFT}\" y1=\"{y:.1}\" x2=\"{:.1}\" y2=\"{y:.1}\" stroke=\"#e7e7e7\"/>",
            LEFT + plot_width
        ));
    }
    body.push_str(&format!("<line x1=\"{LEFT}\" y1=\"{TOP}\" x2=\"{LEFT}\" y2=\"{:.1}\" stroke=\"#777\"/><line x1=\"{LEFT}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#777\"/>", TOP + plot_height, TOP + plot_height, LEFT + plot_width, TOP + plot_height));
}

fn value_bounds(chart: &ChartPreview) -> (f64, f64) {
    let mut minimum = 0.0f64;
    let mut maximum = 0.0f64;
    for value in chart.series.iter().flat_map(|series| series.values.iter()) {
        minimum = minimum.min(*value);
        maximum = maximum.max(*value);
    }
    if (maximum - minimum).abs() < f64::EPSILON {
        maximum = minimum + 1.0;
    }
    (minimum, maximum)
}

fn render_cartesian(chart: &ChartPreview, body: &mut String, numeric_x: bool) {
    render_axes(body);
    let plot_width = WIDTH - LEFT - RIGHT;
    let plot_height = HEIGHT - TOP - BOTTOM;
    let (minimum, maximum) = value_bounds(chart);
    for (series_index, series) in chart.series.iter().enumerate() {
        let count = series.values.len();
        if count == 0 {
            continue;
        }
        let x_min = series
            .x_values
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let x_max = series
            .x_values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let points = series
            .values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let x = if numeric_x && x_max > x_min && index < series.x_values.len() {
                    LEFT + (series.x_values[index] - x_min) / (x_max - x_min) * plot_width
                } else if count == 1 {
                    LEFT + plot_width / 2.0
                } else {
                    LEFT + index as f64 / (count - 1) as f64 * plot_width
                };
                let y = TOP + (maximum - value) / (maximum - minimum) * plot_height;
                (x, y)
            })
            .collect::<Vec<_>>();
        let color = PALETTE[series_index % PALETTE.len()];
        let joined = points
            .iter()
            .map(|(x, y)| format!("{x:.1},{y:.1}"))
            .collect::<Vec<_>>()
            .join(" ");
        body.push_str(&format!(
            "<polyline points=\"{joined}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"2.2\"/>"
        ));
        for (index, (x, y)) in points.iter().enumerate() {
            let radius = series
                .bubble_sizes
                .get(index)
                .map(|value| value.abs().sqrt().clamp(3.0, 15.0))
                .unwrap_or(3.3);
            body.push_str(&format!("<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{radius:.1}\" fill=\"{color}\" fill-opacity=\".72\"/>"));
        }
    }
}

fn render_bars(chart: &ChartPreview, body: &mut String) {
    render_axes(body);
    let plot_width = WIDTH - LEFT - RIGHT;
    let plot_height = HEIGHT - TOP - BOTTOM;
    let (_, maximum) = value_bounds(chart);
    let count = chart
        .series
        .iter()
        .map(|series| series.values.len())
        .max()
        .unwrap_or(1)
        .max(1);
    let group_width = plot_width / count as f64;
    let bar_width = (group_width * 0.78 / chart.series.len().max(1) as f64).max(1.0);
    for (series_index, series) in chart.series.iter().enumerate() {
        let color = PALETTE[series_index % PALETTE.len()];
        for (index, value) in series.values.iter().enumerate() {
            let height = value.max(0.0) / maximum.max(1.0) * plot_height;
            let x = LEFT
                + index as f64 * group_width
                + group_width * 0.11
                + series_index as f64 * bar_width;
            body.push_str(&format!("<rect x=\"{x:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{height:.1}\" fill=\"{color}\"/>", TOP + plot_height - height, bar_width * 0.92));
        }
    }
}

fn render_pie(chart: &ChartPreview, body: &mut String) {
    let values = &chart.series[0].values;
    let total: f64 = values.iter().map(|value| value.max(0.0)).sum();
    if total <= 0.0 {
        return render_cartesian(chart, body, false);
    }
    let (cx, cy, radius) = (WIDTH / 2.0, 174.0, 105.0);
    let mut angle = -std::f64::consts::FRAC_PI_2;
    for (index, value) in values.iter().enumerate() {
        let next = angle + value.max(0.0) / total * std::f64::consts::TAU;
        let large = i32::from(next - angle > std::f64::consts::PI);
        let (x1, y1) = (cx + radius * angle.cos(), cy + radius * angle.sin());
        let (x2, y2) = (cx + radius * next.cos(), cy + radius * next.sin());
        body.push_str(&format!("<path d=\"M {cx:.1} {cy:.1} L {x1:.1} {y1:.1} A {radius:.1} {radius:.1} 0 {large} 1 {x2:.1} {y2:.1} Z\" fill=\"{}\" stroke=\"#fff\"/>", PALETTE[index % PALETTE.len()]));
        angle = next;
    }
}

fn render_radar(chart: &ChartPreview, body: &mut String) {
    let count = chart
        .series
        .iter()
        .map(|series| series.values.len())
        .max()
        .unwrap_or(3)
        .max(3);
    let (cx, cy, radius) = (WIDTH / 2.0, 174.0, 108.0);
    let (_, maximum) = value_bounds(chart);
    for ring in 1..=5 {
        let r = radius * f64::from(ring) / 5.0;
        let points = radar_points(count, cx, cy, r, None, 1.0);
        body.push_str(&format!(
            "<polygon points=\"{points}\" fill=\"none\" stroke=\"#ddd\"/>"
        ));
    }
    for index in 0..count {
        let angle =
            -std::f64::consts::FRAC_PI_2 + index as f64 * std::f64::consts::TAU / count as f64;
        body.push_str(&format!(
            "<line x1=\"{cx:.1}\" y1=\"{cy:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"#ddd\"/>",
            cx + radius * angle.cos(),
            cy + radius * angle.sin()
        ));
    }
    for (index, series) in chart.series.iter().enumerate() {
        let points = radar_points(
            count,
            cx,
            cy,
            radius,
            Some(&series.values),
            maximum.max(1.0),
        );
        let color = PALETTE[index % PALETTE.len()];
        body.push_str(&format!("<polygon points=\"{points}\" fill=\"{color}\" fill-opacity=\".16\" stroke=\"{color}\" stroke-width=\"2\"/>"));
    }
}

fn render_box_whisker(chart: &ChartPreview, body: &mut String) {
    render_axes(body);
    let plot_width = WIDTH - LEFT - RIGHT;
    let plot_height = HEIGHT - TOP - BOTTOM;
    let (minimum, maximum) = value_bounds(chart);
    let scale_y = |value: f64| TOP + (maximum - value) / (maximum - minimum) * plot_height;
    let slot = plot_width / chart.series.len().max(1) as f64;
    for (index, series) in chart.series.iter().enumerate() {
        let mut values = series.values.clone();
        values.sort_by(f64::total_cmp);
        if values.is_empty() {
            continue;
        }
        let quantile = |fraction: f64| {
            let position = ((values.len() - 1) as f64 * fraction).round() as usize;
            values[position]
        };
        let low = values[0];
        let q1 = quantile(0.25);
        let median = quantile(0.5);
        let q3 = quantile(0.75);
        let high = values[values.len() - 1];
        let x = LEFT + (index as f64 + 0.5) * slot;
        let width = (slot * 0.45).clamp(12.0, 72.0);
        let color = PALETTE[index % PALETTE.len()];
        body.push_str(&format!(
            "<line x1=\"{x:.1}\" y1=\"{:.1}\" x2=\"{x:.1}\" y2=\"{:.1}\" stroke=\"{color}\" stroke-width=\"2\"/><line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{color}\"/><line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{color}\"/><rect x=\"{:.1}\" y=\"{:.1}\" width=\"{width:.1}\" height=\"{:.1}\" fill=\"{color}\" fill-opacity=\".18\" stroke=\"{color}\" stroke-width=\"2\"/><line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{color}\" stroke-width=\"2\"/>",
            scale_y(high),
            scale_y(low),
            x - width / 3.0,
            scale_y(high),
            x + width / 3.0,
            scale_y(high),
            x - width / 3.0,
            scale_y(low),
            x + width / 3.0,
            scale_y(low),
            x - width / 2.0,
            scale_y(q3),
            scale_y(q1) - scale_y(q3),
            x - width / 2.0,
            scale_y(median),
            x + width / 2.0,
            scale_y(median),
        ));
    }
}

fn radar_points(
    count: usize,
    cx: f64,
    cy: f64,
    radius: f64,
    values: Option<&[f64]>,
    maximum: f64,
) -> String {
    (0..count)
        .map(|index| {
            let scale = values
                .and_then(|items| items.get(index))
                .copied()
                .unwrap_or(maximum)
                .max(0.0)
                / maximum;
            let angle =
                -std::f64::consts::FRAC_PI_2 + index as f64 * std::f64::consts::TAU / count as f64;
            format!(
                "{:.1},{:.1}",
                cx + radius * scale * angle.cos(),
                cy + radius * scale * angle.sin()
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_legend(chart: &ChartPreview, body: &mut String) {
    let shown = chart.series.len().min(6);
    let item_width = (WIDTH - 30.0) / shown.max(1) as f64;
    for (index, series) in chart.series.iter().take(shown).enumerate() {
        let x = 18.0 + index as f64 * item_width;
        body.push_str(&format!("<rect x=\"{x:.1}\" y=\"326\" width=\"10\" height=\"10\" fill=\"{}\"/><text x=\"{:.1}\" y=\"336\" font-family=\"Arial,sans-serif\" font-size=\"11\" fill=\"#444\">{}</text>", PALETTE[index % PALETTE.len()], x + 14.0, escape_xml(&series.name)));
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_cached_scatter_data_and_escapes_title() {
        let xml = r#"<c:chartSpace xmlns:c="urn:c"><c:chart><c:title><c:tx><c:v>A &amp; B</c:v></c:tx></c:title><c:plotArea><c:scatterChart><c:ser><c:tx><c:v>S1</c:v></c:tx><c:xVal><c:numLit><c:pt idx="0"><c:v>1</c:v></c:pt><c:pt idx="1"><c:v>2</c:v></c:pt></c:numLit></c:xVal><c:yVal><c:numLit><c:pt idx="0"><c:v>3</c:v></c:pt><c:pt idx="1"><c:v>4</c:v></c:pt></c:numLit></c:yVal></c:ser></c:scatterChart></c:plotArea></c:chart></c:chartSpace>"#;
        let svg = render_chart_svg(xml).unwrap();
        assert!(svg.contains("A &amp; B"));
        assert!(svg.contains("<polyline"));
        assert!(svg.contains("<circle"));
        assert!(!svg.contains("A & B"));
    }

    #[test]
    fn renders_bar_and_radar_charts() {
        for kind in ["barChart", "radarChart"] {
            let xml = format!("<chart><plotArea><{kind}><ser><tx><v>S</v></tx><val><numLit><pt idx=\"0\"><v>4</v></pt><pt idx=\"1\"><v>9</v></pt></numLit></val></ser></{kind}></plotArea></chart>");
            let svg = render_chart_svg(&xml).unwrap();
            assert!(svg.contains("<svg"));
            assert!(svg.contains(if kind == "barChart" {
                "<rect x="
            } else {
                "<polygon"
            }));
        }
    }

    #[test]
    fn exposes_formula_references_when_a_chart_has_no_cache() {
        let xml = r#"<c:chartSpace xmlns:c="urn:c"><c:chart><c:plotArea><c:barChart><c:ser><c:tx><c:strRef><c:f>'Sales Data'!$B$1</c:f></c:strRef></c:tx><c:cat><c:strRef><c:f>'Sales Data'!$A$2:$A$4</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>'Sales Data'!$B$2:$B$4</c:f></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#;
        let preview = parse_chart_preview(xml).unwrap();
        assert_eq!(preview.series.len(), 1);
        assert_eq!(
            preview.series[0].name_formula.as_deref(),
            Some("'Sales Data'!$B$1")
        );
        assert_eq!(
            preview.series[0].categories_formula.as_deref(),
            Some("'Sales Data'!$A$2:$A$4")
        );
        assert_eq!(
            preview.series[0].values_formula.as_deref(),
            Some("'Sales Data'!$B$2:$B$4")
        );
        assert!(preview.series[0].values.is_empty());
    }

    #[test]
    fn renders_extended_histogram_from_chart_data() {
        let xml = r#"<cx:chartSpace xmlns:cx="urn:cx"><cx:chartData><cx:data id="0"><cx:numDim type="val"><cx:lvl><cx:pt idx="0">1</cx:pt><cx:pt idx="1">2</cx:pt><cx:pt idx="2">3</cx:pt><cx:pt idx="3">9</cx:pt></cx:lvl></cx:numDim></cx:data></cx:chartData><cx:chart><cx:title><cx:tx><cx:v>Histogram</cx:v></cx:tx></cx:title><cx:plotArea><cx:plotAreaRegion><cx:series layoutId="clusteredColumn"><cx:tx><cx:v>Samples</cx:v></cx:tx><cx:dataId val="0"/><cx:layoutPr><cx:binning/></cx:layoutPr></cx:series></cx:plotAreaRegion></cx:plotArea></cx:chart></cx:chartSpace>"#;
        let preview = parse_chart_preview(xml).unwrap();
        assert_eq!(preview.chart_kinds, ["bar"]);
        assert_eq!(preview.series.len(), 1);
        assert_eq!(preview.series[0].values.iter().sum::<f64>(), 4.0);
        let svg = render_preview_svg(&preview);
        assert!(svg.contains("Histogram"));
        assert!(!svg.contains("No cached chart data"));
    }
}
