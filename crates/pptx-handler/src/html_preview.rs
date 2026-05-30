#![allow(
    clippy::too_many_arguments,
    clippy::collapsible_else_if,
    clippy::needless_borrow,
    clippy::search_is_some,
    clippy::useless_format,
    clippy::manual_strip
)]

use handler_common::HandlerError;
use oxml::OxmlPackage;
use std::collections::HashMap;

// Embed the complete preview.css from C# resources
const PREVIEW_CSS: &str = r#"/* OfficeCli HTML Preview Stylesheet */
:root {
    --sidebar-w: 180px;
}
* { margin: 0; padding: 0; box-sizing: border-box; }
body {
    background: #1a1a2e;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Arial, sans-serif;
    display: flex;
    height: 100vh;
    overflow: hidden;
}
.sidebar {
    width: var(--sidebar-w);
    min-width: var(--sidebar-w);
    background: #12122a;
    border-right: 1px solid #2a2a4a;
    overflow-y: auto;
    padding: 12px 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
}
.sidebar-title {
    color: #888;
    font-size: 11px;
    text-align: center;
    margin-bottom: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
}
.thumb {
    cursor: pointer;
    border: 2px solid transparent;
    border-radius: 3px;
    padding: 2px;
    transition: border-color 0.15s;
    position: relative;
}
.thumb:hover { border-color: #555; }
.thumb.active { border-color: #5b9bd5; }
.thumb-inner {
    width: 100%;
    aspect-ratio: var(--slide-aspect);
    border-radius: 2px;
    overflow: hidden;
    position: relative;
    pointer-events: none;
}
.thumb-slide {
    width: var(--slide-design-w);
    height: var(--slide-design-h);
    position: absolute;
    top: 0;
    left: 0;
    transform-origin: 0 0;
    background: white;
}
.thumb-num {
    position: absolute;
    bottom: 2px;
    right: 4px;
    color: #888;
    font-size: 10px;
}
.main {
    flex: 1;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 20px;
    gap: 24px;
    scroll-behavior: smooth;
}
.file-title {
    display: none;
}
.slide-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
}
.slide-label {
    display: none;
}
.slide-wrapper {
    width: 100%;
    display: flex;
    justify-content: center;
}
.slide {
    width: var(--slide-design-w);
    height: var(--slide-design-h);
    position: relative;
    overflow: hidden;
    background: white;
    box-shadow: 0 4px 20px rgba(0,0,0,0.4);
    border-radius: 2px;
    transform-origin: center top;
    flex-shrink: 0;
}
.slide-notes {
    width: var(--slide-design-w);
    margin-top: 8px;
    padding: 10px 14px;
    background: #1f1f1f;
    color: #ddd;
    border-left: 3px solid #888;
    border-radius: 2px;
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    font-size: 12pt;
    line-height: 1.4;
    box-sizing: border-box;
}
.slide-notes[dir="rtl"] {
    border-left: none;
    border-right: 3px solid #888;
    text-align: right;
}
.slide-notes-label {
    font-size: 10pt;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: #888;
    margin-bottom: 4px;
}
.slide-notes-body div { margin: 2px 0; }
.page-counter {
    position: fixed;
    bottom: 16px;
    right: 24px;
    background: rgba(0,0,0,0.6);
    color: #ccc;
    padding: 4px 12px;
    border-radius: 12px;
    font-size: 13px;
    z-index: 100;
    pointer-events: none;
    transition: opacity 0.3s;
}
body.fullscreen .sidebar { display: none; }
body.fullscreen .main {
    padding: 0;
    gap: 0;
    overflow: hidden;
    scroll-behavior: auto;
}
body.fullscreen .slide-container {
    width: 100vw;
    height: 100vh;
    justify-content: center;
    display: none;
    gap: 0;
}
body.fullscreen .slide-container.fs-active { display: flex; }
body.fullscreen .slide-label { color: #444; font-size: 11px; }
body.fullscreen .slide {
    box-shadow: none;
    border-radius: 0;
}
.sidebar-toggle {
    position: fixed;
    top: 8px;
    left: 8px;
    z-index: 200;
    background: rgba(30,30,60,0.85);
    color: #aaa;
    border: 1px solid #444;
    border-radius: 4px;
    padding: 4px 8px;
    cursor: pointer;
    font-size: 14px;
    display: none;
    opacity: 0;
    transition: opacity 0.25s;
}
.sidebar-toggle:hover { color: #fff; border-color: #888; }
.toggle-zone {
    position: fixed;
    top: 0;
    left: 0;
    width: 60px;
    height: 60px;
    z-index: 199;
    display: none;
}
.toggle-zone:hover + .sidebar-toggle,
.sidebar-toggle:hover {
    opacity: 1;
}
body.sidebar-hidden .sidebar { display: none; }
body.sidebar-hidden .sidebar-toggle { left: 8px; }
html.headless .sidebar,
html.headless .sidebar-toggle,
html.headless .toggle-zone,
html.headless .page-counter { display: none !important; }

@media (max-width: 900px) {
    .sidebar { display: none; }
    .sidebar-toggle { display: block; }
    .toggle-zone { display: block; }
    body.sidebar-visible .sidebar { display: flex; position: fixed; top: 0; left: 0; bottom: 0; z-index: 150; }
    body.sidebar-visible .sidebar-toggle { left: calc(var(--sidebar-w) + 8px); opacity: 1; }
    body.sidebar-visible .toggle-zone { display: none; }
}

.shape {
    position: absolute;
    overflow: visible;
    white-space: pre-wrap;
    word-wrap: break-word;
}
.shape.has-fill {
    overflow: hidden;
}
.shape-text {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
}
.shape-text.valign-top { justify-content: flex-start; }
.shape-text.valign-center { justify-content: center; }
.shape-text.valign-bottom { justify-content: flex-end; }
.para { width: 100%; line-height: 1.2; }
.picture { position: absolute; overflow: hidden; }
.picture img { width: 100%; height: 100%; object-fit: fill; }
.table-container { position: absolute; overflow: visible; }
.slide-table { width: 100%; height: 100%; border-collapse: collapse; table-layout: fixed; }
.slide-table td { padding: 4px 6px; vertical-align: top; overflow: hidden; font-size: 10pt; color: inherit; }
.connector { position: absolute; pointer-events: none; }
.group { position: absolute; }
"#;

// Embed the complete preview.js from C# resources
const PREVIEW_JS: &str = r#"(function() {
    const main = document.querySelector('.main');
    const sidebar = document.querySelector('.sidebar');
    const counter = document.querySelector('.page-counter');
    let currentSlide = 0;
    let isFullscreen = false;

    function getContainers() { return [...document.querySelectorAll('.main > .slide-container')]; }
    function getThumbs() { return [...document.querySelectorAll('.sidebar > .thumb')]; }
    function getTotal() { return getContainers().length; }

    function scaleSlides() {
        const availW = main.clientWidth - 40;
        document.querySelectorAll('.main > .slide-container .slide').forEach(slide => {
            const designW = slide.offsetWidth;
            if (designW > availW && availW > 0) {
                const s = availW / designW;
                slide.style.transform = `scale(${s})`;
                slide.style.transformOrigin = 'center top';
                const designH = slide.offsetHeight;
                slide.parentElement.style.height = (designH * s) + 'px';
                slide.parentElement.style.width = (designW * s) + 'px';
            } else {
                slide.style.transform = '';
                slide.parentElement.style.height = '';
                slide.parentElement.style.width = '';
            }
        });
    }
    scaleSlides();
    window.scaleSlides = scaleSlides;
    window.addEventListener('resize', scaleSlides);

    function setActiveThumb(idx) {
        getThumbs().forEach((t, i) => t.classList.toggle('active', i === idx));
        currentSlide = idx;
        if (counter) counter.textContent = `${idx + 1} / ${getTotal()}`;
    }

    if (sidebar) {
        sidebar.addEventListener('click', function(e) {
            const thumb = e.target.closest('.thumb');
            if (!thumb) return;
            const thumbs = getThumbs();
            const idx = thumbs.indexOf(thumb);
            if (idx < 0) return;
            if (isFullscreen) { showFullscreenSlide(idx); return; }
            const containers = getContainers();
            if (containers[idx]) {
                containers[idx].scrollIntoView({ behavior: 'smooth', block: 'center' });
            }
            setActiveThumb(idx);
        });
    }

    let scrollObserver;
    if (main) {
        scrollObserver = new IntersectionObserver(entries => {
            if (isFullscreen) return;
            const containers = getContainers();
            entries.forEach(e => {
                if (e.isIntersecting && e.intersectionRatio > 0.3) {
                    const idx = containers.indexOf(e.target);
                    if (idx >= 0) setActiveThumb(idx);
                }
            });
        }, { root: main, threshold: 0.3 });
        getContainers().forEach(c => scrollObserver.observe(c));

        new MutationObserver(mutations => {
            mutations.forEach(m => {
                m.addedNodes.forEach(node => {
                    if (node.nodeType === 1 && node.classList.contains('slide-container')) {
                        scrollObserver.observe(node);
                    }
                });
            });
        }).observe(main, { childList: true });
    }

    function showFullscreenSlide(idx) {
        const containers = getContainers();
        const total = containers.length;
        idx = Math.max(0, Math.min(idx, total - 1));
        containers.forEach((c, i) => c.classList.toggle('fs-active', i === idx));
        setActiveThumb(idx);
        const slide = containers[idx]?.querySelector('.slide');
        if (slide) {
            const vw = window.innerWidth, vh = window.innerHeight - 30;
            const sw = slide.scrollWidth || slide.offsetWidth;
            const sh = slide.scrollHeight || slide.offsetHeight;
            const s = Math.min(vw / sw, vh / sh, 1);
            slide.style.transform = `scale(${s})`;
            slide.style.transformOrigin = 'center top';
        }
    }
    function enterFullscreen() {
        isFullscreen = true;
        document.body.classList.add('fullscreen');
        showFullscreenSlide(currentSlide);
    }
    function exitFullscreen() {
        isFullscreen = false;
        document.body.classList.remove('fullscreen');
        getContainers().forEach(c => { c.classList.remove('fs-active'); c.style.display = ''; });
        scaleSlides();
        getContainers()[currentSlide]?.scrollIntoView({ block: 'center' });
    }

    document.addEventListener('keydown', e => {
        if (e.key === 'f' || e.key === 'F') {
            e.preventDefault();
            isFullscreen ? exitFullscreen() : enterFullscreen();
            return;
        }
        if (e.key === 'Escape' && isFullscreen) {
            e.preventDefault();
            exitFullscreen();
            return;
        }
        const next = e.key === 'ArrowDown' || e.key === ' ' || e.key === 'ArrowRight';
        const prev = e.key === 'ArrowUp' || e.key === 'ArrowLeft';
        if (!next && !prev) return;
        e.preventDefault();

        const total = getTotal();
        if (isFullscreen) {
            showFullscreenSlide(currentSlide + (next ? 1 : -1));
        } else {
            const target = next
                ? Math.min(currentSlide + 1, total - 1)
                : Math.max(currentSlide - 1, 0);
            const containers = getContainers();
            if (containers[target]) {
                containers[target].scrollIntoView({ behavior: 'smooth', block: 'center' });
            }
            setActiveThumb(target);
        }
    });

    function buildThumbs() {
        const slides = document.querySelectorAll('.main > .slide-container .slide');
        const inners = document.querySelectorAll('.thumb-inner');
        slides.forEach((slide, i) => {
            if (i >= inners.length) return;
            const inner = inners[i];
            if (inner.querySelector('.thumb-slide')) return;
            const clone = slide.cloneNode(true);
            clone.className = 'thumb-slide';
            clone.style.transform = '';
            clone.querySelectorAll('[id]').forEach(el => el.removeAttribute('id'));
            clone.querySelectorAll('script').forEach(el => el.remove());
            inner.appendChild(clone);
        });
        scaleThumbs();
    }
    function scaleThumbs() {
        document.querySelectorAll('.thumb-inner').forEach(inner => {
            const thumbSlide = inner.querySelector('.thumb-slide');
            if (!thumbSlide) return;
            const thumbW = inner.clientWidth;
            const slideW = thumbSlide.scrollWidth || thumbSlide.offsetWidth;
            if (slideW > 0 && thumbW > 0) {
                thumbSlide.style.transform = `scale(${thumbW / slideW})`;
                thumbSlide.style.transformOrigin = '0 0';
            }
        });
    }
    buildThumbs();
    window.buildThumbs = buildThumbs;
    window.scaleThumbs = scaleThumbs;
    window.addEventListener('resize', scaleThumbs);

    window.toggleSidebar = function() {
        document.body.classList.toggle('sidebar-visible');
        document.body.classList.toggle('sidebar-hidden');
        requestAnimationFrame(() => {
            scaleSlides();
            buildThumbs();
            scaleThumbs();
        });
    };

    if (getTotal() > 0) setActiveThumb(0);
})();"#;

// ==================== Color Math & Resolvers ====================

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let rf = r as f64 / 255.0;
    let gf = g as f64 / 255.0;
    let bf = b as f64 / 255.0;
    let max = rf.max(gf.max(bf));
    let min = rf.min(gf.min(bf));
    let delta = max - min;
    let mut h = 0.0;
    let mut s = 0.0;
    let l = (max + min) / 2.0;

    if delta.abs() > 1e-10 {
        s = if l < 0.5 {
            delta / (max + min)
        } else {
            delta / (2.0 - max - min)
        };
        if (max - rf).abs() < 1e-10 {
            h = ((gf - bf) / delta + (if gf < bf { 6.0 } else { 0.0 })) / 6.0;
        } else if (max - gf).abs() < 1e-10 {
            h = ((bf - rf) / delta + 2.0) / 6.0;
        } else {
            h = ((rf - gf) / delta + 4.0) / 6.0;
        }
    }
    (h, s, l)
}

fn hue_to_rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    if s < 1e-10 {
        let val = (l * 255.0).round() as u8;
        return (val, val, val);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = (hue_to_rgb(p, q, h + 1.0 / 3.0) * 255.0).round() as u8;
    let g = (hue_to_rgb(p, q, h) * 255.0).round() as u8;
    let b = (hue_to_rgb(p, q, h - 1.0 / 3.0) * 255.0).round() as u8;
    (r, g, b)
}

fn apply_transforms(
    hex: &str,
    tint: Option<i32>,
    shade: Option<i32>,
    lum_mod: Option<i32>,
    lum_off: Option<i32>,
    alpha: Option<i32>,
) -> String {
    let mut r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
    let mut g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
    let mut b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);

    if let Some(t_val) = tint {
        let t = t_val as f64 / 100000.0;
        r = (r as f64 + (255.0 - r as f64) * (1.0 - t)).round() as u8;
        g = (g as f64 + (255.0 - g as f64) * (1.0 - t)).round() as u8;
        b = (b as f64 + (255.0 - b as f64) * (1.0 - t)).round() as u8;
    }

    if let Some(s_val) = shade {
        let s = s_val as f64 / 100000.0;
        r = (r as f64 * s).round() as u8;
        g = (g as f64 * s).round() as u8;
        b = (b as f64 * s).round() as u8;
    }

    if lum_mod.is_some() || lum_off.is_some() {
        let m = lum_mod.unwrap_or(100000) as f64 / 100000.0;
        let o = lum_off.unwrap_or(0) as f64 / 100000.0;
        let (h, s, mut l) = rgb_to_hsl(r, g, b);
        l = (l * m + o).clamp(0.0, 1.0);
        let (nr, ng, nb) = hsl_to_rgb(h, s, l);
        r = nr;
        g = ng;
        b = nb;
    }

    if let Some(a_val) = alpha {
        if a_val < 100000 {
            return format!("rgba({},{},{},{:.2})", r, g, b, a_val as f64 / 100000.0);
        }
    }
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

fn resolve_fill_color(
    solid_fill: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
) -> Option<String> {
    let mut base_hex = None;
    let mut clr_node = None;

    if let Some(srgb) = solid_fill.children().find(|n| n.has_tag_name("srgbClr")) {
        if let Some(val) = srgb.attribute("val") {
            base_hex = Some(val.to_string());
        }
        clr_node = Some(srgb);
    } else if let Some(scheme) = solid_fill.children().find(|n| n.has_tag_name("schemeClr")) {
        if let Some(val) = scheme.attribute("val") {
            if let Some(hex) = theme_colors.get(val) {
                base_hex = Some(hex.to_string());
            } else if val == "lt1" {
                base_hex = Some("FFFFFF".to_string());
            } else if val == "dk1" {
                base_hex = Some("000000".to_string());
            }
        }
        clr_node = Some(scheme);
    } else if let Some(sys) = solid_fill.children().find(|n| n.has_tag_name("sysClr")) {
        if let Some(val) = sys.attribute("lastClr") {
            base_hex = Some(val.to_string());
        }
        clr_node = Some(sys);
    }

    if let Some(hex) = base_hex {
        if let Some(node) = clr_node {
            let tint = node
                .children()
                .find(|n| n.has_tag_name("tint"))
                .and_then(|n| n.attribute("val"))
                .and_then(|s| s.parse::<i32>().ok());
            let shade = node
                .children()
                .find(|n| n.has_tag_name("shade"))
                .and_then(|n| n.attribute("val"))
                .and_then(|s| s.parse::<i32>().ok());
            let lum_mod = node
                .children()
                .find(|n| n.has_tag_name("lumMod"))
                .and_then(|n| n.attribute("val"))
                .and_then(|s| s.parse::<i32>().ok());
            let lum_off = node
                .children()
                .find(|n| n.has_tag_name("lumOff"))
                .and_then(|n| n.attribute("val"))
                .and_then(|s| s.parse::<i32>().ok());
            let alpha = node
                .children()
                .find(|n| n.has_tag_name("alpha"))
                .and_then(|n| n.attribute("val"))
                .and_then(|s| s.parse::<i32>().ok());
            return Some(apply_transforms(&hex, tint, shade, lum_mod, lum_off, alpha));
        }
        return Some(format!("#{}", hex));
    }
    None
}

fn gradient_to_css(grad_fill: &roxmltree::Node, theme_colors: &HashMap<String, String>) -> String {
    let mut stops = Vec::new();
    if let Some(stop_list) = grad_fill.children().find(|n| n.has_tag_name("gsLst")) {
        for stop in stop_list.children().filter(|n| n.has_tag_name("gs")) {
            let pos = stop
                .attribute("pos")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0)
                / 1000.0;
            let color = resolve_fill_color(&stop, theme_colors)
                .unwrap_or_else(|| "transparent".to_string());
            stops.push(format!("{} {:.2}%", color, pos));
        }
    }
    if stops.is_empty() {
        return "transparent".to_string();
    }

    if grad_fill.children().any(|n| n.has_tag_name("path")) {
        return format!("radial-gradient(circle closest-side, {})", stops.join(", "));
    }

    let lin = grad_fill.children().find(|n| n.has_tag_name("lin"));
    let angle_deg = lin
        .and_then(|n| n.attribute("ang"))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(5400000.0)
        / 60000.0;
    let css_angle = angle_deg + 90.0;
    format!("linear-gradient({:.2}deg, {})", css_angle, stops.join(", "))
}

// ==================== Outlines / Borders ====================

fn parse_outline(
    ln: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
) -> Option<(f64, String, String)> {
    if ln.children().any(|n| n.has_tag_name("noFill")) {
        return None;
    }
    let has_fill = ln
        .children()
        .any(|n| n.has_tag_name("solidFill") || n.has_tag_name("gradFill"));
    let width_emu = ln.attribute("w").and_then(|s| s.parse::<f64>().ok());
    if !has_fill && width_emu.is_none() {
        return None;
    }

    let color = ln
        .children()
        .find(|n| n.has_tag_name("solidFill"))
        .and_then(|n| resolve_fill_color(&n, theme_colors))
        .unwrap_or_else(|| {
            theme_colors
                .get("dk1")
                .map(|hex| format!("#{}", hex))
                .unwrap_or_else(|| "#000000".to_string())
        });
    let width_pt = width_emu.unwrap_or(12700.0) / 12700.0;
    let width_pt = if width_pt < 0.5 { 0.5 } else { width_pt };

    let prst_dash = ln
        .children()
        .find(|n| n.has_tag_name("prstDash"))
        .and_then(|n| n.attribute("val"))
        .unwrap_or("solid");

    Some((width_pt, prst_dash.to_string(), color))
}

fn outline_to_css(ln: &roxmltree::Node, theme_colors: &HashMap<String, String>) -> String {
    if let Some((width_pt, prst_dash, color)) = parse_outline(ln, theme_colors) {
        let border_style = match prst_dash.as_str() {
            "dash" | "lgDash" | "sysDash" => "dashed",
            "dot" | "sysDot" => "dotted",
            "dashDot" | "lgDashDot" | "sysDashDot" | "sysDashDotDot" => "dashed",
            _ => "solid",
        };
        format!("border:{:.2}pt {} {}", width_pt, border_style, color)
    } else {
        "".to_string()
    }
}

fn dash_type_to_svg_dasharray(dash_type: &str, stroke_width: f64) -> String {
    let w = stroke_width;
    match dash_type {
        "dot" | "sysDot" => format!("{:.2} {:.2}", w, w * 2.0),
        "dash" => format!("{:.2} {:.2}", w * 4.0, w * 3.0),
        "lgDash" => format!("{:.2} {:.2}", w * 8.0, w * 3.0),
        "sysDash" => format!("{:.2} {:.2}", w * 3.0, w * 1.0),
        "dashDot" => format!("{:.2} {:.2} {:.2} {:.2}", w * 4.0, w * 2.0, w, w * 2.0),
        "lgDashDot" => format!("{:.2} {:.2} {:.2} {:.2}", w * 8.0, w * 2.0, w, w * 2.0),
        "sysDashDot" => format!("{:.2} {:.2} {:.2} {:.2}", w * 3.0, w * 1.5, w, w * 1.5),
        "sysDashDotDot" => format!(
            "{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}",
            w * 3.0,
            w * 1.5,
            w,
            w * 1.5,
            w,
            w * 1.5
        ),
        _ => "".to_string(),
    }
}

// ==================== Shadow & Soft Edges ====================

fn effect_list_to_shadow_css(
    effect_list: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
) -> String {
    if let Some(shadow) = effect_list.children().find(|n| n.has_tag_name("outerShdw")) {
        let alpha = shadow
            .children()
            .find(|n| n.has_tag_name("alpha"))
            .and_then(|n| n.attribute("val"))
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(50000);
        let opacity = alpha as f64 / 100000.0;

        let rgb = shadow
            .children()
            .find(|n| n.has_tag_name("srgbClr"))
            .and_then(|n| n.attribute("val"))
            .map(|s| s.to_string());

        let color = if let Some(hex) = rgb {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
            format!("rgba({},{},{},{:.2})", r, g, b, opacity)
        } else if let Some(scheme) = shadow.children().find(|n| n.has_tag_name("schemeClr")) {
            let scheme_name = scheme.attribute("val").unwrap_or("");
            let resolved = theme_colors.get(scheme_name).map(|s| s.to_string());
            if let Some(hex) = resolved {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                format!("rgba({},{},{},{:.2})", r, g, b, opacity)
            } else {
                format!("rgba(0,0,0,{:.2})", opacity)
            }
        } else {
            format!("rgba(0,0,0,{:.2})", opacity)
        };

        let blur_pt = shadow
            .attribute("blurRad")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 12700.0;
        let dist_pt = shadow
            .attribute("dist")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 12700.0;
        let angle_deg = shadow
            .attribute("dir")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 60000.0;
        let angle_rad = angle_deg * std::f64::consts::PI / 180.0;
        let offset_x = dist_pt * angle_rad.cos();
        let offset_y = dist_pt * angle_rad.sin();

        return format!(
            "drop-shadow({:.2}pt {:.2}pt {:.2}pt {})",
            offset_x, offset_y, blur_pt, color
        );
    }
    "".to_string()
}

fn effect_list_to_glow_css(
    effect_list: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
) -> String {
    if let Some(glow) = effect_list.children().find(|n| n.has_tag_name("glow")) {
        let alpha = glow
            .children()
            .find(|n| n.has_tag_name("alpha"))
            .and_then(|n| n.attribute("val"))
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(40000);
        let opacity = alpha as f64 / 100000.0;
        let radius_pt = glow
            .attribute("rad")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(63500.0)
            / 12700.0;

        let rgb = glow
            .children()
            .find(|n| n.has_tag_name("srgbClr"))
            .and_then(|n| n.attribute("val"))
            .map(|s| s.to_string());

        let color = if let Some(hex) = rgb {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
            format!("rgba({},{},{},{:.2})", r, g, b, opacity)
        } else if let Some(scheme) = glow.children().find(|n| n.has_tag_name("schemeClr")) {
            let scheme_name = scheme.attribute("val").unwrap_or("");
            let resolved = theme_colors.get(scheme_name).map(|s| s.to_string());
            if let Some(hex) = resolved {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                format!("rgba({},{},{},{:.2})", r, g, b, opacity)
            } else {
                format!("rgba(0,0,0,{:.2})", opacity)
            }
        } else {
            if let Some(hex) = theme_colors.get("accent1") {
                let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
                let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
                let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
                format!("rgba({},{},{},{:.2})", r, g, b, opacity)
            } else {
                "rgba(0,0,0,0)".to_string()
            }
        };

        return format!("drop-shadow(0 0 {:.2}pt {})", radius_pt, color);
    }
    "".to_string()
}

fn effect_list_to_reflection_css(effect_list: &roxmltree::Node) -> String {
    if let Some(refl) = effect_list
        .children()
        .find(|n| n.has_tag_name("reflection"))
    {
        let dist_pt = refl
            .attribute("dist")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 12700.0;
        let start_opacity = refl
            .attribute("stAl")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(52000.0)
            / 100000.0;
        let end_opacity = refl
            .attribute("endAl")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 100000.0;
        let end_pos = refl
            .attribute("endPos")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(90000.0)
            / 1000.0;
        let end_pos = end_pos.clamp(0.0, 100.0);

        let start_stop = format!("rgba(255,255,255,{:.3}) 0%", start_opacity);
        let end_stop = format!("rgba(255,255,255,{:.3}) {:.1}%", end_opacity, end_pos);
        let tail_stop = if end_pos < 100.0 {
            ",transparent 100%"
        } else {
            ""
        };

        return format!(
            "-webkit-box-reflect:below {:.2}pt linear-gradient({}, {}{})",
            dist_pt, start_stop, end_stop, tail_stop
        );
    }
    "".to_string()
}

// ==================== Geometries & Clip Paths ====================

fn read_adj_value(preset_geom: &roxmltree::Node, index: usize, default_value: i64) -> i64 {
    if let Some(av_lst) = preset_geom.children().find(|n| n.has_tag_name("avLst")) {
        let gd = av_lst
            .children()
            .filter(|n| n.has_tag_name("gd"))
            .nth(index);
        if let Some(guide) = gd {
            if let Some(fmla) = guide.attribute("fmla") {
                if fmla.starts_with("val ") {
                    if let Ok(parsed) = fmla[4..].parse::<i64>() {
                        return parsed;
                    }
                }
            }
        }
    }
    default_value
}

fn right_arrow_polygon(width: f64, height: f64, preset_geom: &roxmltree::Node) -> String {
    let adj1 = read_adj_value(preset_geom, 0, 50000).clamp(0, 100000);
    let adj2 = read_adj_value(preset_geom, 1, 50000).clamp(0, 100000);

    let tail_top = (100000.0 - adj1 as f64) / 2000.0;
    let tail_bot = 100.0 - tail_top;

    let head_start_x = if width > 0.0 && height > 0.0 {
        let min_side = width.min(height);
        let head_w = min_side * adj2 as f64 / 100000.0;
        ((width - head_w) / width * 100.0).clamp(0.0, 100.0)
    } else {
        100.0 - adj2 as f64 / 1000.0
    };

    format!("clip-path:polygon(0% {:.1}%, {:.1}% {:.1}%, {:.1}% 0%, 100% 50%, {:.1}% 100%, {:.1}% {:.1}%, 0% {:.1}%)",
        tail_top, head_start_x, tail_top, head_start_x, head_start_x, head_start_x, tail_bot, tail_bot)
}

fn star5_polygon(preset_geom: &roxmltree::Node) -> String {
    let adj = read_adj_value(preset_geom, 0, 19098).clamp(0, 50000);
    let inner_ratio = adj as f64 / 50000.0;
    let mut pts = Vec::new();
    for i in 0..10 {
        let angle = -std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / 5.0;
        let r = if i % 2 == 0 { 0.5 } else { 0.5 * inner_ratio };
        let x = 50.0 + r * angle.cos() * 100.0;
        let y = 50.0 + r * angle.sin() * 100.0;
        pts.push(format!("{:.1}% {:.1}%", x, y));
    }
    format!("clip-path:polygon({})", pts.join(","))
}

fn preset_geometry_to_css(
    preset: &str,
    cx_pt: f64,
    cy_pt: f64,
    preset_geom: &roxmltree::Node,
) -> String {
    if preset == "rightArrow" {
        return right_arrow_polygon(cx_pt, cy_pt, preset_geom);
    }
    if preset == "star5" {
        return star5_polygon(preset_geom);
    }
    if preset == "roundRect"
        || preset == "round1Rect"
        || preset == "round2SameRect"
        || preset == "round2DiagRect"
    {
        let min_side = cx_pt.min(cy_pt);
        let av_val = read_adj_value(preset_geom, 0, 16667).clamp(0, 100000);
        let radius = min_side * (av_val as f64 / 100000.0);
        let r_str = format!("{:.1}pt", radius);
        return match preset {
            "round1Rect" => format!("border-radius:{} 0 0 0", r_str),
            "round2SameRect" => format!("border-radius:{} {} 0 0", r_str, r_str),
            "round2DiagRect" => format!("border-radius:{} 0 {} 0", r_str, r_str),
            _ => format!("border-radius:{}", r_str),
        };
    }

    match preset {
        "ellipse" => "border-radius:50%".to_string(),
        "triangle" | "isosTriangle" => "clip-path:polygon(50% 0%, 100% 100%, 0% 100%)".to_string(),
        "rtTriangle" => "clip-path:polygon(0% 0%, 100% 100%, 0% 100%)".to_string(),
        "diamond" => "clip-path:polygon(50% 0%, 100% 50%, 50% 100%, 0% 50%)".to_string(),
        "parallelogram" => "clip-path:polygon(15% 0%, 100% 0%, 85% 100%, 0% 100%)".to_string(),
        "trapezoid" => "clip-path:polygon(20% 0%, 80% 0%, 100% 100%, 0% 100%)".to_string(),
        "pentagon" => "clip-path:polygon(50% 0%, 100% 38%, 82% 100%, 18% 100%, 0% 38%)".to_string(),
        "hexagon" => "clip-path:polygon(25% 0%, 75% 0%, 100% 50%, 75% 100%, 25% 100%, 0% 50%)".to_string(),
        "octagon" => "clip-path:polygon(29% 0%, 71% 0%, 100% 29%, 100% 71%, 71% 100%, 29% 100%, 0% 71%, 0% 29%)".to_string(),
        "chevron" => "clip-path:polygon(0% 0%, 80% 0%, 100% 50%, 80% 100%, 0% 100%, 20% 50%)".to_string(),
        "homePlate" => "clip-path:polygon(0% 0%, 85% 0%, 100% 50%, 85% 100%, 0% 100%)".to_string(),
        "plus" | "cross" => "clip-path:polygon(33% 0%, 67% 0%, 67% 33%, 100% 33%, 100% 67%, 67% 67%, 67% 100%, 33% 100%, 33% 67%, 0% 67%, 0% 33%, 33% 33%)".to_string(),
        "star4" => "clip-path:polygon(50% 0%, 62% 38%, 100% 50%, 62% 62%, 50% 100%, 38% 62%, 0% 50%, 38% 38%)".to_string(),
        "heart" => "clip-path:polygon(50% 18%, 53% 12%, 57% 6%, 62% 2%, 68% 0%, 75% 0%, 82% 0%, 89% 3%, 94% 8%, 98% 14%, 100% 21%, 100% 28%, 99% 35%, 95% 43%, 90% 51%, 84% 59%, 77% 67%, 69% 75%, 60% 84%, 50% 100%, 40% 84%, 31% 75%, 23% 67%, 16% 59%, 10% 51%, 5% 43%, 1% 35%, 0% 28%, 0% 21%, 2% 14%, 6% 8%, 11% 3%, 18% 0%, 25% 0%, 32% 0%, 38% 2%, 43% 6%, 47% 12%)".to_string(),
        "cloud" | "cloudCallout" => "clip-path:polygon(25% 80%,18% 80%,12% 78%,7% 74%,5% 69%,4% 64%,5% 60%,3% 56%,1% 51%,1% 47%,3% 42%,7% 38%,11% 36%,15% 35%,14% 29%,14% 23%,17% 19%,21% 16%,26% 15%,30% 15%,31% 10%,34% 6%,38% 3%,43% 1%,48% 0%,55% 5%,61% 2%,67% 1%,72% 2%,76% 6%,78% 15%,82% 12%,87% 11%,91% 13%,94% 17%,95% 22%,95% 30%,97% 33%,99% 37%,100% 42%,99% 47%,97% 52%,93% 55%,90% 55%,93% 59%,96% 64%,97% 68%,96% 73%,92% 76%,88% 78%,85% 78%,84% 82%,82% 87%,78% 90%,73% 92%,68% 92%,63% 90%,60% 90%,56% 93%,51% 96%,46% 97%,41% 96%,38% 93%,35% 90%)".to_string(),
        "cube" => "clip-path:polygon(10% 0%, 100% 0%, 100% 85%, 90% 100%, 0% 100%, 0% 15%)".to_string(),
        _ => String::new(),
    }
}

fn custom_geometry_to_clip_path(cust_geom: &roxmltree::Node) -> String {
    let path_list = match cust_geom.children().find(|n| n.has_tag_name("pathLst")) {
        Some(pl) => pl,
        None => return "".to_string(),
    };
    let path = match path_list.children().find(|n| n.has_tag_name("path")) {
        Some(p) => p,
        None => return "".to_string(),
    };

    let path_w = path
        .attribute("w")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(100000.0);
    let path_h = path
        .attribute("h")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(100000.0);
    let pw = if path_w == 0.0 { 100000.0 } else { path_w };
    let ph = if path_h == 0.0 { 100000.0 } else { path_h };

    let try_parse_point = |pt: &roxmltree::Node| -> Option<(f64, f64)> {
        let x_str = pt.attribute("x")?;
        let y_str = pt.attribute("y")?;
        let xv = x_str.parse::<f64>().ok()?;
        let yv = y_str.parse::<f64>().ok()?;
        Some((xv * 100.0 / pw, yv * 100.0 / ph))
    };

    let mut has_only_lines = true;
    for child in path.children() {
        let tag = child.tag_name().name();
        if tag == "cubicBezTo" || tag == "quadBezTo" {
            has_only_lines = false;
            break;
        }
    }

    if has_only_lines {
        let mut points = Vec::new();
        for child in path.children() {
            let tag = child.tag_name().name();
            if tag == "moveTo" {
                if let Some(pt) = child.children().find(|n| n.has_tag_name("pt")) {
                    if let Some((mx, my)) = try_parse_point(&pt) {
                        points.push(format!("{:.1}% {:.1}%", mx, my));
                    }
                }
            } else if tag == "lineTo" {
                if let Some(pt) = child.children().find(|n| n.has_tag_name("pt")) {
                    if let Some((lx, ly)) = try_parse_point(&pt) {
                        points.push(format!("{:.1}% {:.1}%", lx, ly));
                    }
                }
            }
        }
        if points.len() >= 3 {
            return format!("clip-path:polygon({})", points.join(","));
        }
    } else {
        let mut poly_points = Vec::new();
        let mut cur_x = 0.0;
        let mut cur_y = 0.0;
        const BEZIER_SEGMENTS: usize = 8;

        for child in path.children() {
            let tag = child.tag_name().name();
            if tag == "moveTo" {
                if let Some(pt) = child.children().find(|n| n.has_tag_name("pt")) {
                    if let Some((mx, my)) = try_parse_point(&pt) {
                        poly_points.push(format!("{:.1}% {:.1}%", mx, my));
                        cur_x = mx;
                        cur_y = my;
                    }
                }
            } else if tag == "lineTo" {
                if let Some(pt) = child.children().find(|n| n.has_tag_name("pt")) {
                    if let Some((lx, ly)) = try_parse_point(&pt) {
                        poly_points.push(format!("{:.1}% {:.1}%", lx, ly));
                        cur_x = lx;
                        cur_y = ly;
                    }
                }
            } else if tag == "cubicBezTo" {
                let pts: Vec<roxmltree::Node> =
                    child.children().filter(|n| n.has_tag_name("pt")).collect();
                if pts.len() >= 3 {
                    if let (Some((c1x, c1y)), Some((c2x, c2y)), Some((c3x, c3y))) = (
                        try_parse_point(&pts[0]),
                        try_parse_point(&pts[1]),
                        try_parse_point(&pts[2]),
                    ) {
                        for i in 1..=BEZIER_SEGMENTS {
                            let t = i as f64 / BEZIER_SEGMENTS as f64;
                            let u = 1.0 - t;
                            let px = u * u * u * cur_x
                                + 3.0 * u * u * t * c1x
                                + 3.0 * u * t * t * c2x
                                + t * t * t * c3x;
                            let py = u * u * u * cur_y
                                + 3.0 * u * u * t * c1y
                                + 3.0 * u * t * t * c2y
                                + t * t * t * c3y;
                            poly_points.push(format!("{:.1}% {:.1}%", px, py));
                        }
                        cur_x = c3x;
                        cur_y = c3y;
                    }
                }
            } else if tag == "quadBezTo" {
                let pts: Vec<roxmltree::Node> =
                    child.children().filter(|n| n.has_tag_name("pt")).collect();
                if pts.len() >= 2 {
                    if let (Some((q1x, q1y)), Some((q2x, q2y))) =
                        (try_parse_point(&pts[0]), try_parse_point(&pts[1]))
                    {
                        for i in 1..=BEZIER_SEGMENTS {
                            let t = i as f64 / BEZIER_SEGMENTS as f64;
                            let u = 1.0 - t;
                            let px = u * u * cur_x + 2.0 * u * t * q1x + t * t * q2x;
                            let py = u * u * cur_y + 2.0 * u * t * q1y + t * t * q2y;
                            poly_points.push(format!("{:.1}% {:.1}%", px, py));
                        }
                        cur_x = q2x;
                        cur_y = q2y;
                    }
                }
            }
        }
        if poly_points.len() >= 3 {
            return format!("clip-path:polygon({})", poly_points.join(","));
        }
    }
    "".to_string()
}

// ==================== Placeholders & Inheritance ====================

fn find_matching_placeholder<'a>(
    ph_type: Option<&str>,
    ph_idx: Option<usize>,
    shape_tree: &'a roxmltree::Node<'a, 'a>,
) -> Option<roxmltree::Node<'a, 'a>> {
    if let Some(idx) = ph_idx {
        for sp in shape_tree.descendants().filter(|n| n.has_tag_name("sp")) {
            if let Some(ph) = sp.descendants().find(|n| n.has_tag_name("ph")) {
                if let Some(ph_i) = ph.attribute("idx").and_then(|s| s.parse::<usize>().ok()) {
                    if ph_i == idx {
                        return Some(sp);
                    }
                }
            }
        }
    }
    if let Some(ty) = ph_type {
        for sp in shape_tree.descendants().filter(|n| n.has_tag_name("sp")) {
            if let Some(ph) = sp.descendants().find(|n| n.has_tag_name("ph")) {
                if let Some(ph_t) = ph.attribute("type") {
                    if ph_t == ty {
                        return Some(sp);
                    }
                }
            }
        }
    }
    if ph_type.is_none() && ph_idx.is_none() {
        for sp in shape_tree.descendants().filter(|n| n.has_tag_name("sp")) {
            if let Some(ph) = sp.descendants().find(|n| n.has_tag_name("ph")) {
                let ph_t = ph.attribute("type").unwrap_or("body");
                if ph_t == "title"
                    || ph_t == "ctrTitle"
                    || ph_t == "subTitle"
                    || ph_t == "body"
                    || ph_t == "obj"
                {
                    return Some(sp);
                }
            }
        }
    }
    None
}

fn resolve_inherited_position(
    ph_type: Option<&str>,
    ph_idx: Option<usize>,
    layout_tree: Option<roxmltree::Node<'_, '_>>,
    master_tree: Option<roxmltree::Node<'_, '_>>,
) -> Option<(f64, f64, f64, f64)> {
    if let Some(tree) = layout_tree {
        if let Some(matched) = find_matching_placeholder(ph_type, ph_idx, &tree) {
            if let Some(xfrm) = matched.descendants().find(|n| n.has_tag_name("xfrm")) {
                if let (Some(off), Some(ext)) = (
                    xfrm.descendants().find(|n| n.has_tag_name("off")),
                    xfrm.descendants().find(|n| n.has_tag_name("ext")),
                ) {
                    let x = off
                        .attribute("x")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let y = off
                        .attribute("y")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let cx = ext
                        .attribute("cx")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let cy = ext
                        .attribute("cy")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    return Some((x, y, cx, cy));
                }
            }
        }
    }
    if let Some(tree) = master_tree {
        if let Some(matched) = find_matching_placeholder(ph_type, ph_idx, &tree) {
            if let Some(xfrm) = matched.descendants().find(|n| n.has_tag_name("xfrm")) {
                if let (Some(off), Some(ext)) = (
                    xfrm.descendants().find(|n| n.has_tag_name("off")),
                    xfrm.descendants().find(|n| n.has_tag_name("ext")),
                ) {
                    let x = off
                        .attribute("x")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let y = off
                        .attribute("y")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let cx = ext
                        .attribute("cx")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    let cy = ext
                        .attribute("cy")
                        .and_then(|s| s.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    return Some((x, y, cx, cy));
                }
            }
        }
    }
    None
}

fn get_level_def_rp<'a>(
    style_list: &'a roxmltree::Node<'a, 'a>,
    level: usize,
) -> Option<roxmltree::Node<'a, 'a>> {
    let tag_name = match level {
        0 => "lvl1pPr",
        1 => "lvl2pPr",
        2 => "lvl3pPr",
        3 => "lvl4pPr",
        4 => "lvl5pPr",
        5 => "lvl6pPr",
        6 => "lvl7pPr",
        7 => "lvl8pPr",
        8 => "lvl9pPr",
        _ => "lvl1pPr",
    };
    style_list
        .children()
        .find(|n| n.has_tag_name(tag_name))
        .and_then(|n| n.children().find(|c| c.has_tag_name("defRPr")))
}

fn resolve_placeholder_font_size(
    ph_type: Option<&str>,
    ph_idx: Option<usize>,
    level: usize,
    layout_tree: Option<roxmltree::Node<'_, '_>>,
    master_tree: Option<roxmltree::Node<'_, '_>>,
    master_text_styles: Option<roxmltree::Node<'_, '_>>,
) -> Option<f64> {
    if let Some(tree) = layout_tree {
        if let Some(matched) = find_matching_placeholder(ph_type, ph_idx, &tree) {
            if let Some(tx_body) = matched.descendants().find(|n| n.has_tag_name("txBody")) {
                if let Some(lst_style) = tx_body.children().find(|n| n.has_tag_name("lstStyle")) {
                    if let Some(def_rp) = get_level_def_rp(&lst_style, level) {
                        if let Some(sz) = def_rp.attribute("sz").and_then(|s| s.parse::<f64>().ok())
                        {
                            return Some(sz / 100.0);
                        }
                    }
                }
            }
        }
    }
    if let Some(tree) = master_tree {
        if let Some(matched) = find_matching_placeholder(ph_type, ph_idx, &tree) {
            if let Some(tx_body) = matched.descendants().find(|n| n.has_tag_name("txBody")) {
                if let Some(lst_style) = tx_body.children().find(|n| n.has_tag_name("lstStyle")) {
                    if let Some(def_rp) = get_level_def_rp(&lst_style, level) {
                        if let Some(sz) = def_rp.attribute("sz").and_then(|s| s.parse::<f64>().ok())
                        {
                            return Some(sz / 100.0);
                        }
                    }
                }
            }
        }
    }
    if let Some(tx_styles) = master_text_styles {
        let is_title = ph_type
            .map(|t| t == "title" || t == "ctrTitle")
            .unwrap_or(false);
        let is_body = ph_type
            .map(|t| t == "body" || t == "subTitle" || t == "obj")
            .unwrap_or(false);

        let style_list = if is_title {
            tx_styles.children().find(|n| n.has_tag_name("titleStyle"))
        } else if is_body {
            tx_styles.children().find(|n| n.has_tag_name("bodyStyle"))
        } else {
            tx_styles.children().find(|n| n.has_tag_name("otherStyle"))
        };

        if let Some(sl) = style_list {
            if let Some(def_rp) = get_level_def_rp(&sl, level) {
                if let Some(sz) = def_rp.attribute("sz").and_then(|s| s.parse::<f64>().ok()) {
                    return Some(sz / 100.0);
                }
            }
        }
    }

    let is_title = ph_type
        .map(|t| t == "title" || t == "ctrTitle")
        .unwrap_or(false);
    let is_subtitle = ph_type.map(|t| t == "subTitle").unwrap_or(false);
    if is_title {
        Some(44.0)
    } else if is_subtitle {
        Some(32.0)
    } else {
        None
    }
}

// ==================== Speaker Notes ====================

fn get_speaker_notes(package: &OxmlPackage, slide_path: &str) -> Option<String> {
    let slide_rels = package.part_rels(slide_path).ok()?;
    let notes_rel = slide_rels
        .all()
        .values()
        .find(|r| r.type_uri.contains("relationships/notesSlide"))?;
    let notes_path = package.resolve_rel_target(slide_path, &notes_rel.target);

    let notes_xml = package.read_part_xml(&notes_path).ok()?;
    let doc = roxmltree::Document::parse(&notes_xml).ok()?;

    let sp_tree = doc.descendants().find(|n| n.has_tag_name("spTree"))?;
    let notes_shape = sp_tree
        .children()
        .filter(|n| n.has_tag_name("sp"))
        .find(|sp| {
            if let Some(ph) = sp.descendants().find(|n| n.has_tag_name("ph")) {
                if let Some(idx) = ph.attribute("idx").and_then(|s| s.parse::<usize>().ok()) {
                    if idx == 1 {
                        return true;
                    }
                }
                if let Some(ty) = ph.attribute("type") {
                    if ty == "body" {
                        return true;
                    }
                }
            }
            false
        });

    let shape = notes_shape.or_else(|| {
        sp_tree
            .children()
            .filter(|n| n.has_tag_name("sp"))
            .find(|sp| {
                sp.descendants()
                    .any(|n| n.has_tag_name("ph") && n.attribute("type") == Some("body"))
            })
    })?;

    let tx_body = shape.children().find(|n| n.has_tag_name("txBody"))?;
    let mut notes_lines = Vec::new();
    let mut rtl = false;

    let paragraphs: Vec<roxmltree::Node> = tx_body
        .descendants()
        .filter(|n| n.has_tag_name("p"))
        .collect();
    if paragraphs.is_empty() {
        return None;
    }

    if let Some(first_p) = paragraphs.first() {
        if let Some(p_pr) = first_p.children().find(|n| n.has_tag_name("pPr")) {
            if p_pr
                .attribute("rtl")
                .map(|s| s == "1" || s == "true")
                .unwrap_or(false)
            {
                rtl = true;
            }
        }
    }

    for p in paragraphs {
        let mut p_text = String::new();
        for r in p.children().filter(|n| n.has_tag_name("r")) {
            if let Some(t) = r.children().find(|n| n.has_tag_name("t")) {
                p_text.push_str(t.text().unwrap_or(""));
            }
        }
        notes_lines.push(p_text);
    }

    if notes_lines.iter().all(|s| s.trim().is_empty()) {
        return None;
    }

    let mut sb = String::new();
    let dir_attr = if rtl { " dir=\"rtl\"" } else { "" };
    sb.push_str(&format!("  <div class=\"slide-notes\"{}>\n", dir_attr));
    sb.push_str("    <div class=\"slide-notes-label\">Notes</div>\n");
    sb.push_str("    <div class=\"slide-notes-body\">\n");
    for line in notes_lines {
        if line.trim().is_empty() {
            sb.push_str("      <br/>\n");
        } else {
            sb.push_str(&format!("      <div>{}</div>\n", html_escape(&line)));
        }
    }
    sb.push_str("    </div>\n  </div>\n");

    Some(sb)
}

// ==================== Text Lists & Bullet Counters ====================

fn to_alpha(mut n: usize, upper: bool) -> String {
    if n == 0 {
        n = 1;
    }
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        let base_char = if upper { b'A' } else { b'a' };
        let ch = (base_char + (n % 26) as u8) as char;
        s.insert(0, ch);
        n /= 26;
    }
    s
}

fn to_roman(mut n: usize) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let values = [1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1];
    let numerals = [
        "M", "CM", "D", "CD", "C", "XC", "L", "XL", "X", "IX", "V", "IV", "I",
    ];
    let mut s = String::new();
    for i in 0..values.len() {
        while n >= values[i] {
            s.push_str(numerals[i]);
            n -= values[i];
        }
    }
    s
}

fn format_auto_number_glyph(scheme: &str, n: usize) -> String {
    let body = if scheme.starts_with("alphaLc") || scheme.starts_with("AlphaLc") {
        to_alpha(n, false)
    } else if scheme.starts_with("alphaUc") || scheme.starts_with("AlphaUc") {
        to_alpha(n, true)
    } else if scheme.starts_with("romanLc") || scheme.starts_with("RomanLc") {
        to_roman(n).to_lowercase()
    } else if scheme.starts_with("romanUc") || scheme.starts_with("RomanUc") {
        to_roman(n)
    } else {
        n.to_string()
    };

    if scheme.ends_with("Period") {
        format!("{}.", body)
    } else if scheme.ends_with("ParenBoth") {
        format!("({})", body)
    } else if scheme.ends_with("ParenR") {
        format!("{})", body)
    } else if scheme.ends_with("Minus") {
        format!("- {} -", body)
    } else if scheme.ends_with("Plain") {
        body
    } else {
        format!("{}.", body)
    }
}

// ==================== Alternate Content & 3D ====================

fn render_alternate_content(
    ac: &roxmltree::Node,
    slide_path: &str,
    rels: &oxml::rels::Relationships,
    _theme_colors: &HashMap<String, String>,
    output: &mut String,
    package: &OxmlPackage,
) {
    let is_model_3d = ac.descendants().any(|d| d.has_tag_name("model3d"));
    let is_zoom = ac.descendants().any(|d| d.has_tag_name("sldZm"));
    if !is_model_3d && !is_zoom {
        return;
    }

    let choice = match ac.children().find(|n| n.has_tag_name("Choice")) {
        Some(c) => c,
        None => return,
    };
    let frame = match choice
        .children()
        .find(|n| n.has_tag_name("graphicFrame") || n.has_tag_name("sp"))
    {
        Some(f) => f,
        None => return,
    };
    let xfrm = match frame.descendants().find(|n| n.has_tag_name("xfrm")) {
        Some(x) => x,
        None => return,
    };

    let off = match xfrm.children().find(|n| n.has_tag_name("off")) {
        Some(o) => o,
        None => return,
    };
    let ext = match xfrm.children().find(|n| n.has_tag_name("ext")) {
        Some(e) => e,
        None => return,
    };

    let x = off
        .attribute("x")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let y = off
        .attribute("y")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let cx = ext
        .attribute("cx")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let cy = ext
        .attribute("cy")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    let left_pt = x / 12700.0;
    let top_pt = y / 12700.0;
    let width_pt = cx / 12700.0;
    let height_pt = cy / 12700.0;

    if is_model_3d {
        render_model_3d(
            &choice, slide_path, rels, left_pt, top_pt, width_pt, height_pt, output, package,
        );
    } else {
        render_zoom_fallback(
            &choice, slide_path, rels, left_pt, top_pt, width_pt, height_pt, output, package,
        );
    }
}

fn render_model_3d(
    choice: &roxmltree::Node,
    slide_path: &str,
    rels: &oxml::rels::Relationships,
    left_pt: f64,
    top_pt: f64,
    width_pt: f64,
    height_pt: f64,
    output: &mut String,
    package: &OxmlPackage,
) {
    let model3d = match choice.descendants().find(|n| n.has_tag_name("model3d")) {
        Some(m) => m,
        None => return,
    };
    let embed_id = match model3d
        .attribute((crate::dom_types::NS_R, "embed"))
        .or_else(|| model3d.attribute("r:embed"))
    {
        Some(id) => id,
        None => return,
    };

    let mut glb_b64 = String::new();
    let mut glb_file_name = "3D Model".to_string();

    if let Some(rel) = rels.get(embed_id) {
        let target_path = package.resolve_rel_target(slide_path, &rel.target);
        glb_file_name = std::path::Path::new(&target_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("model.glb")
            .to_string();
        if let Ok(bytes) = package.read_part_bytes(&target_path) {
            glb_b64 = base64_encode(&bytes);
        }
    }

    if glb_b64.is_empty() {
        return;
    }

    static mut MODEL_COUNTER: usize = 0;
    let m_id = unsafe {
        MODEL_COUNTER += 1;
        MODEL_COUNTER
    };

    let canvas_id = format!("model3d_{}", m_id);
    let container_id = format!("m3d_wrap_{}", canvas_id);
    let label = html_escape(&format!("3D Model: {}", glb_file_name));

    let mut rot_x = 0.0;
    let mut rot_y = 0.0;
    let mut rot_z = 0.0;
    if let Some(rot) = model3d.descendants().find(|n| n.has_tag_name("rot")) {
        let ax = rot
            .attribute("ax")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let ay = rot
            .attribute("ay")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let az = rot
            .attribute("az")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        rot_x = ax / 60000.0 * std::f64::consts::PI / 180.0;
        rot_y = ay / 60000.0 * std::f64::consts::PI / 180.0;
        rot_z = az / 60000.0 * std::f64::consts::PI / 180.0;
    }

    let mut fallback_img_src = None;
    if let Some(fallback) = choice
        .parent()
        .and_then(|p| p.children().find(|n| n.has_tag_name("Fallback")))
    {
        if let Some(fb_blip) = fallback.descendants().find(|n| n.has_tag_name("blip")) {
            if let Some(fb_embed) = fb_blip
                .attribute((crate::dom_types::NS_R, "embed"))
                .or_else(|| fb_blip.attribute("r:embed"))
            {
                if let Some(rel) = rels.get(fb_embed) {
                    let fb_target = package.resolve_rel_target(slide_path, &rel.target);
                    if let Ok(bytes) = package.read_part_bytes(&fb_target) {
                        fallback_img_src =
                            Some(format!("data:image/png;base64,{}", base64_encode(&bytes)));
                    }
                }
            }
        }
    }

    output.push_str(&format!(
        "    <div id=\"{}\" style=\"position:absolute;left:{:.2}pt;top:{:.2}pt;width:{:.2}pt;height:{:.2}pt;border:2px dashed rgba(108,117,125,0.6);border-radius:4px;background:rgba(248,249,250,0.7);overflow:hidden;box-sizing:border-box;\">\n",
        container_id, left_pt, top_pt, width_pt, height_pt
    ));
    output.push_str(&format!(
        "      <div class=\"m3d-label\" style=\"position:absolute;inset:0;display:flex;align-items:center;justify-content:center;font:11pt sans-serif;color:#495057;text-align:center;padding:4px;pointer-events:none;\">{}</div>\n",
        label
    ));
    output.push_str(&format!(
        "      <canvas id=\"{}\" style=\"position:relative;width:100%;height:100%;\"></canvas>\n",
        canvas_id
    ));
    if let Some(ref fb_src) = fallback_img_src {
        output.push_str(&format!("      <img class=\"m3d-fallback\" src=\"{}\" style=\"width:100%;height:100%;object-fit:contain;display:none;\" />\n", fb_src));
    }
    output.push_str("    </div>\n");

    let glb_var_name = format!("_glb_{}", canvas_id);
    output.push_str(&format!(
        "<script>window.{}='{}';</script>\n",
        glb_var_name, glb_b64
    ));

    output.push_str(&format!(r#"    <script type="module">
    let THREE, GLTFLoader;
    try {{
      THREE = await import('three');
      ({{ GLTFLoader }} = await import('three/addons/loaders/GLTFLoader.js'));
    }} catch(e) {{
      const c = document.getElementById('{canvas_id}');
      if (c) {{ c.style.display='none'; const fb=c.parentElement?.querySelector('.m3d-fallback'); if(fb) fb.style.display='block'; }}
      throw e;
    }}
    (function() {{
      const canvas = document.getElementById('{canvas_id}');
      if (!canvas) return;
      const container = canvas.parentElement;
      try {{
        const designW = {width_pt:.2} * 96 / 72;
        const designH = {height_pt:.2} * 96 / 72;
        canvas.width = designW * 2; canvas.height = designH * 2;
        canvas.style.width = '100%'; canvas.style.height = '100%';

        const w = designW, h = designH;
        const dpr = window.devicePixelRatio || 1;
        const renderer = new THREE.WebGLRenderer({{ canvas, alpha: true, antialias: true }});
        renderer.setSize(canvas.width / dpr, canvas.height / dpr);
        renderer.setPixelRatio(dpr);
        renderer.outputColorSpace = THREE.SRGBColorSpace;

        const scene = new THREE.Scene();
        const camera = new THREE.PerspectiveCamera(45, w / h, 0.01, 1000);

        scene.add(new THREE.AmbientLight(0x808080, 0.8));
        const key = new THREE.DirectionalLight(0xfff0e0, 1.2);
        key.position.set(2, 3, 4);
        scene.add(key);
        const fill = new THREE.DirectionalLight(0x6090e0, 0.6);
        fill.position.set(-3, 2, -1);
        scene.add(fill);
        const rim = new THREE.DirectionalLight(0xd0b0ff, 0.4);
        rim.position.set(-1, 1, -3);
        scene.add(rim);

        const b64 = window.{glb_var_name};
        const bin = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
        const loader = new GLTFLoader();
        loader.parse(bin.buffer, '', (gltf) => {{
          const model = gltf.scene;
          const box = new THREE.Box3().setFromObject(model);
          const center = box.getCenter(new THREE.Vector3());
          const size = box.getSize(new THREE.Vector3());
          model.position.sub(center);
          const maxDim = Math.max(size.x, size.y, size.z);
          const scale = 2.0 / maxDim;
          model.scale.setScalar(scale);
          model.rotation.x = {rot_x:.6};
          model.rotation.y = {rot_y:.6};
          model.rotation.z = {rot_z:.6};
          scene.add(model);
          camera.position.set(0, 0, 3.2);
          camera.lookAt(0, 0, 0);
          let baseRotY = {rot_y:.6};
          function animate() {{
            requestAnimationFrame(animate);
            baseRotY += 0.003;
            model.rotation.y = baseRotY;
            renderer.render(scene, camera);
          }}
          animate();
        }});
      }} catch(e) {{
        canvas.style.display = 'none';
        const fb = container?.querySelector('.m3d-fallback');
        if (fb) fb.style.display = 'block';
      }}
    }})();
    </script>
"#));
}

fn render_zoom_fallback(
    choice: &roxmltree::Node,
    slide_path: &str,
    rels: &oxml::rels::Relationships,
    left_pt: f64,
    top_pt: f64,
    width_pt: f64,
    height_pt: f64,
    output: &mut String,
    package: &OxmlPackage,
) {
    let mut fallback_img_src = None;
    if let Some(fallback) = choice
        .parent()
        .and_then(|p| p.children().find(|n| n.has_tag_name("Fallback")))
    {
        if let Some(fb_blip) = fallback.descendants().find(|n| n.has_tag_name("blip")) {
            if let Some(fb_embed) = fb_blip
                .attribute((crate::dom_types::NS_R, "embed"))
                .or_else(|| fb_blip.attribute("r:embed"))
            {
                if let Some(rel) = rels.get(fb_embed) {
                    let fb_target = package.resolve_rel_target(slide_path, &rel.target);
                    if let Ok(bytes) = package.read_part_bytes(&fb_target) {
                        fallback_img_src =
                            Some(format!("data:image/png;base64,{}", base64_encode(&bytes)));
                    }
                }
            }
        }
    }

    output.push_str(&format!(
        "    <div style=\"position:absolute;left:{:.2}pt;top:{:.2}pt;width:{:.2}pt;height:{:.2}pt;border:2px dashed rgba(255,193,7,0.6);border-radius:8px;overflow:hidden;\">\n",
        left_pt, top_pt, width_pt, height_pt
    ));
    if let Some(src) = fallback_img_src {
        output.push_str(&format!(
            "      <img src=\"{}\" style=\"width:100%;height:100%;object-fit:contain;\" />\n",
            src
        ));
    }
    output.push_str("    </div>\n");
}

fn render_ole_placeholder(
    gf: &roxmltree::Node,
    left_pt: f64,
    top_pt: f64,
    width_pt: f64,
    height_pt: f64,
    output: &mut String,
) {
    let prog_id = gf
        .descendants()
        .find(|n| n.has_tag_name("oleObj"))
        .and_then(|n| n.attribute("progId"))
        .unwrap_or("Embedded Object");
    let label = html_escape(&format!("OLE: {}", prog_id));
    output.push_str(&format!(
        "    <div class=\"ole-placeholder\" style=\"position:absolute;left:{:.2}pt;top:{:.2}pt;width:{:.2}pt;height:{:.2}pt;border:2px dashed rgba(108,117,125,0.6);border-radius:4px;display:flex;align-items:center;justify-content:center;font:11pt sans-serif;color:#495057;background:rgba(248,249,250,0.7);overflow:hidden;text-align:center;padding:4px;box-sizing:border-box;\">{}</div>\n",
        left_pt, top_pt, width_pt, height_pt, label
    ));
}

// ==================== Main Preview Engine ====================

fn get_layout_and_master_paths(
    package: &OxmlPackage,
    slide_path: &str,
) -> (Option<String>, Option<String>) {
    let mut layout_path = None;
    let mut master_path = None;

    if let Ok(slide_rels) = package.part_rels(slide_path) {
        if let Some(rel) = slide_rels.all().values().find(|r| {
            r.type_uri.contains("relationships/slideLayout")
                || r.type_uri.contains("relationships/layout")
        }) {
            let lp = package.resolve_rel_target(slide_path, &rel.target);
            layout_path = Some(lp.clone());

            if let Ok(layout_rels) = package.part_rels(&lp) {
                if let Some(m_rel) = layout_rels.all().values().find(|r| {
                    r.type_uri.contains("relationships/slideMaster")
                        || r.type_uri.contains("relationships/master")
                }) {
                    master_path = Some(package.resolve_rel_target(&lp, &m_rel.target));
                }
            }
        }
    }

    (layout_path, master_path)
}

/// Render the PowerPoint presentation as HTML for browser preview.
pub fn view_as_html(package: &OxmlPackage) -> Result<String, HandlerError> {
    let pres_xml = package.read_part_xml("ppt/presentation.xml").map_err(|e| {
        HandlerError::OperationFailed(format!("Failed to read presentation.xml: {}", e))
    })?;
    let doc = roxmltree::Document::parse(&pres_xml)
        .map_err(|e| HandlerError::OperationFailed(format!("roxmltree parse error: {}", e)))?;

    let sld_sz = doc
        .descendants()
        .find(|n| n.has_tag_name("sldSz") || n.has_tag_name((crate::dom_types::NS_P, "sldSz")));
    let (w_pt, h_pt) = if let Some(node) = sld_sz {
        let cx = node
            .attribute("cx")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(9144000.0);
        let cy = node
            .attribute("cy")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(6858000.0);
        (cx / 12700.0, cy / 12700.0)
    } else {
        (960.0, 540.0)
    };
    let aspect = w_pt / h_pt;

    let mut theme_colors = HashMap::new();
    theme_colors.insert("dk1".to_string(), "000000".to_string());
    theme_colors.insert("lt1".to_string(), "ffffff".to_string());
    theme_colors.insert("dk2".to_string(), "1F497D".to_string());
    theme_colors.insert("lt2".to_string(), "E5E0EC".to_string());
    theme_colors.insert("accent1".to_string(), "4F81BD".to_string());
    theme_colors.insert("accent2".to_string(), "C0504D".to_string());
    theme_colors.insert("accent3".to_string(), "9BBB59".to_string());
    theme_colors.insert("accent4".to_string(), "8064A2".to_string());
    theme_colors.insert("accent5".to_string(), "4BACC6".to_string());
    theme_colors.insert("accent6".to_string(), "F79646".to_string());
    theme_colors.insert("hlink".to_string(), "0000FF".to_string());

    if let Ok(theme_xml) = package.read_part_xml("ppt/theme/theme1.xml") {
        if let Ok(theme_doc) = roxmltree::Document::parse(&theme_xml) {
            if let Some(scheme) = theme_doc
                .descendants()
                .find(|n| n.has_tag_name("clrScheme"))
            {
                for child in scheme.children() {
                    let name = child.tag_name().name();
                    if let Some(srgb) = child.descendants().find(|n| n.has_tag_name("srgbClr")) {
                        if let Some(val) = srgb.attribute("val") {
                            theme_colors.insert(name.to_string(), val.to_string());
                        }
                    } else if let Some(sys) = child.descendants().find(|n| n.has_tag_name("sysClr"))
                    {
                        if let Some(val) = sys.attribute("lastClr") {
                            theme_colors.insert(name.to_string(), val.to_string());
                        }
                    }
                }
            }
        }
    }

    let presentation = crate::navigation::build_presentation(package)?;
    let slide_count = presentation.slides.len();
    let mut sidebar_thumbs = String::new();
    let mut slides_html = String::new();
    let mut has_math = false;

    for (i, slide) in presentation.slides.iter().enumerate() {
        let slide_num = i + 1;

        sidebar_thumbs.push_str(&format!(
            "  <div class=\"thumb\" data-slide=\"{}\">\n    <div class=\"thumb-inner\"></div>\n    <span class=\"thumb-num\">{}</span>\n  </div>\n",
            slide_num, slide_num
        ));

        slides_html.push_str(&format!(
            "<div class=\"slide-container\" data-slide=\"{}\">\n  <div class=\"slide-label\">Slide {}</div>\n  <div class=\"slide-wrapper\">\n",
            slide_num, slide_num
        ));

        let slide_xml = package.read_part_xml(&slide.part_path).map_err(|e| {
            HandlerError::OperationFailed(format!(
                "Failed to read slide part {}: {}",
                slide.part_path, e
            ))
        })?;
        if slide_xml.contains("<m:oMath") || slide_xml.contains("<oMath") {
            has_math = true;
        }
        let slide_doc = roxmltree::Document::parse(&slide_xml)
            .map_err(|e| HandlerError::OperationFailed(format!("roxmltree parse error: {}", e)))?;

        let slide_rels = package
            .part_rels(&slide.part_path)
            .unwrap_or_else(|_| oxml::rels::Relationships::empty());

        let (layout_path, master_path) = get_layout_and_master_paths(package, &slide.part_path);

        let layout_xml = layout_path
            .as_ref()
            .and_then(|p| package.read_part_xml(p).ok());
        let layout_doc = layout_xml
            .as_ref()
            .and_then(|xml| roxmltree::Document::parse(xml).ok());
        let layout_tree = layout_doc
            .as_ref()
            .and_then(|doc| doc.descendants().find(|n| n.has_tag_name("spTree")));

        let master_xml = master_path
            .as_ref()
            .and_then(|p| package.read_part_xml(p).ok());
        let master_doc = master_xml
            .as_ref()
            .and_then(|xml| roxmltree::Document::parse(xml).ok());
        let master_tree = master_doc
            .as_ref()
            .and_then(|doc| doc.descendants().find(|n| n.has_tag_name("spTree")));
        let master_text_styles = master_doc
            .as_ref()
            .and_then(|doc| doc.descendants().find(|n| n.has_tag_name("txStyles")));

        let mut slide_bg_style = String::new();
        if let Some(bg) = slide_doc.descendants().find(|n| n.has_tag_name("bg")) {
            if let Some(bg_pr) = bg.descendants().find(|n| n.has_tag_name("bgPr")) {
                if let Some(solid_fill) = bg_pr.descendants().find(|n| n.has_tag_name("solidFill"))
                {
                    if let Some(color) = resolve_fill_color(&solid_fill, &theme_colors) {
                        slide_bg_style.push_str(&format!("background:{};", color));
                    }
                } else if let Some(grad_fill) =
                    bg_pr.descendants().find(|n| n.has_tag_name("gradFill"))
                {
                    slide_bg_style.push_str(&format!(
                        "background:{};",
                        gradient_to_css(&grad_fill, &theme_colors)
                    ));
                } else if let Some(blip_fill) =
                    bg_pr.descendants().find(|n| n.has_tag_name("blipFill"))
                {
                    if let Some(blip) = blip_fill.descendants().find(|n| n.has_tag_name("blip")) {
                        if let Some(embed) = blip
                            .attribute((crate::dom_types::NS_R, "embed"))
                            .or_else(|| blip.attribute("r:embed"))
                        {
                            if let Some(rel) = slide_rels.get(embed) {
                                let target_path =
                                    package.resolve_rel_target(&slide.part_path, &rel.target);
                                if let Ok(bytes) = package.read_part_bytes(&target_path) {
                                    let b64 = base64_encode(&bytes);
                                    let mut opacity_style = String::new();
                                    if let Some(alpha_mod) =
                                        blip.children().find(|n| n.has_tag_name("alphaModFix"))
                                    {
                                        if let Some(amt) = alpha_mod
                                            .attribute("amt")
                                            .and_then(|s| s.parse::<f64>().ok())
                                        {
                                            if amt < 100000.0 {
                                                opacity_style =
                                                    format!("opacity:{:.2};", amt / 100000.0);
                                            }
                                        }
                                    }
                                    slide_bg_style.push_str(&format!("background:url('data:image/png;base64,{}') center/cover no-repeat;{}", b64, opacity_style));
                                }
                            }
                        }
                    }
                }
            }
        }

        slides_html.push_str(&format!(
            "    <div class=\"slide\" style=\"{}\">\n",
            slide_bg_style
        ));

        // Walk layout and master to render non-text background placeholder layers
        if let Some(ref tree) = layout_tree {
            for child in tree.children() {
                let tag = child.tag_name().name();
                if tag == "sp" {
                    // Make sure it doesn't leak layout prompt texts
                    if child.children().any(|n| {
                        n.has_tag_name("spPr")
                            && n.children()
                                .any(|c| c.has_tag_name("solidFill") || c.has_tag_name("gradFill"))
                    }) {
                        let mut prompt_shape_html = String::new();
                        render_shape(
                            &child,
                            slide_num,
                            &slide.part_path,
                            &slide_rels,
                            &theme_colors,
                            &mut prompt_shape_html,
                            package,
                            layout_tree,
                            master_tree,
                            master_text_styles,
                            None,
                        );
                        slides_html.push_str(&prompt_shape_html);
                    }
                } else if tag == "pic" {
                    let mut pic_html = String::new();
                    render_picture(
                        &child,
                        slide_num,
                        &slide.part_path,
                        &slide_rels,
                        &mut pic_html,
                        package,
                        &theme_colors,
                        None,
                    );
                    slides_html.push_str(&pic_html);
                }
            }
        }

        if let Some(ref tree) = master_tree {
            for child in tree.children() {
                let tag = child.tag_name().name();
                if tag == "sp" {
                    if child.children().any(|n| {
                        n.has_tag_name("spPr")
                            && n.children()
                                .any(|c| c.has_tag_name("solidFill") || c.has_tag_name("gradFill"))
                    }) {
                        let mut prompt_shape_html = String::new();
                        render_shape(
                            &child,
                            slide_num,
                            &slide.part_path,
                            &slide_rels,
                            &theme_colors,
                            &mut prompt_shape_html,
                            package,
                            layout_tree,
                            master_tree,
                            master_text_styles,
                            None,
                        );
                        slides_html.push_str(&prompt_shape_html);
                    }
                } else if tag == "pic" {
                    let mut pic_html = String::new();
                    render_picture(
                        &child,
                        slide_num,
                        &slide.part_path,
                        &slide_rels,
                        &mut pic_html,
                        package,
                        &theme_colors,
                        None,
                    );
                    slides_html.push_str(&pic_html);
                }
            }
        }

        // Render slide actual elements (on top of master/layout background layers)
        let sp_tree = slide_doc.descendants().find(|n| {
            n.has_tag_name("spTree") || n.has_tag_name((crate::dom_types::NS_P, "spTree"))
        });
        if let Some(tree) = sp_tree {
            for child in tree.children() {
                let tag = child.tag_name().name();
                if tag == "sp" {
                    render_shape(
                        &child,
                        slide_num,
                        &slide.part_path,
                        &slide_rels,
                        &theme_colors,
                        &mut slides_html,
                        package,
                        layout_tree,
                        master_tree,
                        master_text_styles,
                        None,
                    );
                } else if tag == "pic" {
                    render_picture(
                        &child,
                        slide_num,
                        &slide.part_path,
                        &slide_rels,
                        &mut slides_html,
                        package,
                        &theme_colors,
                        None,
                    );
                } else if tag == "graphicFrame" {
                    if child.descendants().any(|n| n.has_tag_name("tbl")) {
                        let xfrm = child.descendants().find(|n| n.has_tag_name("xfrm"));
                        if let Some(x_node) = xfrm {
                            let off = x_node.descendants().find(|n| n.has_tag_name("off"));
                            let ext = x_node.descendants().find(|n| n.has_tag_name("ext"));
                            if let (Some(o), Some(e)) = (off, ext) {
                                let x = o
                                    .attribute("x")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                let y = o
                                    .attribute("y")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                let cx = e
                                    .attribute("cx")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                let cy = e
                                    .attribute("cy")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                render_table(
                                    &child,
                                    slide_num,
                                    &theme_colors,
                                    &mut slides_html,
                                    x,
                                    y,
                                    cx,
                                    cy,
                                );
                            }
                        }
                    } else if child.descendants().any(|n| n.has_tag_name("oleObj")) {
                        let xfrm = child.descendants().find(|n| n.has_tag_name("xfrm"));
                        if let Some(x_node) = xfrm {
                            let off = x_node.descendants().find(|n| n.has_tag_name("off"));
                            let ext = x_node.descendants().find(|n| n.has_tag_name("ext"));
                            if let (Some(o), Some(e)) = (off, ext) {
                                let x = o
                                    .attribute("x")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                let y = o
                                    .attribute("y")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                let cx = e
                                    .attribute("cx")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                let cy = e
                                    .attribute("cy")
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .unwrap_or(0.0)
                                    / 12700.0;
                                render_ole_placeholder(&child, x, y, cx, cy, &mut slides_html);
                            }
                        }
                    }
                } else if tag == "grpSp" {
                    render_group_shape(
                        &child,
                        slide_num,
                        &slide.part_path,
                        &slide_rels,
                        &theme_colors,
                        &mut slides_html,
                        package,
                        layout_tree,
                        master_tree,
                        master_text_styles,
                    );
                } else if tag == "cxnSp" {
                    render_connector(&child, &theme_colors, &mut slides_html, None);
                } else if child.tag_name().name() == "AlternateContent" {
                    render_alternate_content(
                        &child,
                        &slide.part_path,
                        &slide_rels,
                        &theme_colors,
                        &mut slides_html,
                        package,
                    );
                }
            }
        }

        slides_html.push_str("    </div>\n  </div>\n");

        if let Some(notes) = get_speaker_notes(package, &slide.part_path) {
            slides_html.push_str(&notes);
        }

        slides_html.push_str("</div>\n");
    }

    let mut head_injections = String::new();
    if has_math {
        head_injections.push_str("<link rel=\"stylesheet\" href=\"https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.css\" media=\"print\" onload=\"this.media='all'\" onerror=\"this.remove()\">\n");
        head_injections.push_str("<script defer src=\"https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.js\" onerror=\"document.querySelectorAll('.katex-formula').forEach(function(el){el.textContent=el.dataset.formula;el.style.fontFamily='monospace';el.style.color='#666'})\"></script>\n");
    }
    head_injections.push_str("<script type=\"importmap\">{\"imports\":{\"three\":\"https://cdn.jsdelivr.net/npm/three@0.170.0/build/three.module.js\",\"three/addons/\":\"https://cdn.jsdelivr.net/npm/three@0.170.0/examples/jsm/\"}}</script>\n");

    let file_name = "Presentation Preview";

    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{file_name}</title>
{head_injections}<style>
:root {{
  --slide-design-w: {w_pt:.0}pt;
  --slide-design-h: {h_pt:.0}pt;
  --slide-aspect: {aspect:.4};
}}
{PREVIEW_CSS}
</style>
<script>if(navigator.webdriver||/HeadlessChrome/.test(navigator.userAgent))document.documentElement.classList.add('headless')</script>
</head>
<body>
<div class="toggle-zone"></div><button class="sidebar-toggle" onclick="toggleSidebar()">☰</button>
<div class="sidebar">
  <div class="sidebar-title">{file_name}</div>
{sidebar_thumbs}</div>
<div class="main">
  <h1 class="file-title">{file_name}</h1>
{slides_html}</div>
<div class="page-counter">1 / {slide_count}</div>
<script>
{PREVIEW_JS}
</script>
<script>
(function() {{
    var _katexRetries = 0;
    function fallbackKatex() {{
        document.querySelectorAll('.katex-formula:not(.katex-rendered)').forEach(function(el) {{
            el.textContent = el.dataset.formula;
            el.style.fontFamily = 'monospace';
            el.style.color = '#666';
            el.classList.add('katex-rendered');
        }});
    }}
    function renderKatex() {{
        var pending = document.querySelectorAll('.katex-formula:not(.katex-rendered)');
        if (pending.length === 0) return;
        if (typeof katex === 'undefined') {{
            if (!window._katexLoading) {{
                window._katexLoading = true;
                var link = document.createElement('link');
                link.rel = 'stylesheet';
                link.href = 'https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.css';
                link.onerror = function() {{ this.remove(); }};
                document.head.appendChild(link);
                var script = document.createElement('script');
                script.src = 'https://cdn.jsdelivr.net/npm/katex@0.16.11/dist/katex.min.js';
                script.onload = renderKatex;
                script.onerror = fallbackKatex;
                document.head.appendChild(script);
                return;
            }}
            if (++_katexRetries > 20) {{ fallbackKatex(); return; }}
            setTimeout(renderKatex, 100); return;
        }}
        pending.forEach(function(el) {{
            try {{
                katex.render(el.dataset.formula, el, {{ throwOnError: false, displayMode: true }});
                el.classList.add('katex-rendered');
            }} catch(e) {{ el.textContent = el.dataset.formula + ' (Error: ' + e.message + '. See https://katex.org/docs/supported.html for supported syntax.)'; }}
        }});
    }}
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', renderKatex);
    else renderKatex();
    new MutationObserver(function() {{ renderKatex(); }}).observe(document.body, {{ childList: true, subtree: true }});
}})();
</script>
</body>
</html>"#,
        file_name = file_name,
        head_injections = head_injections,
        sidebar_thumbs = sidebar_thumbs,
        slides_html = slides_html,
        slide_count = slide_count
    ))
}

fn render_shape(
    node: &roxmltree::Node,
    slide_num: usize,
    slide_path: &str,
    rels: &oxml::rels::Relationships,
    theme_colors: &HashMap<String, String>,
    output: &mut String,
    package: &OxmlPackage,
    layout_tree: Option<roxmltree::Node<'_, '_>>,
    master_tree: Option<roxmltree::Node<'_, '_>>,
    master_text_styles: Option<roxmltree::Node<'_, '_>>,
    override_pos: Option<(f64, f64, f64, f64)>,
) {
    let nv_sp_pr = node.descendants().find(|n| n.has_tag_name("nvSpPr"));
    let mut name = String::new();
    let mut id = String::new();
    let mut ph_type = None;
    let mut ph_idx = None;

    if let Some(nv) = nv_sp_pr {
        if let Some(c_nv_pr) = nv.descendants().find(|n| n.has_tag_name("cNvPr")) {
            name = c_nv_pr.attribute("name").unwrap_or("").to_string();
            id = c_nv_pr.attribute("id").unwrap_or("").to_string();
        }
        if let Some(ph) = nv.descendants().find(|n| n.has_tag_name("ph")) {
            ph_type = ph.attribute("type");
            ph_idx = ph.attribute("idx").and_then(|s| s.parse::<usize>().ok());
        }
    }

    let xfrm = node.descendants().find(|n| n.has_tag_name("xfrm"));
    let mut x_pt = 0.0;
    let mut y_pt = 0.0;
    let mut cx_pt = 0.0;
    let mut cy_pt = 0.0;
    let mut rot_deg = 0.0;
    let mut flip_h = false;
    let mut flip_v = false;

    if let Some(pos) = override_pos {
        x_pt = pos.0 / 12700.0;
        y_pt = pos.1 / 12700.0;
        cx_pt = pos.2 / 12700.0;
        cy_pt = pos.3 / 12700.0;
    } else if let Some(x_node) = xfrm {
        if let Some(off) = x_node.descendants().find(|n| n.has_tag_name("off")) {
            let x = off
                .attribute("x")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let y = off
                .attribute("y")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            x_pt = x / 12700.0;
            y_pt = y / 12700.0;
        }
        if let Some(ext) = x_node.descendants().find(|n| n.has_tag_name("ext")) {
            let cx = ext
                .attribute("cx")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let cy = ext
                .attribute("cy")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            cx_pt = cx / 12700.0;
            cy_pt = cy / 12700.0;
        }
        if let Some(rot) = x_node.attribute("rot").and_then(|s| s.parse::<f64>().ok()) {
            rot_deg = rot / 60000.0;
        }
        flip_h = x_node
            .attribute("flipH")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false);
        flip_v = x_node
            .attribute("flipV")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false);
    } else {
        if let Some((x, y, cx, cy)) =
            resolve_inherited_position(ph_type, ph_idx, layout_tree, master_tree)
        {
            x_pt = x / 12700.0;
            y_pt = y / 12700.0;
            cx_pt = cx / 12700.0;
            cy_pt = cy / 12700.0;
        } else {
            let widescreen_w = 9144000.0;
            let widescreen_h = 5143500.0;
            let margin = widescreen_w / 16.0;
            let content_w = widescreen_w - margin * 2.0;

            if let Some(ty) = ph_type {
                if ty == "title" || ty == "ctrTitle" {
                    x_pt = margin / 12700.0;
                    y_pt = (widescreen_h / 8.0) / 12700.0;
                    cx_pt = content_w / 12700.0;
                    cy_pt = (widescreen_h / 4.0) / 12700.0;
                } else if ty == "subTitle" {
                    x_pt = margin / 12700.0;
                    y_pt = (widescreen_h * 3.0 / 8.0) / 12700.0;
                    cx_pt = content_w / 12700.0;
                    cy_pt = (widescreen_h / 4.0) / 12700.0;
                } else {
                    x_pt = margin / 12700.0;
                    y_pt = (widescreen_h * 3.0 / 8.0) / 12700.0;
                    cx_pt = content_w / 12700.0;
                    cy_pt = (widescreen_h / 2.0) / 12700.0;
                }
            } else if ph_idx.is_some() {
                x_pt = margin / 12700.0;
                y_pt = (widescreen_h / 4.0) / 12700.0;
                cx_pt = content_w / 12700.0;
                cy_pt = (widescreen_h / 2.0) / 12700.0;
            } else {
                return;
            }
        }
    }

    let mut styles = vec![
        format!("left:{:.2}pt", x_pt),
        format!("top:{:.2}pt", y_pt),
        format!("width:{:.2}pt", cx_pt),
        format!("height:{:.2}pt", cy_pt),
    ];

    let sp_pr = node.descendants().find(|n| n.has_tag_name("spPr"));
    let mut border_radius = String::new();
    let mut clip_path = String::new();
    let mut parsed_outline = None;

    if let Some(sp_pr_node) = sp_pr {
        if let Some(solid_fill) = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("solidFill"))
        {
            if let Some(color) = resolve_fill_color(&solid_fill, theme_colors) {
                let fill_style = format!("background:{}", color);
                styles.push(fill_style.clone());
            }
        } else if let Some(grad_fill) = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("gradFill"))
        {
            let fill_style = format!("background:{}", gradient_to_css(&grad_fill, theme_colors));
            styles.push(fill_style.clone());
        } else if let Some(blip_fill) = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("blipFill"))
        {
            if let Some(blip) = blip_fill.descendants().find(|n| n.has_tag_name("blip")) {
                if let Some(embed) = blip
                    .attribute((crate::dom_types::NS_R, "embed"))
                    .or_else(|| blip.attribute("r:embed"))
                {
                    if let Some(rel) = rels.get(embed) {
                        let target_path = package.resolve_rel_target(slide_path, &rel.target);
                        if let Ok(bytes) = package.read_part_bytes(&target_path) {
                            let b64 = base64_encode(&bytes);
                            let fill_style = format!(
                                "background:url('data:image/png;base64,{}') center/cover no-repeat",
                                b64
                            );
                            styles.push(fill_style.clone());
                        }
                    }
                }
            }
        } else if sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("noFill"))
            .is_some()
        {
            styles.push("background:transparent".to_string());
        }

        if let Some(prst_geom) = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("prstGeom"))
        {
            if let Some(prst) = prst_geom.attribute("prst") {
                let geom_css = preset_geometry_to_css(prst, cx_pt, cy_pt, &prst_geom);
                if !geom_css.is_empty() {
                    if geom_css.starts_with("clip-path:") {
                        clip_path = geom_css;
                    } else {
                        border_radius = geom_css.clone();
                        styles.push(border_radius.clone());
                    }
                }
            }
        } else if let Some(cust_geom) = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("custGeom"))
        {
            let geom_css = custom_geometry_to_clip_path(&cust_geom);
            if !geom_css.is_empty() {
                clip_path = geom_css;
            }
        }

        if let Some(ln) = sp_pr_node.descendants().find(|n| n.has_tag_name("ln")) {
            parsed_outline = parse_outline(&ln, theme_colors);
            if let Some((width_pt, ref prst_dash, ref color)) = parsed_outline {
                if prst_dash == "solid" {
                    styles.push(format!("border:{:.2}pt solid {}", width_pt, color));
                }
            }
        }

        let effect_list = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("effectLst"));
        let mut filter_effects = Vec::new();
        if let Some(ref eff) = effect_list {
            let shadow_css = effect_list_to_shadow_css(eff, theme_colors);
            let glow_css = effect_list_to_glow_css(eff, theme_colors);
            if !shadow_css.is_empty() {
                filter_effects.push(shadow_css);
            }
            if !glow_css.is_empty() {
                filter_effects.push(glow_css);
            }

            let reflection_css = effect_list_to_reflection_css(eff);
            if !reflection_css.is_empty() {
                styles.push(reflection_css);
            }

            if let Some(soft_edge) = eff.children().find(|n| n.has_tag_name("softEdge")) {
                if let Some(rad) = soft_edge
                    .attribute("rad")
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    let edge_px = (rad / 12700.0 * 0.8).max(2.0);
                    styles.push(format!("-webkit-mask-image:linear-gradient(to right,transparent 0,black {:.1}px,black calc(100% - {:.1}px),transparent 100%),linear-gradient(to bottom,transparent 0,black {:.1}px,black calc(100% - {:.1}px),transparent 100%)", edge_px, edge_px, edge_px, edge_px));
                    styles.push(
                        "-webkit-mask-composite:source-in;mask-composite:intersect".to_string(),
                    );
                }
            }
        }

        if !filter_effects.is_empty() {
            styles.push(format!("filter:{}", filter_effects.join(" ")));
        }

        if let Some(sp3d) = sp_pr_node.descendants().find(|n| n.has_tag_name("sp3d")) {
            if sp3d.children().any(|n| n.has_tag_name("bevelT")) {
                let bevel_w = sp3d
                    .children()
                    .find(|n| n.has_tag_name("bevelT"))
                    .and_then(|n| n.attribute("w"))
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(76200.0)
                    / 12700.0;
                let bw = (bevel_w * 0.5).max(1.0);
                styles.push(format!("box-shadow:inset {:.1}px {:.1}px {:.1}px rgba(255,255,255,0.25),inset -{:.1}px -{:.1}px {:.1}px rgba(0,0,0,0.15)", bw, bw, bw * 1.5, bw, bw, bw * 1.5));
            }
        }
    }

    let mut transforms = Vec::new();
    if rot_deg != 0.0 {
        transforms.push(format!("rotate({:.2}deg)", rot_deg));
    }
    if flip_h && flip_v {
        transforms.push("scale(-1,-1)".to_string());
    } else if flip_h {
        transforms.push("scaleX(-1)".to_string());
    } else if flip_v {
        transforms.push("scaleY(-1)".to_string());
    }
    if !transforms.is_empty() {
        styles.push(format!("transform:{}", transforms.join(" ")));
    }

    let tx_body = node.descendants().find(|n| n.has_tag_name("txBody"));
    let mut valign = "top";
    let mut wrap_none = false;
    let mut l_ins = 7.2;
    let mut t_ins = 3.6;
    let mut r_ins = 7.2;
    let mut b_ins = 3.6;

    if let Some(ref tx) = tx_body {
        if let Some(body_pr) = tx.descendants().find(|n| n.has_tag_name("bodyPr")) {
            if let Some(anchor) = body_pr.attribute("anchor") {
                valign = match anchor {
                    "ctr" => "center",
                    "b" => "bottom",
                    _ => "top",
                };
            }
            if body_pr
                .attribute("wrap")
                .map(|s| s == "none")
                .unwrap_or(false)
            {
                wrap_none = true;
            }
            if let Some(l) = body_pr
                .attribute("lIns")
                .and_then(|s| s.parse::<f64>().ok())
            {
                l_ins = l / 12700.0;
            }
            if let Some(t) = body_pr
                .attribute("tIns")
                .and_then(|s| s.parse::<f64>().ok())
            {
                t_ins = t / 12700.0;
            }
            if let Some(r) = body_pr
                .attribute("rIns")
                .and_then(|s| s.parse::<f64>().ok())
            {
                r_ins = r / 12700.0;
            }
            if let Some(b) = body_pr
                .attribute("bIns")
                .and_then(|s| s.parse::<f64>().ok())
            {
                b_ins = b / 12700.0;
            }
        }
    }

    if !clip_path.is_empty() {
        let (pct_l, pct_t, pct_r, pct_b) = match name.as_str() {
            "diamond" => (0.25, 0.25, 0.25, 0.25),
            "triangle" | "isosTriangle" => (0.20, 0.20, 0.20, 0.0),
            "rtTriangle" => (0.0, 0.15, 0.15, 0.0),
            "star5" | "star4" | "star6" => (0.25, 0.25, 0.25, 0.25),
            "hexagon" => (0.25, 0.10, 0.25, 0.10),
            "pentagon" => (0.12, 0.12, 0.12, 0.0),
            _ => (0.0, 0.0, 0.0, 0.0),
        };
        if pct_l > 0.0 || pct_t > 0.0 || pct_r > 0.0 || pct_b > 0.0 {
            l_ins = l_ins.max(cx_pt * pct_l);
            t_ins = t_ins.max(cy_pt * pct_t);
            r_ins = r_ins.max(cx_pt * pct_r);
            b_ins = b_ins.max(cy_pt * pct_b);
        }
    }

    styles.push(format!(
        "padding:{:.2}pt {:.2}pt {:.2}pt {:.2}pt",
        t_ins, r_ins, b_ins, l_ins
    ));

    if wrap_none {
        styles.push("overflow:visible".to_string());
    }

    let mut shape_href_url = None;
    if let Some(nv) = nv_sp_pr {
        if let Some(hlink) = nv.descendants().find(|n| n.has_tag_name("hlinkClick")) {
            if let Some(r_id) = hlink
                .attribute((crate::dom_types::NS_R, "id"))
                .or_else(|| hlink.attribute("r:id"))
            {
                if let Some(rel) = rels.get(r_id) {
                    shape_href_url = Some(rel.target.clone());
                }
            }
        }
    }

    let path_attr = format!(
        " data-path=\"/slide[{}]/shape[{}]\" title=\"{}\"",
        slide_num,
        id,
        html_escape(&name)
    );

    if let Some(ref url) = shape_href_url {
        output.push_str(&format!("    <a class=\"shape-link\" href=\"{}\" rel=\"noopener\" target=\"_blank\" style=\"display:contents;cursor:pointer;\">\n", html_escape(url)));
    }

    if !clip_path.is_empty() {
        let mut outer_styles = Vec::new();
        let mut fill_styles = Vec::new();
        for s in &styles {
            if s.starts_with("background:") || s.starts_with("background-image:") {
                fill_styles.push(s.clone());
            } else {
                outer_styles.push(s.clone());
            }
        }
        output.push_str(&format!(
            "    <div class=\"shape\"{} style=\"{}\">\n",
            path_attr,
            outer_styles.join(";")
        ));
        if !fill_styles.is_empty() {
            output.push_str(&format!(
                "      <div style=\"position:absolute;inset:0;{};{}\"></div>\n",
                clip_path,
                fill_styles.join(";")
            ));
        }

        if let Some((bw, ref dt, ref bc)) = parsed_outline {
            if dt != "solid" && clip_path.starts_with("clip-path:polygon(") {
                let poly_str = &clip_path["clip-path:polygon(".len()..clip_path.len() - 1];
                let svg_points = poly_str.replace('%', "");
                let dash_arr = dash_type_to_svg_dasharray(dt, bw);
                let dash_attr = if !dash_arr.is_empty() {
                    format!(" stroke-dasharray=\"{}\"", dash_arr)
                } else {
                    "".to_string()
                };
                output.push_str(&format!("      <svg style=\"position:absolute;inset:0;width:100%;height:100%;overflow:visible\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\">\n"));
                output.push_str(&format!("        <polygon points=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{:.2}pt\" vector-effect=\"non-scaling-stroke\" stroke-linecap=\"butt\"{}/>\n", svg_points, bc, bw, dash_attr));
                output.push_str("      </svg>\n");
            }
        }
    } else {
        output.push_str(&format!(
            "    <div class=\"shape\"{} style=\"{}\">\n",
            path_attr,
            styles.join(";")
        ));
    }

    if let Some(tx) = tx_body {
        let wrap_style = if wrap_none {
            " style=\"white-space:nowrap;overflow:visible;\""
        } else {
            ""
        };
        output.push_str(&format!(
            "      <div class=\"shape-text valign-{}\"{}>\n",
            valign, wrap_style
        ));

        let mut text_output = String::new();
        render_text_body_with_context(
            &tx,
            theme_colors,
            &mut text_output,
            Some(*node),
            Some(package),
            layout_tree,
            master_tree,
            master_text_styles,
            rels,
        );
        output.push_str(&text_output);

        output.push_str("      </div>\n");
    }

    if let Some((bw, ref dt, ref bc)) = parsed_outline {
        if dt != "solid" && clip_path.is_empty() {
            let dash_arr = dash_type_to_svg_dasharray(dt, bw);
            let dash_attr = if !dash_arr.is_empty() {
                format!(" stroke-dasharray=\"{}\"", dash_arr)
            } else {
                "".to_string()
            };
            if !border_radius.is_empty() {
                let rx = if border_radius.contains("50%") {
                    "50%"
                } else {
                    "6"
                };
                output.push_str(&format!("      <svg style=\"position:absolute;inset:0;width:100%;height:100%;overflow:visible\">\n"));
                output.push_str(&format!("        <rect x=\"{:.1}pt\" y=\"{:.1}pt\" width=\"calc(100% - {:.1}pt)\" height=\"calc(100% - {:.1}pt)\" rx=\"{}\" ry=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{:.2}pt\" stroke-linecap=\"butt\"{}/>\n", bw/2.0, bw/2.0, bw, bw, rx, rx, bc, bw, dash_attr));
                output.push_str("      </svg>\n");
            } else {
                output.push_str(&format!("      <svg style=\"position:absolute;inset:0;width:100%;height:100%;overflow:visible\">\n"));
                output.push_str(&format!("        <rect x=\"{:.1}pt\" y=\"{:.1}pt\" width=\"calc(100% - {:.1}pt)\" height=\"calc(100% - {:.1}pt)\" fill=\"none\" stroke=\"{}\" stroke-width=\"{:.2}pt\" stroke-linecap=\"butt\"{}/>\n", bw/2.0, bw/2.0, bw, bw, bc, bw, dash_attr));
                output.push_str("      </svg>\n");
            }
        }
    }

    output.push_str("    </div>\n");

    if shape_href_url.is_some() {
        output.push_str("    </a>\n");
    }
}

fn render_picture(
    node: &roxmltree::Node,
    slide_num: usize,
    slide_path: &str,
    rels: &oxml::rels::Relationships,
    output: &mut String,
    package: &OxmlPackage,
    theme_colors: &HashMap<String, String>,
    override_pos: Option<(f64, f64, f64, f64)>,
) {
    let xfrm = node.descendants().find(|n| n.has_tag_name("xfrm"));
    let mut x_pt = 0.0;
    let mut y_pt = 0.0;
    let mut cx_pt = 0.0;
    let mut cy_pt = 0.0;
    let mut rot_deg = 0.0;
    let mut flip_h = false;
    let mut flip_v = false;

    if let Some(pos) = override_pos {
        x_pt = pos.0 / 12700.0;
        y_pt = pos.1 / 12700.0;
        cx_pt = pos.2 / 12700.0;
        cy_pt = pos.3 / 12700.0;
    } else if let Some(x_node) = xfrm {
        if let Some(off) = x_node.descendants().find(|n| n.has_tag_name("off")) {
            let x = off
                .attribute("x")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let y = off
                .attribute("y")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            x_pt = x / 12700.0;
            y_pt = y / 12700.0;
        }
        if let Some(ext) = x_node.descendants().find(|n| n.has_tag_name("ext")) {
            let cx = ext
                .attribute("cx")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let cy = ext
                .attribute("cy")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            cx_pt = cx / 12700.0;
            cy_pt = cy / 12700.0;
        }
        if let Some(rot) = x_node.attribute("rot").and_then(|s| s.parse::<f64>().ok()) {
            rot_deg = rot / 60000.0;
        }
        flip_h = x_node
            .attribute("flipH")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false);
        flip_v = x_node
            .attribute("flipV")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false);
    } else {
        return;
    }

    let mut styles = vec![
        format!("left:{:.2}pt", x_pt),
        format!("top:{:.2}pt", y_pt),
        format!("width:{:.2}pt", cx_pt),
        format!("height:{:.2}pt", cy_pt),
    ];

    let sp_pr = node.descendants().find(|n| n.has_tag_name("spPr"));
    if let Some(ref sp_pr_node) = sp_pr {
        if let Some(ln) = sp_pr_node.descendants().find(|n| n.has_tag_name("ln")) {
            let border_css = outline_to_css(&ln, theme_colors);
            if !border_css.is_empty() {
                styles.push(border_css);
            }
        }
    }

    let mut filter_effects = Vec::new();
    let mut opacity_val = 1.0;
    let mut reflection_css = String::new();
    let mut brightness_pct = None;
    let mut contrast_pct = None;

    let blip_fill = node.descendants().find(|n| n.has_tag_name("blipFill"));
    let blip = blip_fill
        .as_ref()
        .and_then(|bf| bf.descendants().find(|n| n.has_tag_name("blip")));

    if let Some(ref sp_pr_node) = sp_pr {
        let effect_list = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("effectLst"));
        if let Some(ref eff) = effect_list {
            let shadow_css = effect_list_to_shadow_css(eff, theme_colors);
            let glow_css = effect_list_to_glow_css(eff, theme_colors);
            if !shadow_css.is_empty() {
                filter_effects.push(shadow_css);
            }
            if !glow_css.is_empty() {
                filter_effects.push(glow_css);
            }
            reflection_css = effect_list_to_reflection_css(eff);
        }
    }

    if let Some(ref b) = blip {
        for kid in b.children() {
            let tag = kid.tag_name().name();
            if tag == "lum" {
                if let Some(bright) = kid.attribute("bright").and_then(|s| s.parse::<f64>().ok()) {
                    brightness_pct = Some(bright / 1000.0);
                }
                if let Some(contrast) = kid
                    .attribute("contrast")
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    contrast_pct = Some(contrast / 1000.0);
                }
            } else if tag == "lumOff" {
                if let Some(val) = kid.attribute("val").and_then(|s| s.parse::<f64>().ok()) {
                    brightness_pct = Some(val / 1000.0);
                }
            } else if tag == "lumMod" {
                if let Some(val) = kid.attribute("val").and_then(|s| s.parse::<f64>().ok()) {
                    contrast_pct = Some((val - 100000.0) / 1000.0);
                }
            }
        }
        if let Some(alpha_mod) = b.children().find(|n| n.has_tag_name("alphaModFix")) {
            if let Some(amt) = alpha_mod
                .attribute("amt")
                .and_then(|s| s.parse::<f64>().ok())
            {
                opacity_val = amt / 100000.0;
            }
        }
    }

    if let Some(b) = brightness_pct {
        filter_effects.push(format!("brightness({:.3})", 1.0 + b / 100.0));
    }
    if let Some(c) = contrast_pct {
        filter_effects.push(format!("contrast({:.3})", 1.0 + c / 100.0));
    }
    if !filter_effects.is_empty() {
        styles.push(format!("filter:{}", filter_effects.join(" ")));
    }
    if opacity_val < 1.0 {
        styles.push(format!("opacity:{:.3}", opacity_val));
    }
    if !reflection_css.is_empty() {
        styles.push(reflection_css);
    }

    if let Some(ref sp_pr_node) = sp_pr {
        if let Some(preset_geom) = sp_pr_node
            .descendants()
            .find(|n| n.has_tag_name("prstGeom"))
        {
            if let Some(preset) = preset_geom.attribute("prst") {
                let geom_css = preset_geometry_to_css(preset, cx_pt, cy_pt, &preset_geom);
                if !geom_css.is_empty() {
                    styles.push(geom_css);
                }
            }
        }
    }

    let mut transforms = Vec::new();
    if rot_deg != 0.0 {
        transforms.push(format!("rotate({:.2}deg)", rot_deg));
    }
    if flip_h && flip_v {
        transforms.push("scale(-1,-1)".to_string());
    } else if flip_h {
        transforms.push("scaleX(-1)".to_string());
    } else if flip_v {
        transforms.push("scaleY(-1)".to_string());
    }
    if !transforms.is_empty() {
        styles.push(format!("transform:{}", transforms.join(" ")));
    }

    let mut data_uri = String::new();
    let mut src_rect = None;

    if let Some(ref b) = blip {
        if let Some(embed) = b
            .attribute((crate::dom_types::NS_R, "embed"))
            .or_else(|| b.attribute("r:embed"))
        {
            if let Some(rel) = rels.get(embed) {
                let target_path = package.resolve_rel_target(slide_path, &rel.target);
                if let Ok(bytes) = package.read_part_bytes(&target_path) {
                    let ext = std::path::Path::new(&target_path)
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("png")
                        .to_lowercase();
                    let mime = match ext.as_str() {
                        "jpg" | "jpeg" => "image/jpeg",
                        "gif" => "image/gif",
                        "svg" => "image/svg+xml",
                        _ => "image/png",
                    };
                    data_uri = format!("data:{};base64,{}", mime, base64_encode(&bytes));
                }
            }
        }
        if let Some(ref bf) = blip_fill {
            src_rect = bf.children().find(|n| n.has_tag_name("srcRect"));
        }
    }

    let img_src = if data_uri.is_empty() {
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=".to_string()
    } else {
        data_uri
    };

    let id = node
        .descendants()
        .find(|n| n.has_tag_name("cNvPr"))
        .and_then(|n| n.attribute("id"))
        .unwrap_or("");
    let path_attr = format!(" data-path=\"/slide[{}]/picture[{}]\"", slide_num, id);
    output.push_str(&format!(
        "    <div class=\"picture\"{} style=\"{}\">",
        path_attr,
        styles.join(";")
    ));

    let mut has_crop = false;
    let mut src_l = 0.0;
    let mut src_t = 0.0;
    let mut src_r = 0.0;
    let mut src_b = 0.0;

    if let Some(ref rect) = src_rect {
        src_l = rect
            .attribute("l")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 100000.0;
        src_t = rect
            .attribute("t")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 100000.0;
        src_r = rect
            .attribute("r")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 100000.0;
        src_b = rect
            .attribute("b")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 100000.0;
        if src_l != 0.0 || src_t != 0.0 || src_r != 0.0 || src_b != 0.0 {
            has_crop = true;
        }
    }

    let degenerate_crop = has_crop && (src_l + src_r >= 1.0 || src_t + src_b >= 1.0);

    if degenerate_crop {
        // Nothing
    } else if has_crop {
        let visible_w = (1.0 - src_l - src_r).max(0.0001);
        let visible_h = (1.0 - src_t - src_b).max(0.0001);
        let bg_size_w = 100.0 / visible_w;
        let bg_size_h = 100.0 / visible_h;
        let denom_x = src_l + src_r;
        let denom_y = src_t + src_b;
        let bg_pos_x = if denom_x > 0.0 {
            (src_l / denom_x) * 100.0
        } else {
            0.0
        };
        let bg_pos_y = if denom_y > 0.0 {
            (src_t / denom_y) * 100.0
        } else {
            0.0
        };

        let bg_style = format!("width:100%;height:100%;background-image:url({});background-repeat:no-repeat;background-size:{:.2}% {:.2}%;background-position:{:.2}% {:.2}%", img_src, bg_size_w, bg_size_h, bg_pos_x, bg_pos_y);
        output.push_str(&format!("<div style=\"{}\"></div>", bg_style));
    } else {
        output.push_str(&format!("<img src=\"{}\" loading=\"lazy\"/>", img_src));
    }

    output.push_str("</div>\n");
}

fn render_picture_with_override_pos(
    node: &roxmltree::Node,
    slide_num: usize,
    slide_path: &str,
    rels: &oxml::rels::Relationships,
    output: &mut String,
    package: &OxmlPackage,
    theme_colors: &HashMap<String, String>,
    override_pos: (f64, f64, f64, f64),
) {
    render_picture(
        node,
        slide_num,
        slide_path,
        rels,
        output,
        package,
        theme_colors,
        Some(override_pos),
    );
}

// ==================== Text & Para Contextual Helpers ====================

fn is_title_placeholder(shape: Option<roxmltree::Node<'_, '_>>) -> bool {
    if let Some(s) = shape {
        if let Some(ph) = s.descendants().find(|n| n.has_tag_name("ph")) {
            if let Some(ty) = ph.attribute("type") {
                return ty == "title" || ty == "ctrTitle";
            }
        }
    }
    false
}

fn render_text_body(
    node: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
    output: &mut String,
) {
    let dummy_rels = oxml::rels::Relationships::empty();
    render_text_body_with_context(
        node,
        theme_colors,
        output,
        None,
        None,
        None,
        None,
        None,
        &dummy_rels,
    );
}

fn render_text_body_with_context(
    node: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
    output: &mut String,
    placeholder_shape: Option<roxmltree::Node<'_, '_>>,
    _placeholder_part: Option<&OxmlPackage>,
    layout_tree: Option<roxmltree::Node<'_, '_>>,
    master_tree: Option<roxmltree::Node<'_, '_>>,
    master_text_styles: Option<roxmltree::Node<'_, '_>>,
    slide_rels: &oxml::rels::Relationships,
) {
    let mut auto_num_counters = HashMap::new();
    let mut last_auto_key = String::new();

    let is_title = is_title_placeholder(placeholder_shape);
    let mut theme_font_fallback = None;
    if let Some(master) = master_tree {
        if let Some(font_scheme) = master.descendants().find(|n| n.has_tag_name("fontScheme")) {
            let font_node = if is_title {
                font_scheme.children().find(|n| n.has_tag_name("majorFont"))
            } else {
                font_scheme.children().find(|n| n.has_tag_name("minorFont"))
            };
            if let Some(fn_nd) = font_node {
                if let Some(latin) = fn_nd.children().find(|n| n.has_tag_name("latin")) {
                    if let Some(typeface) = latin.attribute("typeface") {
                        if !typeface.starts_with('+') {
                            theme_font_fallback = Some(typeface);
                        }
                    }
                }
            }
        }
    }

    for para in node.descendants().filter(|n| n.has_tag_name("p")) {
        let mut para_styles = Vec::new();
        let p_pr = para.children().find(|n| n.has_tag_name("pPr"));

        let level = p_pr
            .as_ref()
            .and_then(|n| n.attribute("lvl"))
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        let ph_type = placeholder_shape
            .and_then(|s| s.descendants().find(|n| n.has_tag_name("ph")))
            .and_then(|n| n.attribute("type"));
        let ph_idx = placeholder_shape
            .and_then(|s| s.descendants().find(|n| n.has_tag_name("ph")))
            .and_then(|n| n.attribute("idx"))
            .and_then(|s| s.parse::<usize>().ok());

        let default_font_size = resolve_placeholder_font_size(
            ph_type,
            ph_idx,
            level,
            layout_tree,
            master_tree,
            master_text_styles,
        );

        if let Some(ref pr) = p_pr {
            if let Some(algn) = pr.attribute("algn") {
                let align = match algn {
                    "ctr" => "center",
                    "r" => "right",
                    "just" => "justify",
                    _ => "left",
                };
                para_styles.push(format!("text-align:{}", align));
            }
            if let Some(sb) = pr.children().find(|n| n.has_tag_name("spcBfr")) {
                if let Some(pts) = sb
                    .children()
                    .find(|n| n.has_tag_name("spcPts"))
                    .and_then(|n| n.attribute("val"))
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    para_styles.push(format!("margin-top:{:.2}pt", pts / 100.0));
                }
            }
            if let Some(sa) = pr.children().find(|n| n.has_tag_name("spcAft")) {
                if let Some(pts) = sa
                    .children()
                    .find(|n| n.has_tag_name("spcPts"))
                    .and_then(|n| n.attribute("val"))
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    para_styles.push(format!("margin-bottom:{:.2}pt", pts / 100.0));
                }
            }
            if let Some(ln_spc) = pr.children().find(|n| n.has_tag_name("lnSpc")) {
                if let Some(pct) = ln_spc
                    .children()
                    .find(|n| n.has_tag_name("spcPct"))
                    .and_then(|n| n.attribute("val"))
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    para_styles.push(format!("line-height:{:.2}", pct / 100000.0));
                } else if let Some(pts) = ln_spc
                    .children()
                    .find(|n| n.has_tag_name("spcPts"))
                    .and_then(|n| n.attribute("val"))
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    para_styles.push(format!("line-height:{:.2}pt", pts / 100.0));
                }
            }
            if pr
                .attribute("rtl")
                .map(|s| s == "1" || s == "true")
                .unwrap_or(false)
            {
                para_styles.push("direction:rtl;unicode-bidi:embed".to_string());
            }
            if let Some(mar_l) = pr.attribute("marL").and_then(|s| s.parse::<f64>().ok()) {
                para_styles.push(format!("padding-left:{:.2}pt", mar_l / 12700.0));
            }
        }

        let has_bullet_char = p_pr
            .as_ref()
            .and_then(|pr| pr.children().find(|n| n.has_tag_name("buChar")))
            .is_some();
        let has_bullet_auto = p_pr
            .as_ref()
            .and_then(|pr| pr.children().find(|n| n.has_tag_name("buAuto")))
            .is_some();
        let has_bullet = has_bullet_char || has_bullet_auto;

        let mut auto_num_glyph = None;
        if has_bullet_auto {
            if let Some(ref pr) = p_pr {
                if let Some(bu_auto) = pr.children().find(|n| n.has_tag_name("buAuto")) {
                    let scheme_type = bu_auto.attribute("type").unwrap_or("arabicPeriod");
                    let scheme_key = format!("{}@{}", scheme_type, level);
                    if last_auto_key != scheme_key {
                        auto_num_counters.insert(scheme_key.clone(), 0);
                        last_auto_key = scheme_key.clone();
                    }
                    let start_at = bu_auto
                        .attribute("startAt")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(1);
                    let count = auto_num_counters.get(&scheme_key).copied().unwrap_or(0);
                    let index = if count == 0 {
                        start_at
                    } else {
                        start_at + count
                    };
                    auto_num_counters.insert(scheme_key.clone(), count + 1);
                    auto_num_glyph = Some(format_auto_number_glyph(scheme_type, index));
                }
            }
        } else {
            last_auto_key.clear();
        }

        let style_attr = if para_styles.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", para_styles.join(";"))
        };
        output.push_str(&format!("        <div class=\"para\"{}>\n", style_attr));

        if has_bullet {
            let bullet_char = p_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("buChar")))
                .and_then(|bu| bu.attribute("char"))
                .unwrap_or("•");
            let bullet = auto_num_glyph.unwrap_or_else(|| bullet_char.to_string());

            let mut bu_styles = Vec::new();
            let bu_color = p_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("buClr")))
                .and_then(|clr| clr.children().find(|n| n.has_tag_name("solidFill")))
                .and_then(|fill| resolve_fill_color(&fill, theme_colors));
            if let Some(color) = bu_color {
                bu_styles.push(format!("color:{}", color));
            } else {
                if let Some(first_run) = para.children().find(|n| n.has_tag_name("r")) {
                    if let Some(r_pr) = first_run.children().find(|n| n.has_tag_name("rPr")) {
                        if let Some(solid_fill) =
                            r_pr.children().find(|n| n.has_tag_name("solidFill"))
                        {
                            if let Some(color) = resolve_fill_color(&solid_fill, theme_colors) {
                                bu_styles.push(format!("color:{}", color));
                            }
                        }
                    }
                }
            }

            if let Some(ref pr) = p_pr {
                if let Some(bu_sz) = pr.children().find(|n| n.has_tag_name("buSzPts")) {
                    if let Some(val) = bu_sz.attribute("val").and_then(|s| s.parse::<f64>().ok()) {
                        bu_styles.push(format!("font-size:{:.2}pt", val / 100.0));
                    }
                } else if let Some(bu_sz) = pr.children().find(|n| n.has_tag_name("buSzPct")) {
                    if let Some(val) = bu_sz.attribute("val").and_then(|s| s.parse::<f64>().ok()) {
                        let pct = val / 100000.0;
                        let base_sz = default_font_size.unwrap_or(18.0);
                        bu_styles.push(format!("font-size:{:.2}pt", base_sz * pct));
                    }
                }
                if let Some(indent) = pr.attribute("indent").and_then(|s| s.parse::<f64>().ok()) {
                    if indent < 0.0 {
                        let gap_pt = -indent / 12700.0;
                        bu_styles.push("display:inline-block".to_string());
                        bu_styles.push(format!("width:{:.2}pt", gap_pt));
                        bu_styles.push(format!("margin-left:-{:.2}pt", gap_pt));
                    }
                }
            }

            let bu_style_attr = if bu_styles.is_empty() {
                String::new()
            } else {
                format!(" style=\"{}\"", bu_styles.join(";"))
            };
            output.push_str(&format!(
                "<span class=\"bullet\"{}>{}</span>",
                bu_style_attr,
                html_escape(&bullet)
            ));
        }

        let mut has_math = false;
        for child in para.children() {
            if child.tag_name().name() == "oMath" || child.tag_name().name() == "oMathPara" {
                has_math = true;
                let math_text = child
                    .descendants()
                    .filter(|n| n.has_tag_name("t"))
                    .filter_map(|n| n.text())
                    .collect::<Vec<&str>>()
                    .join("");
                if !math_text.is_empty() {
                    output.push_str(&format!(
                        "<span class=\"katex-formula\" data-formula=\"{}\"></span>",
                        html_escape(&math_text)
                    ));
                }
            }
        }

        let runs = para
            .children()
            .filter(|n| n.has_tag_name("r") || n.has_tag_name("br"));
        let mut has_any_run = false;

        for run in runs {
            has_any_run = true;
            if run.has_tag_name("br") {
                output.push_str("<br/>");
            } else {
                render_run(
                    &run,
                    theme_colors,
                    default_font_size,
                    theme_font_fallback,
                    slide_rels,
                    output,
                );
            }
        }

        if !has_any_run && !has_math {
            output.push_str("&nbsp;");
        }

        output.push_str("        </div>\n");
    }
}

fn render_run(
    run: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
    default_font_size: Option<f64>,
    theme_font_fallback: Option<&str>,
    slide_rels: &oxml::rels::Relationships,
    output: &mut String,
) {
    let mut text = String::new();
    if let Some(t) = run.children().find(|n| n.has_tag_name("t")) {
        text = t.text().unwrap_or("").to_string();
    }
    if text.is_empty() {
        return;
    }

    let mut run_styles = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut underline_style = None;
    let mut strike_style = None;
    let mut text_transform = None;
    let mut baseline_align = None;
    let mut hyperlink_url = None;

    if let Some(r_pr) = run.children().find(|n| n.has_tag_name("rPr")) {
        if r_pr
            .attribute("b")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false)
        {
            bold = true;
        }
        if r_pr
            .attribute("i")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false)
        {
            italic = true;
        }
        if let Some(u) = r_pr.attribute("u") {
            if u != "none" {
                underline_style = Some(u);
            }
        }
        if let Some(strike) = r_pr.attribute("strike") {
            if strike != "noStrike" {
                strike_style = Some(strike);
            }
        }
        if let Some(cap) = r_pr.attribute("cap") {
            if cap == "all" {
                text_transform = Some("text-transform:uppercase");
            } else if cap == "small" {
                text_transform = Some("font-variant-caps:all-small-caps");
            }
        }
        if let Some(baseline) = r_pr
            .attribute("baseline")
            .and_then(|s| s.parse::<i32>().ok())
        {
            if baseline > 0 {
                baseline_align = Some("vertical-align:super;font-size:smaller");
            } else if baseline < 0 {
                baseline_align = Some("vertical-align:sub;font-size:smaller");
            }
        }
        if let Some(spc) = r_pr.attribute("spc").and_then(|s| s.parse::<f64>().ok()) {
            run_styles.push(format!("letter-spacing:{:.2}pt", spc / 100.0));
        }

        if let Some(solid_fill) = r_pr.children().find(|n| n.has_tag_name("solidFill")) {
            if let Some(color) = resolve_fill_color(&solid_fill, theme_colors) {
                run_styles.push(format!("color:{}", color));
            }
        }

        if let Some(highlight) = r_pr.children().find(|n| n.has_tag_name("highlight")) {
            if let Some(color) = resolve_fill_color(&highlight, theme_colors) {
                run_styles.push(format!("background-color:{}", color));
            }
        }

        let latin_typeface = r_pr
            .children()
            .find(|n| n.has_tag_name("latin"))
            .and_then(|n| n.attribute("typeface"));
        let ea_typeface = r_pr
            .children()
            .find(|n| n.has_tag_name("ea"))
            .and_then(|n| n.attribute("typeface"));
        let typeface = latin_typeface.or(ea_typeface);

        let resolved_font = if let Some(font) = typeface {
            if !font.starts_with('+') {
                Some(font)
            } else {
                theme_font_fallback
            }
        } else {
            theme_font_fallback
        };

        if let Some(font) = resolved_font {
            run_styles.push(format!("font-family:'{}',sans-serif", font));
        }

        if let Some(sz) = r_pr.attribute("sz").and_then(|s| s.parse::<f64>().ok()) {
            run_styles.push(format!("font-size:{:.2}pt", sz / 100.0));
        } else if let Some(def_sz) = default_font_size {
            run_styles.push(format!("font-size:{:.2}pt", def_sz));
        }

        if let Some(hlink) = r_pr.children().find(|n| n.has_tag_name("hlinkClick")) {
            if let Some(r_id) = hlink
                .attribute((crate::dom_types::NS_R, "id"))
                .or_else(|| hlink.attribute("r:id"))
            {
                if let Some(rel) = slide_rels.get(r_id) {
                    hyperlink_url = Some(rel.target.clone());
                }
            }
        }
    } else if let Some(def_sz) = default_font_size {
        run_styles.push(format!("font-size:{:.2}pt", def_sz));
    }

    if bold {
        run_styles.push("font-weight:bold".to_string());
    }
    if italic {
        run_styles.push("font-style:italic".to_string());
    }
    if let Some(u) = underline_style {
        match u {
            "sng" | "words" => {
                run_styles.push("text-decoration:underline".to_string());
            }
            "dbl" => {
                run_styles.push("text-decoration:underline".to_string());
                run_styles.push("text-decoration-style:double".to_string());
            }
            "wavy" | "wavyLch" => {
                run_styles.push("text-decoration:underline wavy".to_string());
            }
            "wavyDbl" | "wavyHvy" => {
                run_styles.push("text-decoration:underline wavy".to_string());
                run_styles.push("text-decoration-thickness:2px".to_string());
            }
            "dot" | "sysDot" => {
                run_styles.push("text-decoration:underline dotted".to_string());
            }
            "dotHvy" => {
                run_styles.push("text-decoration:underline dotted".to_string());
                run_styles.push("text-decoration-thickness:2px".to_string());
            }
            "dash" | "dashLch" | "dashLong" => {
                run_styles.push("text-decoration:underline dashed".to_string());
            }
            "dashHvy" | "dashLchHvy" | "dashLongHvy" => {
                run_styles.push("text-decoration:underline dashed".to_string());
                run_styles.push("text-decoration-thickness:2px".to_string());
            }
            _ => {
                run_styles.push("text-decoration:underline".to_string());
            }
        }
    }
    if let Some(strike) = strike_style {
        if strike == "dblStrike" {
            run_styles.push("text-decoration:line-through double".to_string());
        } else {
            run_styles.push("text-decoration:line-through".to_string());
        }
    }
    if let Some(t) = text_transform {
        run_styles.push(t.to_string());
    }
    if let Some(b) = baseline_align {
        run_styles.push(b.to_string());
    }

    let inner = if run_styles.is_empty() {
        html_escape(&text)
    } else {
        format!(
            "<span style=\"{}\">{}</span>",
            run_styles.join(";"),
            html_escape(&text)
        )
    };

    if let Some(url) = hyperlink_url {
        output.push_str(&format!(
            "<a href=\"{}\" rel=\"noopener\" target=\"_blank\">{}</a>",
            html_escape(&url),
            inner
        ));
    } else {
        output.push_str(&inner);
    }
}

// ==================== Premium Table Renderer ====================

fn render_table(
    gf: &roxmltree::Node,
    slide_num: usize,
    theme_colors: &HashMap<String, String>,
    output: &mut String,
    x_pt: f64,
    y_pt: f64,
    cx_pt: f64,
    mut cy_pt: f64,
) {
    let tbl = match gf.descendants().find(|n| n.has_tag_name("tbl")) {
        Some(t) => t,
        None => return,
    };

    let mut row_height_sum_emu = 0.0;
    for tr in tbl.children().filter(|n| n.has_tag_name("tr")) {
        let h = tr
            .attribute("h")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        row_height_sum_emu += h;
    }
    let row_height_sum_pt = row_height_sum_emu / 12700.0;
    if row_height_sum_pt > cy_pt {
        cy_pt = row_height_sum_pt;
    }

    let grid_cols: Vec<roxmltree::Node> = tbl
        .children()
        .find(|n| n.has_tag_name("tblGrid"))
        .map(|n| n.children().filter(|c| c.has_tag_name("gridCol")).collect())
        .unwrap_or_default();

    let mut grid_width_sum_emu = 0.0;
    for gc in &grid_cols {
        let w = gc
            .attribute("w")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        grid_width_sum_emu += w;
    }
    let grid_width_sum_pt = grid_width_sum_emu / 12700.0;
    let table_width_pt = if grid_width_sum_pt > 0.0 {
        grid_width_sum_pt
    } else {
        cx_pt
    };

    let id = gf
        .descendants()
        .find(|n| n.has_tag_name("cNvPr"))
        .and_then(|n| n.attribute("id"))
        .unwrap_or("");
    let table_path = format!("/slide[{}]/table[{}]", slide_num, id);

    output.push_str(&format!(
        "    <div class=\"table-container\" style=\"left:{:.2}pt;top:{:.2}pt;width:{:.2}pt;height:{:.2}pt;\">\n",
        x_pt, y_pt, table_width_pt, cy_pt
    ));
    output.push_str(&format!(
        "      <table class=\"slide-table\" data-path=\"{}\">\n",
        table_path
    ));

    if !grid_cols.is_empty() {
        output.push_str("        <colgroup>");
        for gc in &grid_cols {
            let w = gc
                .attribute("w")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if w > 0.0 {
                output.push_str(&format!("<col style=\"width:{:.2}pt\">", w / 12700.0));
            } else {
                output.push_str(&format!(
                    "<col style=\"width:{:.2}%\">",
                    100.0 / grid_cols.len() as f64
                ));
            }
        }
        output.push_str("</colgroup>\n");
    }

    let tbl_pr = tbl.children().find(|n| n.has_tag_name("tblPr"));
    let has_first_row = tbl_pr
        .and_then(|n| n.attribute("firstRow"))
        .map(|s| s == "1" || s == "true")
        .unwrap_or(false);
    let has_band_row = tbl_pr
        .and_then(|n| n.attribute("bandRow"))
        .map(|s| s == "1" || s == "true")
        .unwrap_or(false);

    let mut row_index = 0;
    let mut rowspan_tracker: HashMap<(usize, usize), usize> = HashMap::new();
    let mut tr_idx = 0;

    for tr in tbl.children().filter(|n| n.has_tag_name("tr")) {
        tr_idx += 1;
        let row_path = format!("{}/tr[{}]", table_path, tr_idx);
        let row_h = tr
            .attribute("h")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let row_style = if row_h > 0.0 {
            format!(" style=\"height:{:.2}pt\"", row_h / 12700.0)
        } else {
            "".to_string()
        };
        output.push_str(&format!(
            "        <tr{} data-path=\"{}\">\n",
            row_style, row_path
        ));

        let mut col_index = 0;
        let mut skip_cols: usize = 0;
        let mut tc_idx = 0;

        for tc in tr.children().filter(|n| n.has_tag_name("tc")) {
            tc_idx += 1;
            let cell_path = format!("{}/tc[{}]", row_path, tc_idx);
            while rowspan_tracker
                .get(&(row_index, col_index))
                .copied()
                .unwrap_or(0)
                > 0
            {
                let remaining = rowspan_tracker.remove(&(row_index, col_index)).unwrap();
                if remaining > 1 {
                    rowspan_tracker.insert((row_index + 1, col_index), remaining - 1);
                }
                col_index += 1;
            }

            if tc
                .attribute("hMerge")
                .map(|s| s == "1" || s == "true")
                .unwrap_or(false)
            {
                skip_cols = skip_cols.saturating_sub(1);
                col_index += 1;
                continue;
            }
            if tc
                .attribute("vMerge")
                .map(|s| s == "1" || s == "true")
                .unwrap_or(false)
            {
                col_index += 1;
                continue;
            }
            if skip_cols > 0 {
                skip_cols -= 1;
                col_index += 1;
                continue;
            }

            let grid_span = tc
                .attribute("gridSpan")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            let row_span = tc
                .attribute("rowSpan")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);

            if grid_span > 1 {
                skip_cols = grid_span - 1;
            }
            if row_span > 1 {
                rowspan_tracker.insert((row_index + 1, col_index), row_span - 1);
            }

            let mut cell_styles = Vec::new();
            let tc_pr = tc.children().find(|n| n.has_tag_name("tcPr"));

            let cell_fill = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("solidFill")))
                .and_then(|fill| resolve_fill_color(&fill, theme_colors));

            let cell_grad = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("gradFill")))
                .map(|grad| gradient_to_css(&grad, theme_colors));

            if let Some(color) = cell_fill {
                cell_styles.push(format!("background:{}", color));
            } else if let Some(grad_css) = cell_grad {
                cell_styles.push(format!("background:{}", grad_css));
            } else {
                let is_header = has_first_row && row_index == 0;
                let is_banded_odd = has_band_row
                    && (if has_first_row {
                        row_index > 0 && (row_index - 1) % 2 == 0
                    } else {
                        row_index % 2 == 0
                    });

                if is_header {
                    let h_color = theme_colors
                        .get("accent1")
                        .or_else(|| theme_colors.get("dk2"))
                        .map(|s| format!("#{}", s))
                        .unwrap_or_else(|| "#4f81bd".to_string());
                    cell_styles.push(format!("background:{}", h_color));
                    cell_styles.push("color:#ffffff".to_string());
                } else if is_banded_odd {
                    let b_color = theme_colors
                        .get("lt2")
                        .map(|s| format!("#{}", s))
                        .unwrap_or_else(|| "rgba(128,128,128,0.1)".to_string());
                    cell_styles.push(format!("background:{}", b_color));
                }
            }

            if let Some(ref pr) = tc_pr {
                if let Some(anchor) = pr.attribute("anchor") {
                    let va = match anchor {
                        "ctr" => "middle",
                        "b" => "bottom",
                        _ => "top",
                    };
                    cell_styles.push(format!("vertical-align:{}", va));
                }
            }

            let bl = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("lnL")))
                .map(|ln| outline_to_css(&ln, theme_colors))
                .unwrap_or_else(|| "1px solid #ccc".to_string());
            let br = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("lnR")))
                .map(|ln| outline_to_css(&ln, theme_colors))
                .unwrap_or_else(|| "1px solid #ccc".to_string());
            let bt = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("lnT")))
                .map(|ln| outline_to_css(&ln, theme_colors))
                .unwrap_or_else(|| "1px solid #ccc".to_string());
            let bb = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("lnB")))
                .map(|ln| outline_to_css(&ln, theme_colors))
                .unwrap_or_else(|| "1px solid #ccc".to_string());

            cell_styles.push(format!("border-left:{}", bl));
            cell_styles.push(format!("border-right:{}", br));
            cell_styles.push(format!("border-top:{}", bt));
            cell_styles.push(format!("border-bottom:{}", bb));

            let ln_tl_br = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("lnTlToBr")));
            let ln_bl_tr = tc_pr
                .as_ref()
                .and_then(|pr| pr.children().find(|n| n.has_tag_name("lnBlToTr")));
            let tl_br_css = ln_tl_br.as_ref().map(|ln| outline_to_css(ln, theme_colors));
            let bl_tr_css = ln_bl_tr.as_ref().map(|ln| outline_to_css(ln, theme_colors));

            let has_diag = tl_br_css.is_some() || bl_tr_css.is_some();
            if has_diag {
                cell_styles.push("position:relative".to_string());
            }

            if let Some(ref pr) = tc_pr {
                let mar_l = pr
                    .attribute("marL")
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(91440.0)
                    / 12700.0;
                let mar_r = pr
                    .attribute("marR")
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(91440.0)
                    / 12700.0;
                let mar_t = pr
                    .attribute("marT")
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(45720.0)
                    / 12700.0;
                let mar_b = pr
                    .attribute("marB")
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(45720.0)
                    / 12700.0;
                cell_styles.push(format!(
                    "padding:{:.2}pt {:.2}pt {:.2}pt {:.2}pt",
                    mar_t, mar_r, mar_b, mar_l
                ));
            }

            let span_attrs = format!(
                "{}{}",
                if grid_span > 1 {
                    format!(" colspan=\"{}\"", grid_span)
                } else {
                    "".to_string()
                },
                if row_span > 1 {
                    format!(" rowspan=\"{}\"", row_span)
                } else {
                    "".to_string()
                }
            );

            let style_attr = if cell_styles.is_empty() {
                "".to_string()
            } else {
                format!(" style=\"{}\"", cell_styles.join(";"))
            };

            let mut diag_overlay = String::new();
            if has_diag {
                let mut svg_lines = String::new();
                if let Some(ref border_css) = tl_br_css {
                    let stroke = border_css.split(' ').next_back().unwrap_or("#000");
                    let width = border_css.split(' ').next().unwrap_or("1.0pt");
                    svg_lines.push_str(&format!("<line x1=\"0\" y1=\"0\" x2=\"100%\" y2=\"100%\" stroke=\"{}\" stroke-width=\"{}\"/>", stroke, width));
                }
                if let Some(ref border_css) = bl_tr_css {
                    let stroke = border_css.split(' ').next_back().unwrap_or("#000");
                    let width = border_css.split(' ').next().unwrap_or("1.0pt");
                    svg_lines.push_str(&format!("<line x1=\"0\" y1=\"100%\" x2=\"100%\" y2=\"0\" stroke=\"{}\" stroke-width=\"{}\"/>", stroke, width));
                }
                diag_overlay = format!("<svg class=\"cell-diag\" width=\"100%\" height=\"100%\" style=\"position:absolute;inset:0;pointer-events:none;overflow:visible\" preserveAspectRatio=\"none\">{}</svg>", svg_lines);
            }

            output.push_str(&format!(
                "          <td data-path=\"{}\"{}{}>{}\n",
                cell_path, span_attrs, style_attr, diag_overlay
            ));

            let tx_body = tc.children().find(|n| n.has_tag_name("txBody"));
            if let Some(tx) = tx_body {
                let mut cell_text_output = String::new();
                render_text_body(&tx, theme_colors, &mut cell_text_output);
                output.push_str(&cell_text_output);
            }
            output.push_str("          </td>\n");

            col_index += grid_span;
        }
        output.push_str("        </tr>\n");
        row_index += 1;
    }

    output.push_str("      </table>\n");
    output.push_str("    </div>\n");
}

fn render_connector(
    node: &roxmltree::Node,
    theme_colors: &HashMap<String, String>,
    output: &mut String,
    override_pos: Option<(f64, f64, f64, f64)>,
) {
    let xfrm = node.descendants().find(|n| n.has_tag_name("xfrm"));
    let mut x_pt = 0.0;
    let mut y_pt = 0.0;
    let mut cx_pt = 0.0;
    let mut cy_pt = 0.0;
    let mut rot_deg = 0.0;
    let mut flip_h = false;
    let mut flip_v = false;

    if let Some(pos) = override_pos {
        x_pt = pos.0 / 12700.0;
        y_pt = pos.1 / 12700.0;
        cx_pt = pos.2 / 12700.0;
        cy_pt = pos.3 / 12700.0;
    } else if let Some(x_node) = xfrm {
        if let Some(off) = x_node.descendants().find(|n| n.has_tag_name("off")) {
            let x = off
                .attribute("x")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let y = off
                .attribute("y")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            x_pt = x / 12700.0;
            y_pt = y / 12700.0;
        }
        if let Some(ext) = x_node.descendants().find(|n| n.has_tag_name("ext")) {
            let cx = ext
                .attribute("cx")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let cy = ext
                .attribute("cy")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            cx_pt = cx / 12700.0;
            cy_pt = cy / 12700.0;
        }
        if let Some(rot) = x_node.attribute("rot").and_then(|s| s.parse::<f64>().ok()) {
            rot_deg = rot / 60000.0;
        }
        flip_h = x_node
            .attribute("flipH")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false);
        flip_v = x_node
            .attribute("flipV")
            .map(|s| s == "1" || s == "true")
            .unwrap_or(false);
    } else {
        return;
    }

    let sp_pr = node.descendants().find(|n| n.has_tag_name("spPr"));
    let outline = sp_pr
        .as_ref()
        .and_then(|pr| pr.children().find(|n| n.has_tag_name("ln")));
    let default_color = theme_colors
        .get("dk1")
        .map(|s| format!("#{}", s))
        .unwrap_or_else(|| "#000000".to_string());

    let mut line_color = default_color;
    let mut line_width = 1.0;
    let mut prst_dash = "solid".to_string();

    if let Some(ref ln) = outline {
        if let Some(color) = ln
            .children()
            .find(|n| n.has_tag_name("solidFill"))
            .and_then(|f| resolve_fill_color(&f, theme_colors))
        {
            line_color = color;
        }
        if let Some(w) = ln.attribute("w").and_then(|s| s.parse::<f64>().ok()) {
            line_width = w / 12700.0;
        }
        if let Some(dash) = ln
            .children()
            .find(|n| n.has_tag_name("prstDash"))
            .and_then(|d| d.attribute("val"))
        {
            prst_dash = dash.to_string();
        }
    }

    let padding_pt = line_width + 1.0;
    let render_cx = if cx_pt == 0.0 { padding_pt } else { cx_pt };
    let render_cy = if cy_pt == 0.0 { padding_pt } else { cy_pt };
    let render_x = if cx_pt == 0.0 {
        x_pt - padding_pt / 2.0
    } else {
        x_pt
    };
    let render_y = if cy_pt == 0.0 {
        y_pt - padding_pt / 2.0
    } else {
        y_pt
    };

    let x1 = if flip_h { "100%" } else { "0" };
    let y1 = if flip_v { "100%" } else { "0" };
    let x2 = if flip_h { "0" } else { "100%" };
    let y2 = if flip_v { "0" } else { "100%" };

    let (svg_x1, svg_y1, svg_x2, svg_y2) = if cy_pt == 0.0 {
        (
            if flip_h { "100%" } else { "0" },
            "50%",
            if flip_h { "0" } else { "100%" },
            "50%",
        )
    } else if cx_pt == 0.0 {
        (
            "50%",
            if flip_v { "100%" } else { "0" },
            "50%",
            if flip_v { "0" } else { "100%" },
        )
    } else {
        (x1, y1, x2, y2)
    };

    let dash_arr = dash_type_to_svg_dasharray(&prst_dash, line_width);
    let dash_attr = if !dash_arr.is_empty() {
        format!(" stroke-dasharray=\"{}\"", dash_arr)
    } else {
        "".to_string()
    };

    let head_end = outline
        .as_ref()
        .and_then(|ln| ln.children().find(|n| n.has_tag_name("headEnd")));
    let tail_end = outline
        .as_ref()
        .and_then(|ln| ln.children().find(|n| n.has_tag_name("tailEnd")));
    let has_head = head_end
        .and_then(|he| he.attribute("type"))
        .map(|t| t != "none")
        .unwrap_or(false);
    let has_tail = tail_end
        .and_then(|te| te.attribute("type"))
        .map(|t| t != "none")
        .unwrap_or(false);

    let mut marker_defs = String::new();
    let mut marker_start = String::new();
    let mut marker_end = String::new();

    if has_head || has_tail {
        let arrow_size = (line_width * 3.0).max(3.0);
        let safe_color = line_color.clone();
        marker_defs.push_str("<defs>");
        if has_head {
            marker_defs.push_str(&format!("<marker id=\"ah\" markerWidth=\"{:.1}\" markerHeight=\"{:.1}\" refX=\"{:.1}\" refY=\"{:.1}\" orient=\"auto-start-reverse\"><polygon points=\"0 0,{:.1} {:.1},0 {:.1}\" fill=\"{}\"/></marker>", arrow_size, arrow_size, arrow_size, arrow_size / 2.0, arrow_size, arrow_size / 2.0, arrow_size, safe_color));
            marker_start = " marker-start=\"url(#ah)\"".to_string();
        }
        if has_tail {
            marker_defs.push_str(&format!("<marker id=\"at\" markerWidth=\"{:.1}\" markerHeight=\"{:.1}\" refX=\"{:.1}\" refY=\"{:.1}\" orient=\"auto\"><polygon points=\"0 0,{:.1} {:.1},0 {:.1}\" fill=\"{}\"/></marker>", arrow_size, arrow_size, arrow_size, arrow_size / 2.0, arrow_size, arrow_size / 2.0, arrow_size, safe_color));
            marker_end = " marker-end=\"url(#at)\"".to_string();
        }
        marker_defs.push_str("</defs>");
    }

    let preset = sp_pr
        .as_ref()
        .and_then(|pr| pr.children().find(|n| n.has_tag_name("prstGeom")))
        .and_then(|g| g.attribute("prst"))
        .unwrap_or("straightConnector1");

    let stroke_attrs = format!(
        "stroke=\"{}\" stroke-width=\"{:.2}pt\" fill=\"none\"{}{}{}",
        line_color, line_width, dash_attr, marker_start, marker_end
    );
    let cxn_transform = if rot_deg != 0.0 {
        format!(";transform:rotate({:.2}deg)", rot_deg)
    } else {
        "".to_string()
    };

    output.push_str(&format!("    <div class=\"connector\" style=\"left:{:.2}pt;top:{:.2}pt;width:{:.2}pt;height:{:.2}pt{}\">\n", render_x, render_y, render_cx, render_cy, cxn_transform));

    if preset.starts_with("bentConnector") {
        let points = match preset {
            "bentConnector2" => "0,0 100,0 100,100",
            "bentConnector4" | "bentConnector5" => "0,0 25,0 25,50 75,50 75,100 100,100",
            _ => "0,0 50,0 50,100 100,100",
        };
        output.push_str("      <svg width=\"100%\" height=\"100%\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\" style=\"overflow:visible;display:block\">\n");
        if !marker_defs.is_empty() {
            output.push_str(&format!("        {}\n", marker_defs));
        }
        output.push_str(&format!(
            "        <polyline points=\"{}\" {}/>\n",
            points, stroke_attrs
        ));
        output.push_str("      </svg>\n");
    } else if preset.starts_with("curvedConnector") {
        let d = match preset {
            "curvedConnector2" => "M 0,0 Q 100,0 100,100",
            "curvedConnector4" | "curvedConnector5" => {
                "M 0,0 C 25,0 25,50 50,50 C 75,50 75,100 100,100"
            }
            _ => "M 0,0 C 50,0 50,100 100,100",
        };
        output.push_str("      <svg width=\"100%\" height=\"100%\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\" style=\"overflow:visible;display:block\">\n");
        if !marker_defs.is_empty() {
            output.push_str(&format!("        {}\n", marker_defs));
        }
        output.push_str(&format!("        <path d=\"{}\" {}/>\n", d, stroke_attrs));
        output.push_str("      </svg>\n");
    } else {
        output.push_str("      <svg width=\"100%\" height=\"100%\" preserveAspectRatio=\"none\" style=\"overflow:visible;display:block\">\n");
        if !marker_defs.is_empty() {
            output.push_str(&format!("        {}\n", marker_defs));
        }
        output.push_str(&format!(
            "        <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" {}/>\n",
            svg_x1, svg_y1, svg_x2, svg_y2, stroke_attrs
        ));
        output.push_str("      </svg>\n");
    }

    output.push_str("    </div>\n");
}

#[allow(clippy::too_many_arguments)]
fn render_group_shape(
    node: &roxmltree::Node,
    slide_num: usize,
    slide_path: &str,
    rels: &oxml::rels::Relationships,
    theme_colors: &HashMap<String, String>,
    output: &mut String,
    package: &OxmlPackage,
    layout_tree: Option<roxmltree::Node<'_, '_>>,
    master_tree: Option<roxmltree::Node<'_, '_>>,
    master_text_styles: Option<roxmltree::Node<'_, '_>>,
) {
    let grp_sp_pr = node.children().find(|n| n.has_tag_name("grpSpPr"));
    let xfrm = grp_sp_pr
        .as_ref()
        .and_then(|pr| pr.children().find(|n| n.has_tag_name("xfrm")));
    if xfrm.is_none() {
        return;
    }
    let xfrm = xfrm.unwrap();

    let off = match xfrm.children().find(|n| n.has_tag_name("off")) {
        Some(o) => o,
        None => return,
    };
    let ext = match xfrm.children().find(|n| n.has_tag_name("ext")) {
        Some(e) => e,
        None => return,
    };

    let x = off
        .attribute("x")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let y = off
        .attribute("y")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let cx = ext
        .attribute("cx")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let cy = ext
        .attribute("cy")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    let child_off = xfrm.children().find(|n| n.has_tag_name("chOff"));
    let child_ext = xfrm.children().find(|n| n.has_tag_name("chExt"));

    let ch_x = child_off
        .and_then(|n| n.attribute("x"))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(x);
    let ch_y = child_off
        .and_then(|n| n.attribute("y"))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(y);
    let ch_cx = child_ext
        .and_then(|n| n.attribute("cx"))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(cx);
    let ch_cy = child_ext
        .and_then(|n| n.attribute("cy"))
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(cy);

    let scale_x = if ch_cx != 0.0 { cx / ch_cx } else { 1.0 };
    let scale_y = if ch_cy != 0.0 { cy / ch_cy } else { 1.0 };

    let rot_deg = xfrm
        .attribute("rot")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
        / 60000.0;
    let grp_transform = if rot_deg != 0.0 {
        format!(";transform:rotate({:.2}deg)", rot_deg)
    } else {
        "".to_string()
    };

    output.push_str(&format!(
        "    <div class=\"group\" style=\"left:{:.2}pt;top:{:.2}pt;width:{:.2}pt;height:{:.2}pt{}\">\n",
        x / 12700.0, y / 12700.0, cx / 12700.0, cy / 12700.0, grp_transform
    ));

    let calc_group_child_pos = |ch_node: &roxmltree::Node| -> Option<(f64, f64, f64, f64)> {
        let cx_xfrm = ch_node.descendants().find(|n| n.has_tag_name("xfrm"))?;
        let c_off = cx_xfrm.children().find(|n| n.has_tag_name("off"))?;
        let c_ext = cx_xfrm.children().find(|n| n.has_tag_name("ext"))?;

        let ox = c_off
            .attribute("x")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let oy = c_off
            .attribute("y")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let ocx = c_ext
            .attribute("cx")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let ocy = c_ext
            .attribute("cy")
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        Some((
            (ox - ch_x) * scale_x,
            (oy - ch_y) * scale_y,
            ocx * scale_x,
            ocy * scale_y,
        ))
    };

    for child in node.children() {
        let tag = child.tag_name().name();
        if tag == "sp" {
            if let Some(pos) = calc_group_child_pos(&child) {
                let mut shape_html = String::new();
                render_shape(
                    &child,
                    slide_num,
                    slide_path,
                    rels,
                    theme_colors,
                    &mut shape_html,
                    package,
                    layout_tree,
                    master_tree,
                    master_text_styles,
                    Some(pos),
                );
                output.push_str(&shape_html);
            }
        } else if tag == "pic" {
            if let Some(pos) = calc_group_child_pos(&child) {
                let mut pic_html = String::new();
                render_picture_with_override_pos(
                    &child,
                    slide_num,
                    slide_path,
                    rels,
                    &mut pic_html,
                    package,
                    theme_colors,
                    pos,
                );
                output.push_str(&pic_html);
            }
        } else if tag == "cxnSp" {
            if let Some(pos) = calc_group_child_pos(&child) {
                let mut cxn_html = String::new();
                render_connector(&child, theme_colors, &mut cxn_html, Some(pos));
                output.push_str(&cxn_html);
            }
        } else if tag == "grpSp" {
            render_group_shape(
                &child,
                slide_num,
                slide_path,
                rels,
                theme_colors,
                output,
                package,
                layout_tree,
                master_tree,
                master_text_styles,
            );
        }
    }

    output.push_str("    </div>\n");
}

// ==================== Utilities ====================

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
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
