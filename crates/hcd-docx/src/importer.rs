use hcd_core::{
    hash_bytes, hash_file, stable_node_id, BundleWriter, ChunkSourceMap, FidelityLevel,
    FidelityReport, FidelityWarning, HcdCapabilities, HcdError, HcdManifest, ImportEvent,
    NodeMapEntry, SourceAnchor, SourceDescriptor, DEFAULT_CHUNK_BLOCKS, DEFAULT_CHUNK_SOFT_BYTES,
    HCD_SCHEMA_VERSION, MAX_CHUNK_BYTES, MAX_CONTROL_PART_BYTES,
};
use oxml::{PackageError, StreamingOxmlArchive};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_XML_ELEMENTS: usize = 3_000_000;
const MAX_XML_DEPTH: usize = 256;
const TABLE_ROWS_PER_FRAGMENT: usize = 128;
const MAX_TABLE_GRID_COLUMNS: usize = 16_384;
const MAX_TABLE_BAND_SIZE: u32 = 1_024;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub document_id: String,
    pub chunk_soft_bytes: usize,
    pub chunk_blocks: usize,
}

impl ImportOptions {
    pub fn new(document_id: impl Into<String>) -> Self {
        Self {
            document_id: document_id.into(),
            chunk_soft_bytes: DEFAULT_CHUNK_SOFT_BYTES,
            chunk_blocks: DEFAULT_CHUNK_BLOCKS,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetRecord {
    source_part: String,
    hash: String,
    href: String,
    byte_length: u64,
}

#[derive(Default)]
struct RenderedBlock {
    html: String,
    entries: Vec<NodeMapEntry>,
    visual_placement: Option<VisualPlacement>,
    requires_overflow_visible: bool,
}

#[derive(Clone, Copy, Default)]
struct VisualPlacement {
    left_px: f64,
    top_px: f64,
    width_px: f64,
    height_px: f64,
}

#[derive(Default)]
struct ParagraphBuilder {
    paragraph_id: Option<String>,
    format: ParagraphFormat,
    html: String,
    entries: Vec<NodeMapEntry>,
    nested: Vec<RenderedBlock>,
    ordinal: u64,
    run_ordinal: usize,
    has_visible_text: bool,
}

#[derive(Default, Clone)]
struct ParagraphFormat {
    style_id: Option<String>,
    alignment: Option<String>,
    bidi: bool,
    keep_next: bool,
    keep_lines: bool,
    page_break_before: bool,
    left_twips: Option<i64>,
    right_twips: Option<i64>,
    first_line_twips: Option<i64>,
    hanging_twips: Option<i64>,
    before_twips: Option<i64>,
    after_twips: Option<i64>,
    line_twips: Option<i64>,
    line_rule: Option<String>,
    numbering_id: Option<String>,
    numbering_level: Option<String>,
    conditional_style: ConditionalStyleMask,
}

#[derive(Default)]
struct RunBuilder {
    text_id: Option<String>,
    ordinal: usize,
    format: RunFormat,
    opened: bool,
}

#[derive(Default, Clone)]
struct RunFormat {
    style_id: Option<String>,
    bold: bool,
    italic: bool,
    strike: bool,
    underline: Option<String>,
    color: Option<String>,
    highlight: Option<String>,
    font: Option<String>,
    latin_font: Option<String>,
    east_asia_font: Option<String>,
    bidi_font: Option<String>,
    resolved_latin_font: Option<String>,
    resolved_east_asia_font: Option<String>,
    resolved_bidi_font: Option<String>,
    latin_theme: Option<String>,
    east_asia_theme: Option<String>,
    bidi_theme: Option<String>,
    latin_language: Option<String>,
    east_asia_language: Option<String>,
    bidi_language: Option<String>,
    size_half_points: Option<u32>,
    vertical_align: Option<String>,
    rtl: bool,
    hidden: bool,
}

#[derive(Default)]
struct WordStyleDefinition {
    kind: Option<String>,
    is_default: bool,
    based_on: Option<String>,
    linked: Option<String>,
    paragraph: ParagraphFormat,
    run: RunFormat,
    table: TableStyleLayer,
    table_conditions: BTreeMap<String, TableStyleLayer>,
    table_row_band_size: Option<u32>,
    table_column_band_size: Option<u32>,
}

#[derive(Default, Clone)]
struct TableStyleLayer {
    paragraph: ParagraphFormat,
    run: RunFormat,
    table_css: Vec<String>,
    row_css: Vec<String>,
    cell_css: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
struct TableBandSizes {
    row: u32,
    column: u32,
}

impl Default for TableBandSizes {
    fn default() -> Self {
        Self { row: 1, column: 1 }
    }
}

type TableBandCatalog = BTreeMap<String, TableBandSizes>;

#[derive(Clone, Copy, Default)]
struct ConditionalStyleMask {
    present: bool,
    specified: u16,
    enabled: u16,
}

struct ConditionalStyleFlag {
    ooxml_attribute: &'static str,
    condition: &'static str,
    data_attribute: &'static str,
}

const CONDITIONAL_STYLE_FLAGS: [ConditionalStyleFlag; 12] = [
    ConditionalStyleFlag {
        ooxml_attribute: "firstRow",
        condition: "firstRow",
        data_attribute: "data-hcd-cnf-first-row",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "lastRow",
        condition: "lastRow",
        data_attribute: "data-hcd-cnf-last-row",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "firstColumn",
        condition: "firstCol",
        data_attribute: "data-hcd-cnf-first-column",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "lastColumn",
        condition: "lastCol",
        data_attribute: "data-hcd-cnf-last-column",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "oddVBand",
        condition: "band1Vert",
        data_attribute: "data-hcd-cnf-band1-vertical",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "evenVBand",
        condition: "band2Vert",
        data_attribute: "data-hcd-cnf-band2-vertical",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "oddHBand",
        condition: "band1Horz",
        data_attribute: "data-hcd-cnf-band1-horizontal",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "evenHBand",
        condition: "band2Horz",
        data_attribute: "data-hcd-cnf-band2-horizontal",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "firstRowLastColumn",
        condition: "neCell",
        data_attribute: "data-hcd-cnf-ne-cell",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "firstRowFirstColumn",
        condition: "nwCell",
        data_attribute: "data-hcd-cnf-nw-cell",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "lastRowLastColumn",
        condition: "seCell",
        data_attribute: "data-hcd-cnf-se-cell",
    },
    ConditionalStyleFlag {
        ooxml_attribute: "lastRowFirstColumn",
        condition: "swCell",
        data_attribute: "data-hcd-cnf-sw-cell",
    },
];

#[derive(Debug)]
struct RenderedWordStyles {
    css: String,
    table_bands: TableBandCatalog,
    paragraph_numbering: BTreeMap<String, (String, Option<String>)>,
}

struct StylePropertyScope<'a> {
    in_paragraph_properties: bool,
    in_run_properties: bool,
    in_defaults: bool,
    theme: &'a WordTheme,
}

#[derive(Debug, Default)]
struct WordTheme {
    major_latin: Option<String>,
    major_east_asia: Option<String>,
    major_bidi: Option<String>,
    minor_latin: Option<String>,
    minor_east_asia: Option<String>,
    minor_bidi: Option<String>,
    major_supplemental: BTreeMap<String, String>,
    minor_supplemental: BTreeMap<String, String>,
    colors: BTreeMap<String, String>,
}

#[derive(Clone, Copy)]
enum ThemeFontSet {
    Major,
    Minor,
}

impl WordTheme {
    fn font(&self, slot: &str, language: Option<&str>) -> Option<&str> {
        let supplemental = language
            .and_then(language_theme_script)
            .and_then(|script| match slot {
                "majorEastAsia" | "majorBidi" => self.major_supplemental.get(script),
                "minorEastAsia" | "minorBidi" => self.minor_supplemental.get(script),
                _ => None,
            })
            .map(String::as_str);
        match slot {
            "majorAscii" | "majorHAnsi" => self.major_latin.as_deref(),
            "majorEastAsia" => supplemental
                .or(self.major_east_asia.as_deref())
                .or(self.major_latin.as_deref()),
            "majorBidi" => supplemental
                .or(self.major_bidi.as_deref())
                .or(self.major_latin.as_deref()),
            "minorAscii" | "minorHAnsi" => self.minor_latin.as_deref(),
            "minorEastAsia" => supplemental
                .or(self.minor_east_asia.as_deref())
                .or(self.minor_latin.as_deref()),
            "minorBidi" => supplemental
                .or(self.minor_bidi.as_deref())
                .or(self.minor_latin.as_deref()),
            _ => None,
        }
    }

    fn color(&self, slot: &str) -> Option<&str> {
        let canonical = match slot {
            "dark1" | "text1" => "dk1",
            "light1" | "background1" => "lt1",
            "dark2" | "text2" => "dk2",
            "light2" | "background2" => "lt2",
            "hyperlink" => "hlink",
            "followedHyperlink" => "folHlink",
            other => other,
        };
        self.colors.get(canonical).map(String::as_str)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextNodeKind {
    Editable,
    Deleted,
    FieldInstruction,
}

struct PendingText {
    value: String,
    text_id: Option<String>,
    kind: TextNodeKind,
}

#[derive(Default)]
struct FieldFrame {
    instruction: String,
    wrapper_is_anchor: Option<bool>,
    simple: bool,
}

struct RevisionFrame {
    kind: RevisionKind,
    opened: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RevisionKind {
    Insert,
    Delete,
    MoveFrom,
    MoveTo,
}

impl RevisionKind {
    fn from_ooxml(name: &str) -> Option<Self> {
        match name {
            "ins" => Some(Self::Insert),
            "del" => Some(Self::Delete),
            "moveFrom" => Some(Self::MoveFrom),
            "moveTo" => Some(Self::MoveTo),
            _ => None,
        }
    }

    fn html_value(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Delete => "delete",
            Self::MoveFrom => "move-from",
            Self::MoveTo => "move-to",
        }
    }

    fn css_class(self) -> &'static str {
        match self {
            Self::Insert | Self::MoveTo => "hcd-revision-insert",
            Self::Delete | Self::MoveFrom => "hcd-revision-delete",
        }
    }
}

#[derive(Clone)]
struct NumberLevel {
    start: i64,
    number_format: String,
    level_text: String,
    suffix: String,
    font: Option<String>,
    left_twips: Option<i64>,
    hanging_twips: Option<i64>,
    alignment: Option<String>,
    size_half_points: Option<u32>,
    color: Option<String>,
    bold: bool,
    italic: bool,
}

impl Default for NumberLevel {
    fn default() -> Self {
        Self {
            start: 1,
            number_format: "decimal".to_string(),
            level_text: "%1.".to_string(),
            suffix: "tab".to_string(),
            font: None,
            left_twips: None,
            hanging_twips: None,
            alignment: None,
            size_half_points: None,
            color: None,
            bold: false,
            italic: false,
        }
    }
}

#[derive(Default)]
struct AbstractNumbering {
    levels: BTreeMap<u8, NumberLevel>,
}

#[derive(Default)]
struct NumberingInstance {
    abstract_id: String,
    start_overrides: BTreeMap<u8, i64>,
    level_overrides: BTreeMap<u8, NumberLevel>,
}

#[derive(Default)]
struct NumberingCatalog {
    abstracts: HashMap<String, AbstractNumbering>,
    instances: HashMap<String, NumberingInstance>,
}

enum NumberLevelTarget {
    Abstract(String),
    Instance(String),
}

struct PendingNumberLevel {
    index: u8,
    definition: NumberLevel,
    target: NumberLevelTarget,
}

#[derive(Default)]
struct NumberCounters {
    values: [i64; 9],
    initialized: [bool; 9],
}

#[derive(Default)]
struct NumberingState {
    instances: HashMap<String, NumberCounters>,
    abstracts: HashMap<String, NumberCounters>,
}

struct RenderedNumberMarker {
    html: String,
    definition: NumberLevel,
}

fn load_word_theme(archive: &mut StreamingOxmlArchive) -> Result<WordTheme, HcdError> {
    let Some(part) = word_theme_part(archive)? else {
        return Ok(WordTheme::default());
    };
    let xml = archive
        .read_control_part(&part, MAX_CONTROL_PART_BYTES)
        .map_err(package_error)?;
    parse_word_theme(&xml, &part)
}

fn word_theme_part(archive: &mut StreamingOxmlArchive) -> Result<Option<String>, HcdError> {
    const DOCUMENT_PART: &str = "word/document.xml";
    const RELS_PART: &str = "word/_rels/document.xml.rels";
    if archive.contains(RELS_PART) {
        let xml = archive
            .read_control_part(RELS_PART, MAX_CONTROL_PART_BYTES)
            .map_err(package_error)?;
        let mut reader = Reader::from_reader(xml.as_slice());
        reader.config_mut().check_end_names = true;
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Empty(ref element)) | Ok(Event::Start(ref element))
                    if local_name(element.name().as_ref()) == "Relationship" =>
                {
                    let external = attribute_by_local_name(element, "TargetMode")
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("External"));
                    let is_theme = attribute_by_local_name(element, "Type")
                        .is_some_and(|kind| kind.rsplit('/').next() == Some("theme"));
                    if is_theme && !external {
                        if let Some(target) = attribute_by_local_name(element, "Target") {
                            let resolved = resolve_relationship_target(DOCUMENT_PART, &target)?;
                            if archive.contains(&resolved) {
                                return Ok(Some(resolved));
                            }
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(error) => {
                    return Err(HcdError::InvalidBundle(format!(
                        "invalid relationships {RELS_PART}: {error}"
                    )))
                }
            }
            buffer.clear();
        }
    }

    let mut candidates: Vec<_> = archive
        .entries()
        .iter()
        .filter(|entry| {
            !entry.is_dir && entry.name.starts_with("word/theme/") && entry.name.ends_with(".xml")
        })
        .map(|entry| entry.name.clone())
        .collect();
    candidates.sort();
    Ok(candidates.into_iter().next())
}

fn parse_word_theme(xml: &[u8], part: &str) -> Result<WordTheme, HcdError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut theme = WordTheme::default();
    let mut font_set = None;
    let mut color_slot: Option<String> = None;
    let mut depth = 0usize;
    let mut elements = 0usize;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("invalid {part}: {error}")))?;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                elements += 1;
                validate_theme_xml_budget(part, depth, elements)?;
                begin_theme_element(element, &mut theme, &mut font_set, &mut color_slot);
            }
            Event::Empty(ref element) => {
                elements += 1;
                validate_theme_xml_budget(part, depth, elements)?;
                begin_theme_element(element, &mut theme, &mut font_set, &mut color_slot);
                finish_theme_element(
                    local_name(element.name().as_ref()),
                    &mut font_set,
                    &mut color_slot,
                );
            }
            Event::End(ref element) => {
                finish_theme_element(
                    local_name(element.name().as_ref()),
                    &mut font_set,
                    &mut color_slot,
                );
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| HcdError::InvalidBundle(format!("unbalanced {part}")))?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 {
        return Err(HcdError::InvalidBundle(format!("unbalanced {part}")));
    }
    Ok(theme)
}

fn validate_theme_xml_budget(part: &str, depth: usize, elements: usize) -> Result<(), HcdError> {
    if depth > MAX_XML_DEPTH || elements > MAX_XML_ELEMENTS {
        return Err(HcdError::ResourceLimit(format!(
            "{part} exceeds XML safety limits"
        )));
    }
    Ok(())
}

fn begin_theme_element(
    element: &BytesStart<'_>,
    theme: &mut WordTheme,
    font_set: &mut Option<ThemeFontSet>,
    color_slot: &mut Option<String>,
) {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    match name {
        "majorFont" => *font_set = Some(ThemeFontSet::Major),
        "minorFont" => *font_set = Some(ThemeFontSet::Minor),
        "dk1" | "lt1" | "dk2" | "lt2" | "accent1" | "accent2" | "accent3" | "accent4"
        | "accent5" | "accent6" | "hlink" | "folHlink" => *color_slot = Some(name.to_string()),
        "latin" | "ea" | "cs" => {
            let Some(set) = *font_set else {
                return;
            };
            let font = attribute_by_local_name(element, "typeface")
                .filter(|value| safe_font_family(value).is_some());
            let destination = match (set, name) {
                (ThemeFontSet::Major, "latin") => &mut theme.major_latin,
                (ThemeFontSet::Major, "ea") => &mut theme.major_east_asia,
                (ThemeFontSet::Major, "cs") => &mut theme.major_bidi,
                (ThemeFontSet::Minor, "latin") => &mut theme.minor_latin,
                (ThemeFontSet::Minor, "ea") => &mut theme.minor_east_asia,
                (ThemeFontSet::Minor, "cs") => &mut theme.minor_bidi,
                _ => return,
            };
            if font.as_deref().is_some_and(|value| !value.is_empty()) {
                *destination = font;
            }
        }
        "font" => {
            let Some(set) = *font_set else {
                return;
            };
            let Some(script) = attribute_by_local_name(element, "script")
                .filter(|value| is_safe_theme_script(value))
            else {
                return;
            };
            let Some(font) = attribute_by_local_name(element, "typeface")
                .filter(|value| safe_font_family(value).is_some())
                .filter(|value| !value.is_empty())
            else {
                return;
            };
            match set {
                ThemeFontSet::Major => {
                    theme.major_supplemental.insert(script, font);
                }
                ThemeFontSet::Minor => {
                    theme.minor_supplemental.insert(script, font);
                }
            }
        }
        "srgbClr" => {
            if let (Some(slot), Some(color)) = (
                color_slot.as_ref(),
                attribute_by_local_name(element, "val")
                    .and_then(|value| normalized_hex_color(&value)),
            ) {
                theme.colors.insert(slot.clone(), color);
            }
        }
        "sysClr" => {
            let color = attribute_by_local_name(element, "lastClr")
                .or_else(|| attribute_by_local_name(element, "val"))
                .and_then(|value| normalized_hex_color(&value));
            if let (Some(slot), Some(color)) = (color_slot.as_ref(), color) {
                theme.colors.insert(slot.clone(), color);
            }
        }
        _ => {}
    }
}

fn finish_theme_element(
    name: &str,
    font_set: &mut Option<ThemeFontSet>,
    color_slot: &mut Option<String>,
) {
    if matches!(name, "majorFont" | "minorFont") {
        *font_set = None;
    }
    if color_slot.as_deref() == Some(name) {
        *color_slot = None;
    }
}

fn load_word_styles(
    archive: &mut StreamingOxmlArchive,
    theme: &WordTheme,
) -> Result<RenderedWordStyles, HcdError> {
    if !archive.contains("word/styles.xml") {
        return Ok(RenderedWordStyles {
            css: default_styles().to_string(),
            table_bands: BTreeMap::new(),
            paragraph_numbering: BTreeMap::new(),
        });
    }
    let xml = archive
        .read_control_part("word/styles.xml", MAX_CONTROL_PART_BYTES)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut elements = 0usize;
    let mut current: Option<(String, WordStyleDefinition)> = None;
    let mut paragraph_properties_depth = None;
    let mut run_properties_depth = None;
    let mut default_properties_depth = None;
    let mut property_change_depth = None;
    let mut table_condition_depth = None;
    let mut default_paragraph = ParagraphFormat::default();
    let mut default_run = RunFormat::default();
    let mut styles = BTreeMap::new();

    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("invalid word/styles.xml: {error}"))
        })?;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                elements += 1;
                if depth > MAX_XML_DEPTH || elements > MAX_XML_ELEMENTS {
                    return Err(HcdError::ResourceLimit(
                        "word/styles.xml exceeds XML safety limits".to_string(),
                    ));
                }
                let qualified_name = element.name();
                let name = local_name(qualified_name.as_ref());
                if name == "tblStylePr" {
                    table_condition_depth = Some(depth);
                } else if table_condition_depth.is_some() {
                    // Conditional table style properties are parsed in a
                    // separate bounded event pass below. They must not leak
                    // into the base paragraph/run style.
                } else if matches!(name, "pPrChange" | "rPrChange" | "tblPrChange") {
                    property_change_depth = Some(depth);
                } else if property_change_depth.is_none() {
                    match name {
                        "docDefaults" => default_properties_depth = Some(depth),
                        "style" => {
                            if let Some(style_id) = attribute_by_local_name(element, "styleId") {
                                current = Some((
                                    style_id,
                                    WordStyleDefinition {
                                        kind: attribute_by_local_name(element, "type"),
                                        is_default: on_off_value(
                                            attribute_by_local_name(element, "default").as_deref(),
                                        ),
                                        ..Default::default()
                                    },
                                ));
                            }
                        }
                        "pPr" => paragraph_properties_depth = Some(depth),
                        "rPr" => run_properties_depth = Some(depth),
                        _ => capture_style_property(
                            element,
                            &mut current,
                            &mut default_paragraph,
                            &mut default_run,
                            StylePropertyScope {
                                in_paragraph_properties: paragraph_properties_depth.is_some(),
                                in_run_properties: run_properties_depth.is_some(),
                                in_defaults: default_properties_depth.is_some(),
                                theme,
                            },
                        ),
                    }
                }
            }
            Event::Empty(ref element) => {
                elements += 1;
                if elements > MAX_XML_ELEMENTS {
                    return Err(HcdError::ResourceLimit(
                        "word/styles.xml exceeds XML safety limits".to_string(),
                    ));
                }
                if property_change_depth.is_none() && table_condition_depth.is_none() {
                    capture_style_property(
                        element,
                        &mut current,
                        &mut default_paragraph,
                        &mut default_run,
                        StylePropertyScope {
                            in_paragraph_properties: paragraph_properties_depth.is_some(),
                            in_run_properties: run_properties_depth.is_some(),
                            in_defaults: default_properties_depth.is_some(),
                            theme,
                        },
                    );
                }
            }
            Event::End(ref element) => {
                let qualified_name = element.name();
                if table_condition_depth.is_some() {
                    if table_condition_depth == Some(depth) {
                        table_condition_depth = None;
                    }
                } else if property_change_depth.is_some() {
                    if property_change_depth == Some(depth) {
                        property_change_depth = None;
                    }
                } else {
                    match local_name(qualified_name.as_ref()) {
                        "pPr" if paragraph_properties_depth == Some(depth) => {
                            paragraph_properties_depth = None
                        }
                        "rPr" if run_properties_depth == Some(depth) => run_properties_depth = None,
                        "style" => {
                            if let Some((style_id, definition)) = current.take() {
                                styles.insert(style_id, definition);
                            }
                        }
                        "docDefaults" if default_properties_depth == Some(depth) => {
                            default_properties_depth = None
                        }
                        _ => {}
                    }
                }
                depth = depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle("unbalanced word/styles.xml".to_string())
                })?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    if depth != 0 || property_change_depth.is_some() || table_condition_depth.is_some() {
        return Err(HcdError::InvalidBundle(
            "unbalanced word/styles.xml".to_string(),
        ));
    }

    parse_table_style_layers(&xml, theme, &mut styles)?;

    let mut css = default_styles().to_string();
    css.push_str(".hcd-empty-paragraph{min-height:1lh}");
    css.push_str(".hcd-list-marker{box-sizing:border-box;text-indent:0}");
    let default_run_css = run_css_declarations(&default_run);
    if !default_run_css.is_empty() {
        css.push_str(".hcd-chunk{");
        css.push_str(&default_run_css.join(";"));
        css.push('}');
    }
    let mut default_paragraph_css = paragraph_css_declarations(&default_paragraph);
    if let Some((style_id, _)) = styles.iter().find(|(_, definition)| {
        definition.kind.as_deref() == Some("paragraph") && definition.is_default
    }) {
        collect_style_declarations(
            style_id,
            &styles,
            &mut HashSet::new(),
            &mut default_paragraph_css,
        );
    }
    if !default_paragraph_css.is_empty() {
        css.push_str(".hcd-paragraph{");
        css.push_str(&default_paragraph_css.join(";"));
        css.push('}');
    }
    for (style_id, definition) in &styles {
        let mut declarations = Vec::new();
        collect_style_declarations(style_id, &styles, &mut HashSet::new(), &mut declarations);
        if !declarations.is_empty() {
            css.push('.');
            css.push_str(&word_style_class(style_id));
            css.push('{');
            css.push_str(&declarations.join(";"));
            css.push('}');
        } else if definition.kind.as_deref() == Some("paragraph") {
            // Keep deterministic style discovery through data-hcd-word-style;
            // an empty CSS rule has no rendering value and is omitted.
        }
    }
    append_table_style_css(&mut css, &styles);
    hcd_core::validate_css_text(&css)?;
    let table_bands = resolve_table_band_catalog(&styles);
    let paragraph_numbering = resolve_paragraph_style_numbering(&styles);
    Ok(RenderedWordStyles {
        css,
        table_bands,
        paragraph_numbering,
    })
}

fn resolve_paragraph_style_numbering(
    styles: &BTreeMap<String, WordStyleDefinition>,
) -> BTreeMap<String, (String, Option<String>)> {
    fn resolve(
        style_id: &str,
        styles: &BTreeMap<String, WordStyleDefinition>,
        visiting: &mut HashSet<String>,
    ) -> (Option<String>, Option<String>) {
        if !visiting.insert(style_id.to_string()) {
            return (None, None);
        }
        let Some(style) = styles.get(style_id) else {
            return (None, None);
        };
        let inherited = style
            .based_on
            .as_deref()
            .map(|parent| resolve(parent, styles, visiting))
            .unwrap_or_default();
        (
            style.paragraph.numbering_id.clone().or(inherited.0),
            style.paragraph.numbering_level.clone().or(inherited.1),
        )
    }

    styles
        .keys()
        .filter_map(|style_id| {
            let (number_id, level) = resolve(style_id, styles, &mut HashSet::new());
            number_id.map(|number_id| (style_id.clone(), (number_id, level)))
        })
        .collect()
}

fn resolve_table_band_catalog(styles: &BTreeMap<String, WordStyleDefinition>) -> TableBandCatalog {
    styles
        .iter()
        .filter(|(_, style)| style.kind.as_deref() == Some("table"))
        .map(|(style_id, _)| {
            let mut sizes = TableBandSizes::default();
            collect_table_band_sizes(style_id, styles, &mut HashSet::new(), &mut sizes);
            (style_id.clone(), sizes)
        })
        .collect()
}

fn collect_table_band_sizes(
    style_id: &str,
    styles: &BTreeMap<String, WordStyleDefinition>,
    visiting: &mut HashSet<String>,
    sizes: &mut TableBandSizes,
) {
    if !visiting.insert(style_id.to_string()) {
        return;
    }
    let Some(style) = styles.get(style_id) else {
        return;
    };
    if let Some(parent) = &style.based_on {
        collect_table_band_sizes(parent, styles, visiting, sizes);
    }
    if let Some(row) = style.table_row_band_size {
        sizes.row = row;
    }
    if let Some(column) = style.table_column_band_size {
        sizes.column = column;
    }
}

#[derive(Default)]
struct TableStyleDepths {
    paragraph: Option<usize>,
    run: Option<usize>,
    table: Option<usize>,
    row: Option<usize>,
    cell: Option<usize>,
    table_borders: Option<usize>,
    cell_borders: Option<usize>,
    cell_margins: Option<usize>,
    property_change: Option<usize>,
}

struct TableStyleScope<'a> {
    in_paragraph: bool,
    in_run: bool,
    in_table: bool,
    in_row: bool,
    in_cell: bool,
    in_table_borders: bool,
    in_cell_borders: bool,
    in_cell_margins: bool,
    theme: &'a WordTheme,
}

fn parse_table_style_layers(
    xml: &[u8],
    theme: &WordTheme,
    styles: &mut BTreeMap<String, WordStyleDefinition>,
) -> Result<(), HcdError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut elements = 0usize;
    let mut current_style: Option<String> = None;
    let mut current_condition: Option<(String, TableStyleLayer)> = None;
    let mut scopes = TableStyleDepths::default();

    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("invalid table styles in word/styles.xml: {error}"))
        })?;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                elements += 1;
                validate_table_style_budget(depth, elements)?;
                let qualified_name = element.name();
                let name = local_name(qualified_name.as_ref());
                if scopes.property_change.is_some() {
                    // Historical style snapshots do not affect the current table style.
                } else if matches!(name, "pPrChange" | "rPrChange" | "tblPrChange") {
                    scopes.property_change = Some(depth);
                } else {
                    match name {
                        "style" => {
                            current_style =
                                attribute_by_local_name(element, "styleId").filter(|style_id| {
                                    styles.get(style_id).is_some_and(|definition| {
                                        definition.kind.as_deref() == Some("table")
                                    })
                                });
                        }
                        "tblStylePr" if current_style.is_some() => {
                            if let Some(condition) = attribute_by_local_name(element, "type")
                                .filter(|value| safe_table_condition(value))
                            {
                                current_condition = Some((condition, TableStyleLayer::default()));
                            }
                        }
                        "pPr" if current_style.is_some() => scopes.paragraph = Some(depth),
                        "rPr" if current_style.is_some() => scopes.run = Some(depth),
                        "tblPr" if current_style.is_some() => scopes.table = Some(depth),
                        "trPr" if current_style.is_some() => scopes.row = Some(depth),
                        "tcPr" if current_style.is_some() => scopes.cell = Some(depth),
                        "tblBorders" if scopes.table.is_some() => {
                            scopes.table_borders = Some(depth)
                        }
                        "tcBorders" if scopes.cell.is_some() => scopes.cell_borders = Some(depth),
                        "tblCellMar" if scopes.table.is_some() => scopes.cell_margins = Some(depth),
                        _ => {
                            capture_current_table_style_property(
                                element,
                                styles,
                                current_style.as_deref(),
                                &mut current_condition,
                                table_style_scope(&scopes, theme),
                            )?;
                        }
                    }
                }
            }
            Event::Empty(ref element) => {
                elements += 1;
                validate_table_style_budget(depth, elements)?;
                if scopes.property_change.is_none() {
                    capture_current_table_style_property(
                        element,
                        styles,
                        current_style.as_deref(),
                        &mut current_condition,
                        table_style_scope(&scopes, theme),
                    )?;
                }
            }
            Event::End(ref element) => {
                let qualified_name = element.name();
                let name = local_name(qualified_name.as_ref());
                if scopes.property_change.is_some() {
                    if scopes.property_change == Some(depth) {
                        scopes.property_change = None;
                    }
                } else {
                    match name {
                        "pPr" if scopes.paragraph == Some(depth) => scopes.paragraph = None,
                        "rPr" if scopes.run == Some(depth) => scopes.run = None,
                        "tblPr" if scopes.table == Some(depth) => scopes.table = None,
                        "trPr" if scopes.row == Some(depth) => scopes.row = None,
                        "tcPr" if scopes.cell == Some(depth) => scopes.cell = None,
                        "tblBorders" if scopes.table_borders == Some(depth) => {
                            scopes.table_borders = None
                        }
                        "tcBorders" if scopes.cell_borders == Some(depth) => {
                            scopes.cell_borders = None
                        }
                        "tblCellMar" if scopes.cell_margins == Some(depth) => {
                            scopes.cell_margins = None
                        }
                        "tblStylePr" => {
                            if let (Some(style_id), Some((condition, layer))) =
                                (current_style.as_deref(), current_condition.take())
                            {
                                if let Some(style) = styles.get_mut(style_id) {
                                    style.table_conditions.insert(condition, layer);
                                }
                            }
                        }
                        "style" => {
                            current_condition = None;
                            current_style = None;
                        }
                        _ => {}
                    }
                }
                depth = depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle(
                        "unbalanced table styles in word/styles.xml".to_string(),
                    )
                })?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0
        || current_condition.is_some()
        || scopes.property_change.is_some()
        || scopes.paragraph.is_some()
        || scopes.run.is_some()
        || scopes.table.is_some()
        || scopes.row.is_some()
        || scopes.cell.is_some()
    {
        return Err(HcdError::InvalidBundle(
            "unbalanced table style structure in word/styles.xml".to_string(),
        ));
    }
    Ok(())
}

fn validate_table_style_budget(depth: usize, elements: usize) -> Result<(), HcdError> {
    if depth > MAX_XML_DEPTH || elements > MAX_XML_ELEMENTS {
        return Err(HcdError::ResourceLimit(
            "word/styles.xml table styles exceed XML safety limits".to_string(),
        ));
    }
    Ok(())
}

fn table_style_scope<'a>(depths: &TableStyleDepths, theme: &'a WordTheme) -> TableStyleScope<'a> {
    TableStyleScope {
        in_paragraph: depths.paragraph.is_some(),
        in_run: depths.run.is_some(),
        in_table: depths.table.is_some(),
        in_row: depths.row.is_some(),
        in_cell: depths.cell.is_some(),
        in_table_borders: depths.table_borders.is_some(),
        in_cell_borders: depths.cell_borders.is_some(),
        in_cell_margins: depths.cell_margins.is_some(),
        theme,
    }
}

fn capture_current_table_style_property(
    element: &BytesStart<'_>,
    styles: &mut BTreeMap<String, WordStyleDefinition>,
    current_style: Option<&str>,
    current_condition: &mut Option<(String, TableStyleLayer)>,
    scope: TableStyleScope<'_>,
) -> Result<(), HcdError> {
    let Some(style_id) = current_style else {
        return Ok(());
    };
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    if current_condition.is_none()
        && scope.in_table
        && matches!(name, "tblStyleRowBandSize" | "tblStyleColBandSize")
    {
        let value = parse_table_band_size(element, name)?;
        if let Some(style) = styles.get_mut(style_id) {
            if name == "tblStyleRowBandSize" {
                style.table_row_band_size = Some(value);
            } else {
                style.table_column_band_size = Some(value);
            }
        }
        return Ok(());
    }
    if let Some((_, layer)) = current_condition.as_mut() {
        capture_table_style_property(element, layer, scope);
    } else if let Some(style) = styles.get_mut(style_id) {
        capture_table_style_property(element, &mut style.table, scope);
    }
    Ok(())
}

fn capture_table_style_property(
    element: &BytesStart<'_>,
    layer: &mut TableStyleLayer,
    scope: TableStyleScope<'_>,
) {
    if scope.in_paragraph {
        capture_paragraph_format_property(element, &mut layer.paragraph);
    }
    if scope.in_run {
        capture_run_format_property(element, &mut layer.run, scope.theme);
    }
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    if scope.in_table_borders || scope.in_cell_borders {
        capture_table_border(element, &mut layer.cell_css);
    } else if scope.in_cell_margins {
        capture_table_cell_margin(element, &mut layer.cell_css);
    } else if scope.in_cell {
        match name {
            "shd" => capture_table_shading(element, scope.theme, &mut layer.cell_css),
            "vAlign" => {
                if let Some(value) = attribute_by_local_name(element, "val")
                    .and_then(|value| table_cell_vertical_align(&value))
                {
                    layer.cell_css.push(format!("vertical-align:{value}"));
                }
            }
            _ => {}
        }
    } else if scope.in_row {
        match name {
            "trHeight" => {
                if let Some(twips) = attribute_by_local_name(element, "val")
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| *value <= 2_000_000)
                {
                    let property =
                        if attribute_by_local_name(element, "hRule").as_deref() == Some("exact") {
                            "height"
                        } else {
                            "min-height"
                        };
                    layer
                        .row_css
                        .push(format!("{property}:{:.2}pt", twips as f64 / 20.0));
                }
            }
            "cantSplit" if on_off_value(attribute_by_local_name(element, "val").as_deref()) => {
                layer.row_css.push("break-inside:avoid".to_string());
            }
            _ => {}
        }
    } else if scope.in_table {
        match name {
            "shd" => capture_table_shading(element, scope.theme, &mut layer.table_css),
            "tblW" => capture_table_width(element, &mut layer.table_css),
            "jc" => capture_table_alignment(element, &mut layer.table_css),
            "tblLayout" if attribute_by_local_name(element, "type").as_deref() == Some("fixed") => {
                layer.table_css.push("table-layout:fixed".to_string());
            }
            _ => {}
        }
    }
}

fn capture_table_shading(element: &BytesStart<'_>, theme: &WordTheme, output: &mut Vec<String>) {
    let materialized = attribute_by_local_name(element, "fill")
        .filter(|value| value != "auto")
        .and_then(|value| normalized_hex_color(&value));
    let theme_fill = attribute_by_local_name(element, "themeFill")
        .and_then(|slot| theme.color(&slot).map(str::to_string));
    let color = materialized.or_else(|| {
        let base = theme_fill?;
        let shade = attribute_by_local_name(element, "themeFillShade")
            .as_deref()
            .and_then(parse_hex_byte);
        let tint = attribute_by_local_name(element, "themeFillTint")
            .as_deref()
            .and_then(parse_hex_byte);
        transform_theme_color(&base, shade, tint)
    });
    if let Some(color) = color {
        output.push(format!("background-color:#{color}"));
    }
}

fn capture_table_width(element: &BytesStart<'_>, output: &mut Vec<String>) {
    let Some(value) = attribute_by_local_name(element, "w")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= 2_000_000)
    else {
        return;
    };
    match attribute_by_local_name(element, "type").as_deref() {
        Some("dxa") => output.push(format!("width:{:.2}pt", value as f64 / 20.0)),
        Some("pct") if value <= 5_000 => {
            output.push(format!("width:{:.2}%", value as f64 / 50.0));
        }
        _ => {}
    }
}

fn capture_table_alignment(element: &BytesStart<'_>, output: &mut Vec<String>) {
    match attribute_by_local_name(element, "val").as_deref() {
        Some("center") => {
            output.push("margin-left:auto".to_string());
            output.push("margin-right:auto".to_string());
        }
        Some("right" | "end") => {
            output.push("margin-left:auto".to_string());
            output.push("margin-right:0".to_string());
        }
        Some("left" | "start") => {
            output.push("margin-left:0".to_string());
            output.push("margin-right:auto".to_string());
        }
        _ => {}
    }
}

fn capture_table_cell_margin(element: &BytesStart<'_>, output: &mut Vec<String>) {
    let Some(twips) = attribute_by_local_name(element, "w")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= 100_000)
    else {
        return;
    };
    let property = match local_name(element.name().as_ref()) {
        "top" => "padding-top",
        "bottom" => "padding-bottom",
        "left" | "start" => "padding-left",
        "right" | "end" => "padding-right",
        _ => return,
    };
    output.push(format!("{property}:{:.2}pt", twips as f64 / 20.0));
}

fn capture_table_border(element: &BytesStart<'_>, output: &mut Vec<String>) {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    let properties: &[&str] = match name {
        "top" => &["border-top"],
        "bottom" => &["border-bottom"],
        "left" | "start" => &["border-left"],
        "right" | "end" => &["border-right"],
        "insideH" => &["border-top", "border-bottom"],
        "insideV" => &["border-left", "border-right"],
        _ => return,
    };
    let value = attribute_by_local_name(element, "val").unwrap_or_else(|| "single".to_string());
    if matches!(value.as_str(), "nil" | "none") {
        for property in properties {
            output.push(format!("{property}:none"));
        }
        return;
    }
    let style = match value.as_str() {
        "double" => "double",
        "dashed" | "dashSmallGap" | "dashDotStroked" => "dashed",
        "dotted" | "dotDash" | "dotDotDash" => "dotted",
        _ => "solid",
    };
    let width = attribute_by_local_name(element, "sz")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (2..=96).contains(value))
        .unwrap_or(4) as f64
        / 8.0;
    let color = attribute_by_local_name(element, "color")
        .filter(|value| value != "auto")
        .and_then(|value| normalized_hex_color(&value))
        .unwrap_or_else(|| "000000".to_string());
    for property in properties {
        output.push(format!("{property}:{width:.2}pt {style} #{color}"));
    }
}

fn table_cell_vertical_align(value: &str) -> Option<&'static str> {
    match value {
        "top" => Some("top"),
        "center" => Some("middle"),
        "bottom" => Some("bottom"),
        _ => None,
    }
}

fn safe_table_condition(value: &str) -> bool {
    matches!(
        value,
        "wholeTable"
            | "firstRow"
            | "lastRow"
            | "firstCol"
            | "lastCol"
            | "band1Vert"
            | "band2Vert"
            | "band1Horz"
            | "band2Horz"
            | "neCell"
            | "nwCell"
            | "seCell"
            | "swCell"
    )
}

#[derive(Clone, Copy)]
enum TableStyleCssTarget {
    Table,
    Row,
    Cell,
    Text,
}

fn append_table_style_css(css: &mut String, styles: &BTreeMap<String, WordStyleDefinition>) {
    for (style_id, definition) in styles {
        if definition.kind.as_deref() != Some("table") {
            continue;
        }
        let class = word_style_class(style_id);
        for (selector, target) in [
            (format!(".hcd-table.{class}"), TableStyleCssTarget::Table),
            (
                format!(".hcd-table.{class}>tbody>tr"),
                TableStyleCssTarget::Row,
            ),
            (
                format!(".hcd-table.{class}>tbody>tr>td"),
                TableStyleCssTarget::Cell,
            ),
            (format!(".hcd-table.{class}"), TableStyleCssTarget::Text),
        ] {
            let mut declarations = Vec::new();
            collect_table_style_css(
                style_id,
                None,
                target,
                styles,
                &mut HashSet::new(),
                &mut declarations,
            );
            append_css_rule(css, &selector, &declarations);
        }
        // Microsoft Word applies later conditional regions over earlier ones:
        // row bands, column bands, first/last columns, first/last rows, corners.
        // The selectors below intentionally have comparable specificity, so
        // preserving this source order is part of the fidelity contract.
        for condition in [
            "band1Horz",
            "band2Horz",
            "band1Vert",
            "band2Vert",
            "firstCol",
            "lastCol",
            "firstRow",
            "lastRow",
            "nwCell",
            "neCell",
            "swCell",
            "seCell",
        ] {
            let Some(selector) = table_condition_selector(&class, condition) else {
                continue;
            };
            let mut declarations = Vec::new();
            for target in [
                TableStyleCssTarget::Table,
                TableStyleCssTarget::Row,
                TableStyleCssTarget::Cell,
                TableStyleCssTarget::Text,
            ] {
                collect_table_style_css(
                    style_id,
                    Some(condition),
                    target,
                    styles,
                    &mut HashSet::new(),
                    &mut declarations,
                );
            }
            append_css_rule(css, &selector, &declarations);

            let Some(data_attribute) = conditional_style_data_attribute(condition) else {
                continue;
            };
            let table = format!(".hcd-table.{class}");
            for explicit_selector in [
                format!("{table}>tbody>tr[{data_attribute}=\"true\"]>td"),
                format!("{table}>tbody>tr>td[{data_attribute}=\"true\"]"),
            ] {
                append_css_rule(css, &explicit_selector, &declarations);
            }

            let mut paragraph_declarations = Vec::new();
            collect_table_style_css(
                style_id,
                Some(condition),
                TableStyleCssTarget::Text,
                styles,
                &mut HashSet::new(),
                &mut paragraph_declarations,
            );
            append_css_rule(
                css,
                &format!("{table}>tbody>tr>td .hcd-paragraph[{data_attribute}=\"true\"]"),
                &paragraph_declarations,
            );
        }
    }
}

fn collect_table_style_css(
    style_id: &str,
    condition: Option<&str>,
    target: TableStyleCssTarget,
    styles: &BTreeMap<String, WordStyleDefinition>,
    visiting: &mut HashSet<String>,
    output: &mut Vec<String>,
) {
    if !visiting.insert(style_id.to_string()) {
        return;
    }
    let Some(style) = styles.get(style_id) else {
        return;
    };
    if let Some(parent) = &style.based_on {
        collect_table_style_css(parent, condition, target, styles, visiting, output);
    }
    let layer = condition
        .and_then(|condition| style.table_conditions.get(condition))
        .or_else(|| condition.is_none().then_some(&style.table));
    if condition.is_none() {
        append_table_layer_target(&style.table, target, output);
        if let Some(whole_table) = style.table_conditions.get("wholeTable") {
            append_table_layer_target(whole_table, target, output);
        }
    } else if let Some(layer) = layer {
        append_table_layer_target(layer, target, output);
    }
}

fn append_table_layer_target(
    layer: &TableStyleLayer,
    target: TableStyleCssTarget,
    output: &mut Vec<String>,
) {
    match target {
        TableStyleCssTarget::Table => output.extend(layer.table_css.iter().cloned()),
        TableStyleCssTarget::Row => output.extend(layer.row_css.iter().cloned()),
        TableStyleCssTarget::Cell => output.extend(layer.cell_css.iter().cloned()),
        TableStyleCssTarget::Text => {
            output.extend(paragraph_css_declarations(&layer.paragraph));
            output.extend(run_css_declarations(&layer.run));
        }
    }
}

fn append_css_rule(css: &mut String, selector: &str, declarations: &[String]) {
    if declarations.is_empty() {
        return;
    }
    css.push_str(selector);
    css.push('{');
    css.push_str(&declarations.join(";"));
    css.push('}');
}

fn table_condition_selector(class: &str, condition: &str) -> Option<String> {
    let table = format!(".hcd-table.{class}");
    Some(match condition {
        "firstRow" => format!("{table}[data-hcd-look-first-row=\"true\"]>tbody>tr:first-child>td"),
        "lastRow" => format!("{table}[data-hcd-look-last-row=\"true\"]>tbody>tr:last-child>td"),
        "firstCol" => {
            format!("{table}[data-hcd-look-first-column=\"true\"]>tbody>tr>td:first-child")
        }
        "lastCol" => format!("{table}[data-hcd-look-last-column=\"true\"]>tbody>tr>td:last-child"),
        "band1Vert" => format!(
            "{table}[data-hcd-look-v-band=\"true\"]>tbody>tr>td[data-hcd-column-band=\"1\"]"
        ),
        "band2Vert" => format!(
            "{table}[data-hcd-look-v-band=\"true\"]>tbody>tr>td[data-hcd-column-band=\"2\"]"
        ),
        "band1Horz" => {
            format!("{table}[data-hcd-look-h-band=\"true\"]>tbody>tr[data-hcd-row-band=\"1\"]>td")
        }
        "band2Horz" => {
            format!("{table}[data-hcd-look-h-band=\"true\"]>tbody>tr[data-hcd-row-band=\"2\"]>td")
        }
        "nwCell" => {
            format!("{table}[data-hcd-look-first-row=\"true\"][data-hcd-look-first-column=\"true\"]>tbody>tr:first-child>td:first-child")
        }
        "neCell" => {
            format!("{table}[data-hcd-look-first-row=\"true\"][data-hcd-look-last-column=\"true\"]>tbody>tr:first-child>td:last-child")
        }
        "swCell" => {
            format!("{table}[data-hcd-look-last-row=\"true\"][data-hcd-look-first-column=\"true\"]>tbody>tr:last-child>td:first-child")
        }
        "seCell" => {
            format!("{table}[data-hcd-look-last-row=\"true\"][data-hcd-look-last-column=\"true\"]>tbody>tr:last-child>td:last-child")
        }
        _ => return None,
    })
}

fn load_word_numbering(archive: &mut StreamingOxmlArchive) -> Result<NumberingCatalog, HcdError> {
    if !archive.contains("word/numbering.xml") {
        return Ok(NumberingCatalog::default());
    }
    let xml = archive
        .read_control_part("word/numbering.xml", MAX_CONTROL_PART_BYTES)
        .map_err(package_error)?;
    parse_word_numbering(&xml)
}

fn parse_word_numbering(xml: &[u8]) -> Result<NumberingCatalog, HcdError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut catalog = NumberingCatalog::default();
    let mut current_abstract: Option<String> = None;
    let mut current_instance: Option<String> = None;
    let mut current_override_level: Option<u8> = None;
    let mut current_level: Option<PendingNumberLevel> = None;
    let mut depth = 0usize;
    let mut elements = 0usize;

    loop {
        let event = reader.read_event_into(&mut buffer).map_err(|error| {
            HcdError::InvalidBundle(format!("invalid word/numbering.xml: {error}"))
        })?;
        match event {
            Event::Start(ref element) => {
                depth += 1;
                elements += 1;
                if depth > MAX_XML_DEPTH || elements > MAX_XML_ELEMENTS {
                    return Err(HcdError::ResourceLimit(
                        "word/numbering.xml exceeds XML safety limits".to_string(),
                    ));
                }
                begin_numbering_element(
                    element,
                    &mut catalog,
                    &mut current_abstract,
                    &mut current_instance,
                    &mut current_override_level,
                    &mut current_level,
                )?;
            }
            Event::Empty(ref element) => {
                elements += 1;
                if elements > MAX_XML_ELEMENTS {
                    return Err(HcdError::ResourceLimit(
                        "word/numbering.xml exceeds XML safety limits".to_string(),
                    ));
                }
                begin_numbering_element(
                    element,
                    &mut catalog,
                    &mut current_abstract,
                    &mut current_instance,
                    &mut current_override_level,
                    &mut current_level,
                )?;
                finish_empty_numbering_element(
                    element,
                    &mut catalog,
                    &mut current_abstract,
                    &mut current_instance,
                    &mut current_override_level,
                    &mut current_level,
                );
            }
            Event::End(ref element) => {
                finish_numbering_element(
                    local_name(element.name().as_ref()),
                    &mut catalog,
                    &mut current_abstract,
                    &mut current_instance,
                    &mut current_override_level,
                    &mut current_level,
                );
                depth = depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle("unbalanced word/numbering.xml".to_string())
                })?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 || current_level.is_some() {
        return Err(HcdError::InvalidBundle(
            "unbalanced word/numbering.xml".to_string(),
        ));
    }
    Ok(catalog)
}

fn begin_numbering_element(
    element: &BytesStart<'_>,
    catalog: &mut NumberingCatalog,
    current_abstract: &mut Option<String>,
    current_instance: &mut Option<String>,
    current_override_level: &mut Option<u8>,
    current_level: &mut Option<PendingNumberLevel>,
) -> Result<(), HcdError> {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    match name {
        "abstractNum" => {
            if let Some(id) = attribute_by_local_name(element, "abstractNumId") {
                catalog.abstracts.entry(id.clone()).or_default();
                *current_abstract = Some(id);
            }
        }
        "num" => {
            if let Some(id) = attribute_by_local_name(element, "numId") {
                catalog.instances.entry(id.clone()).or_default();
                *current_instance = Some(id);
            }
        }
        "lvlOverride" => {
            *current_override_level = numbering_level_index(element);
        }
        "lvl" => {
            let index = numbering_level_index(element).ok_or_else(|| {
                HcdError::InvalidBundle("numbering level is missing a valid ilvl".to_string())
            })?;
            let target = if let Some(instance) = current_instance.clone() {
                NumberLevelTarget::Instance(instance)
            } else if let Some(abstract_id) = current_abstract.clone() {
                NumberLevelTarget::Abstract(abstract_id)
            } else {
                return Ok(());
            };
            *current_level = Some(PendingNumberLevel {
                index,
                definition: NumberLevel::default(),
                target,
            });
        }
        "abstractNumId" => {
            if let (Some(instance_id), Some(abstract_id)) = (
                current_instance.as_ref(),
                attribute_by_local_name(element, "val"),
            ) {
                catalog
                    .instances
                    .entry(instance_id.clone())
                    .or_default()
                    .abstract_id = abstract_id;
            }
        }
        "startOverride" => {
            if let (Some(instance_id), Some(level), Some(value)) = (
                current_instance.as_ref(),
                *current_override_level,
                numbering_start_value(element),
            ) {
                catalog
                    .instances
                    .entry(instance_id.clone())
                    .or_default()
                    .start_overrides
                    .insert(level, value);
            }
        }
        _ => capture_number_level_property(element, current_level.as_mut()),
    }
    Ok(())
}

fn finish_empty_numbering_element(
    element: &BytesStart<'_>,
    catalog: &mut NumberingCatalog,
    current_abstract: &mut Option<String>,
    current_instance: &mut Option<String>,
    current_override_level: &mut Option<u8>,
    current_level: &mut Option<PendingNumberLevel>,
) {
    finish_numbering_element(
        local_name(element.name().as_ref()),
        catalog,
        current_abstract,
        current_instance,
        current_override_level,
        current_level,
    );
}

fn finish_numbering_element(
    name: &str,
    catalog: &mut NumberingCatalog,
    current_abstract: &mut Option<String>,
    current_instance: &mut Option<String>,
    current_override_level: &mut Option<u8>,
    current_level: &mut Option<PendingNumberLevel>,
) {
    match name {
        "lvl" => {
            if let Some(level) = current_level.take() {
                match level.target {
                    NumberLevelTarget::Abstract(id) => {
                        catalog
                            .abstracts
                            .entry(id)
                            .or_default()
                            .levels
                            .insert(level.index, level.definition);
                    }
                    NumberLevelTarget::Instance(id) => {
                        catalog
                            .instances
                            .entry(id)
                            .or_default()
                            .level_overrides
                            .insert(level.index, level.definition);
                    }
                }
            }
        }
        "lvlOverride" => *current_override_level = None,
        "abstractNum" => *current_abstract = None,
        "num" => *current_instance = None,
        _ => {}
    }
}

fn capture_number_level_property(element: &BytesStart<'_>, level: Option<&mut PendingNumberLevel>) {
    let Some(level) = level else {
        return;
    };
    let value = attribute_by_local_name(element, "val");
    match local_name(element.name().as_ref()) {
        "start" => {
            if let Some(value) = numbering_start_value(element) {
                level.definition.start = value;
            }
        }
        "numFmt" => level.definition.number_format = value.unwrap_or_default(),
        "lvlText" => level.definition.level_text = value.unwrap_or_default(),
        "suff" => level.definition.suffix = value.unwrap_or_default(),
        "lvlJc" => level.definition.alignment = value,
        "ind" => {
            level.definition.left_twips =
                signed_attribute(element, "left").or_else(|| signed_attribute(element, "start"));
            level.definition.hanging_twips = signed_attribute(element, "hanging");
        }
        "rFonts" => {
            level.definition.font = attribute_by_local_name(element, "ascii")
                .or_else(|| attribute_by_local_name(element, "hAnsi"))
                .or_else(|| attribute_by_local_name(element, "hint"));
        }
        "sz" => {
            level.definition.size_half_points = value.and_then(|value| value.parse::<u32>().ok())
        }
        "color" => {
            level.definition.color = value.as_deref().and_then(normalized_hex_color);
        }
        "b" => level.definition.bold = on_off_value(value.as_deref()),
        "i" => level.definition.italic = on_off_value(value.as_deref()),
        _ => {}
    }
}

fn numbering_level_index(element: &BytesStart<'_>) -> Option<u8> {
    attribute_by_local_name(element, "ilvl")
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 8)
}

fn numbering_start_value(element: &BytesStart<'_>) -> Option<i64> {
    attribute_by_local_name(element, "val")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (-2_000_000_000..=2_000_000_000).contains(value))
}

impl NumberingCatalog {
    fn level<'a>(&'a self, instance: &'a NumberingInstance, index: u8) -> Option<&'a NumberLevel> {
        instance.level_overrides.get(&index).or_else(|| {
            self.abstracts
                .get(&instance.abstract_id)
                .and_then(|abstract_numbering| abstract_numbering.levels.get(&index))
        })
    }
}

impl NumberingState {
    #[cfg(test)]
    fn render_marker(
        &mut self,
        catalog: &NumberingCatalog,
        number_id: Option<&str>,
        level: Option<&str>,
    ) -> Option<String> {
        self.render_marker_details(catalog, number_id, level)
            .map(|marker| marker.html)
    }

    fn render_marker_details(
        &mut self,
        catalog: &NumberingCatalog,
        number_id: Option<&str>,
        level: Option<&str>,
    ) -> Option<RenderedNumberMarker> {
        let number_id = number_id.filter(|value| *value != "0")?;
        let level = level
            .unwrap_or("0")
            .parse::<u8>()
            .ok()
            .filter(|value| *value <= 8)?;
        let instance = catalog.instances.get(number_id)?;
        let definition = catalog.level(instance, level)?.clone();
        if definition.number_format == "none" {
            return None;
        }

        let counters = self.instances.entry(number_id.to_string()).or_default();
        for deeper in usize::from(level + 1)..counters.initialized.len() {
            counters.values[deeper] = 0;
            counters.initialized[deeper] = false;
        }
        for parent in 0..=level {
            let index = usize::from(parent);
            if !counters.initialized[index] {
                let start = if let Some(start) = instance.start_overrides.get(&parent).copied() {
                    start
                } else if let Some(previous) = self
                    .abstracts
                    .get(&instance.abstract_id)
                    .filter(|state| state.initialized[index])
                    .map(|state| state.values[index])
                {
                    previous.saturating_add(1)
                } else {
                    catalog
                        .level(instance, parent)
                        .map(|level| level.start)
                        .unwrap_or(1)
                };
                counters.values[index] = start;
                counters.initialized[index] = true;
            } else if parent == level {
                counters.values[index] = counters.values[index].saturating_add(1);
            }
        }

        let abstract_counters = self
            .abstracts
            .entry(instance.abstract_id.clone())
            .or_default();
        for index in 0..=usize::from(level) {
            if counters.initialized[index] {
                abstract_counters.values[index] = counters.values[index];
                abstract_counters.initialized[index] = true;
            }
        }
        for deeper in usize::from(level + 1)..abstract_counters.initialized.len() {
            abstract_counters.values[deeper] = 0;
            abstract_counters.initialized[deeper] = false;
        }

        let mut marker = definition.level_text.clone();
        for referenced_level in 0..=8u8 {
            let token = format!("%{}", referenced_level + 1);
            if !marker.contains(&token) {
                continue;
            }
            let value = counters.values[usize::from(referenced_level)];
            let format = catalog
                .level(instance, referenced_level)
                .map(|level| level.number_format.as_str())
                .unwrap_or("decimal");
            marker = marker.replace(&token, &format_list_number(value, format));
        }
        if marker.is_empty() {
            return None;
        }

        let mut attributes = format!(
            " class=\"hcd-list-marker\" data-hcd-editable=\"false\" data-hcd-num-format=\"{}\"",
            escape_attribute(&definition.number_format)
        );
        let mut marker_css = Vec::new();
        if let Some(font) = definition.font.as_deref().and_then(safe_font_family) {
            marker_css.push(format!("font-family:'{}'", font.replace('\'', "")));
        }
        if let Some(size) = definition.size_half_points {
            marker_css.push(format!("font-size:{:.1}pt", f64::from(size) / 2.0));
            marker_css.push(format!(
                "line-height:{:.4}",
                word_marker_line_height(definition.font.as_deref())
            ));
        }
        if let Some(color) = &definition.color {
            marker_css.push(format!("color:#{color}"));
        }
        if definition.bold {
            marker_css.push("font-weight:700".to_string());
        }
        if definition.italic {
            marker_css.push("font-style:italic".to_string());
        }
        if let Some(alignment) = &definition.alignment {
            marker_css.push(format!(
                "text-align:{}",
                match alignment.as_str() {
                    "right" | "end" => "right",
                    "center" => "center",
                    _ => "left",
                }
            ));
        }
        if let Some(hanging) = definition.hanging_twips.filter(|value| *value > 0) {
            marker_css.push(format!(
                "display:inline-block;min-width:{:.1}pt",
                hanging as f64 / 20.0
            ));
        }
        match definition.suffix.as_str() {
            "nothing" => {}
            "space" => marker_css.push("padding-right:0.25em".to_string()),
            _ => marker_css.push("padding-right:0.5em".to_string()),
        }
        if !marker_css.is_empty() {
            attributes.push_str(" style=\"");
            attributes.push_str(&escape_attribute(&marker_css.join(";")));
            attributes.push('"');
        }
        Some(RenderedNumberMarker {
            html: format!("<span{attributes}>{}</span>", escape_text(&marker)),
            definition,
        })
    }
}

fn format_list_number(value: i64, format: &str) -> String {
    match format {
        "lowerLetter" => alphabetic_number(value, false),
        "upperLetter" => alphabetic_number(value, true),
        "lowerRoman" => roman_number(value, false),
        "upperRoman" => roman_number(value, true),
        "decimalZero" if (0..=9).contains(&value) => format!("0{value}"),
        "ordinal" => english_ordinal(value),
        _ => value.to_string(),
    }
}

fn alphabetic_number(mut value: i64, uppercase: bool) -> String {
    if value <= 0 {
        return value.to_string();
    }
    let mut encoded = Vec::new();
    while value > 0 {
        value -= 1;
        let base = if uppercase { b'A' } else { b'a' };
        encoded.push((base + (value % 26) as u8) as char);
        value /= 26;
    }
    encoded.iter().rev().collect()
}

fn roman_number(value: i64, uppercase: bool) -> String {
    if !(1..=3999).contains(&value) {
        return value.to_string();
    }
    let mut value = value;
    let mut output = String::new();
    for (unit, glyph) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while value >= unit {
            value -= unit;
            output.push_str(glyph);
        }
    }
    if uppercase {
        output
    } else {
        output.to_ascii_lowercase()
    }
}

fn english_ordinal(value: i64) -> String {
    let absolute = value.unsigned_abs();
    let suffix = if (11..=13).contains(&(absolute % 100)) {
        "th"
    } else {
        match absolute % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    };
    format!("{value}{suffix}")
}

fn capture_style_property(
    element: &BytesStart<'_>,
    current: &mut Option<(String, WordStyleDefinition)>,
    default_paragraph: &mut ParagraphFormat,
    default_run: &mut RunFormat,
    scope: StylePropertyScope<'_>,
) {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    if name == "basedOn" {
        if let Some((_, definition)) = current {
            definition.based_on = attribute_by_local_name(element, "val");
        }
        return;
    }
    if name == "link" {
        if let Some((_, definition)) = current {
            definition.linked = attribute_by_local_name(element, "val");
        }
        return;
    }
    if scope.in_paragraph_properties {
        if let Some((_, definition)) = current {
            capture_paragraph_format_property(element, &mut definition.paragraph);
        } else if scope.in_defaults {
            capture_paragraph_format_property(element, default_paragraph);
        }
    }
    if scope.in_run_properties {
        if let Some((_, definition)) = current {
            capture_run_format_property(element, &mut definition.run, scope.theme);
        } else if scope.in_defaults {
            capture_run_format_property(element, default_run, scope.theme);
        }
    }
}

fn collect_style_declarations(
    style_id: &str,
    styles: &BTreeMap<String, WordStyleDefinition>,
    visiting: &mut HashSet<String>,
    output: &mut Vec<String>,
) {
    if !visiting.insert(style_id.to_string()) {
        return;
    }
    let Some(style) = styles.get(style_id) else {
        return;
    };
    if let Some(parent) = &style.based_on {
        collect_style_declarations(parent, styles, visiting, output);
    }
    if style.kind.as_deref() == Some("character") {
        if let Some(linked) = &style.linked {
            collect_linked_run_declarations(linked, styles, visiting, output);
        }
    }
    if style.kind.as_deref() != Some("character") {
        output.extend(paragraph_css_declarations(&style.paragraph));
    }
    output.extend(run_css_declarations(&style.run));
}

fn collect_linked_run_declarations(
    style_id: &str,
    styles: &BTreeMap<String, WordStyleDefinition>,
    visiting: &mut HashSet<String>,
    output: &mut Vec<String>,
) {
    if !visiting.insert(style_id.to_string()) {
        return;
    }
    let Some(style) = styles.get(style_id) else {
        return;
    };
    if let Some(parent) = &style.based_on {
        collect_linked_run_declarations(parent, styles, visiting, output);
    }
    output.extend(run_css_declarations(&style.run));
}

#[derive(Default)]
struct CellBuilder {
    opened: bool,
    grid_span: Option<u32>,
    vertical_merge: Option<String>,
    top_level: bool,
    read_only: bool,
    css: Vec<String>,
    conditional_style: ConditionalStyleMask,
}

#[derive(Default)]
struct DrawingBuilder {
    ordinal: u64,
    layout: Option<DrawingLayout>,
    width_emu: Option<u64>,
    height_emu: Option<u64>,
    alt: Option<String>,
    drawing_id: Option<u64>,
    horizontal_relative_from: Option<String>,
    vertical_relative_from: Option<String>,
    horizontal_offset_emu: Option<i64>,
    vertical_offset_emu: Option<i64>,
    horizontal_align: Option<String>,
    vertical_align: Option<String>,
    simple_x_emu: Option<i64>,
    simple_y_emu: Option<i64>,
    active_position_axis: Option<DrawingAxis>,
    wrap_kind: Option<String>,
    wrap_side: Option<String>,
    behind_document: Option<bool>,
    layout_in_cell: Option<bool>,
    allow_overlap: Option<bool>,
    relative_height: Option<u64>,
    distance_top_emu: Option<u64>,
    distance_bottom_emu: Option<u64>,
    distance_left_emu: Option<u64>,
    distance_right_emu: Option<u64>,
    is_textbox: bool,
    textbox_content: Vec<RenderedBlock>,
    shape_geometry: Option<String>,
    shape_rotation: Option<i64>,
    shape_fill: Option<String>,
    shape_fill_alpha: Option<u32>,
    gradient_colors: Vec<String>,
    gradient_angle: Option<i64>,
    no_shape_fill: bool,
    line_color: Option<String>,
    line_width_emu: Option<u64>,
    line_dash: Option<String>,
    no_line: bool,
    body_left_inset_emu: Option<u64>,
    body_top_inset_emu: Option<u64>,
    body_right_inset_emu: Option<u64>,
    body_bottom_inset_emu: Option<u64>,
    body_vertical: Option<String>,
    body_anchor: Option<String>,
    has_outer_shadow: bool,
}

#[derive(Clone, Copy)]
enum DrawingColorTarget {
    Shape,
    Line,
}

#[derive(Default)]
struct DrawingPropertyState {
    shape_properties_depth: Option<usize>,
    transform_depth: Option<usize>,
    line_depth: Option<usize>,
    shape_solid_fill_depth: Option<usize>,
    line_solid_fill_depth: Option<usize>,
    gradient_fill_depth: Option<usize>,
    outer_shadow_depth: Option<usize>,
    color_scope: Option<(usize, DrawingColorTarget)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DrawingLayout {
    Inline,
    Anchor,
}

impl DrawingLayout {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Anchor => "anchor",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DrawingAxis {
    Horizontal,
    Vertical,
}

enum DrawingTextKind {
    Offset,
    Align,
}

struct PendingDrawingText {
    axis: DrawingAxis,
    kind: DrawingTextKind,
    value: String,
}

#[derive(Default)]
struct PartRelationships {
    assets: HashMap<String, AssetRecord>,
    hyperlinks: HashMap<String, String>,
}

struct TextPartContext<'a> {
    document_id: &'a str,
    part: &'a str,
    relationships: &'a PartRelationships,
    numbering: &'a NumberingCatalog,
    paragraph_numbering: &'a BTreeMap<String, (String, Option<String>)>,
    theme: &'a WordTheme,
    table_bands: &'a TableBandCatalog,
}

#[derive(Clone, Copy)]
struct DocumentPageLayout {
    width_twips: u64,
    height_twips: u64,
    margin_top_twips: u64,
    margin_right_twips: u64,
    margin_bottom_twips: u64,
    margin_left_twips: u64,
}

impl Default for DocumentPageLayout {
    fn default() -> Self {
        Self {
            width_twips: 12_240,
            height_twips: 15_840,
            margin_top_twips: 1_440,
            margin_right_twips: 1_440,
            margin_bottom_twips: 1_440,
            margin_left_twips: 1_440,
        }
    }
}

impl DocumentPageLayout {
    fn presentation_css(self) -> String {
        format!(
            "body[data-hcd-source-format=\"docx\"]{{width:{:.2}pt;max-width:{:.2}pt;min-width:0;min-height:{:.2}pt;padding:{:.2}pt {:.2}pt {:.2}pt {:.2}pt}}",
            self.width_twips as f64 / 20.0,
            self.width_twips as f64 / 20.0,
            self.height_twips as f64 / 20.0,
            self.margin_top_twips as f64 / 20.0,
            self.margin_right_twips as f64 / 20.0,
            self.margin_bottom_twips as f64 / 20.0,
            self.margin_left_twips as f64 / 20.0,
        )
    }
}

struct TableBuilder {
    table_id: String,
    html: String,
    entries: Vec<NodeMapEntry>,
    rows_in_fragment: usize,
    continuation: bool,
    row_number: u64,
    current_grid_column: u32,
    active_vertical_merges: BTreeMap<u32, ActiveVerticalMerge>,
    style_id: Option<String>,
    look: TableLook,
    table_css: Vec<String>,
    cell_css: Vec<String>,
    row_css: Vec<String>,
    row_exception_cell_css: Vec<String>,
    row_has_property_exceptions: bool,
    row_conditional_style: ConditionalStyleMask,
    row_opened: bool,
    grid_widths_twips: Vec<u64>,
    row_band_size: u32,
    column_band_size: u32,
    row_band_size_direct: bool,
    column_band_size_direct: bool,
}

struct TableLook {
    first_row: bool,
    last_row: bool,
    first_column: bool,
    last_column: bool,
    horizontal_banding: bool,
    vertical_banding: bool,
}

impl Default for TableLook {
    fn default() -> Self {
        Self {
            first_row: false,
            last_row: false,
            first_column: false,
            last_column: false,
            horizontal_banding: true,
            vertical_banding: true,
        }
    }
}

struct ActiveVerticalMerge {
    grid_span: u32,
    rowspan_digits_offset: usize,
    rows: u64,
    last_seen_row: u64,
}

impl TableBuilder {
    fn new(table_id: String) -> Self {
        let mut table = Self {
            html: String::new(),
            table_id,
            entries: Vec::new(),
            rows_in_fragment: 0,
            continuation: false,
            row_number: 0,
            current_grid_column: 0,
            active_vertical_merges: BTreeMap::new(),
            style_id: None,
            look: TableLook::default(),
            table_css: Vec::new(),
            cell_css: Vec::new(),
            row_css: Vec::new(),
            row_exception_cell_css: Vec::new(),
            row_has_property_exceptions: false,
            row_conditional_style: ConditionalStyleMask::default(),
            row_opened: false,
            grid_widths_twips: Vec::new(),
            row_band_size: 1,
            column_band_size: 1,
            row_band_size_direct: false,
            column_band_size_direct: false,
        };
        table.rebuild_opening(false);
        table
    }

    fn capture_style(&mut self, element: &BytesStart<'_>, table_bands: &TableBandCatalog) {
        self.style_id = attribute_by_local_name(element, "val")
            .filter(|value| !value.is_empty() && value.len() <= 256);
        if !self.row_band_size_direct {
            self.row_band_size = 1;
        }
        if !self.column_band_size_direct {
            self.column_band_size = 1;
        }
        if let Some(sizes) = self
            .style_id
            .as_deref()
            .and_then(|style_id| table_bands.get(style_id))
        {
            if !self.row_band_size_direct {
                self.row_band_size = sizes.row;
            }
            if !self.column_band_size_direct {
                self.column_band_size = sizes.column;
            }
        }
        self.refresh_opening();
    }

    fn capture_look(&mut self, element: &BytesStart<'_>) {
        if let Some(value) = attribute_by_local_name(element, "val")
            .and_then(|value| u16::from_str_radix(&value, 16).ok())
        {
            self.look.first_row = value & 0x0020 != 0;
            self.look.last_row = value & 0x0040 != 0;
            self.look.first_column = value & 0x0080 != 0;
            self.look.last_column = value & 0x0100 != 0;
            self.look.horizontal_banding = value & 0x0200 == 0;
            self.look.vertical_banding = value & 0x0400 == 0;
        }
        update_table_look_flag(element, "firstRow", &mut self.look.first_row, false);
        update_table_look_flag(element, "lastRow", &mut self.look.last_row, false);
        update_table_look_flag(element, "firstColumn", &mut self.look.first_column, false);
        update_table_look_flag(element, "lastColumn", &mut self.look.last_column, false);
        update_table_look_flag(element, "noHBand", &mut self.look.horizontal_banding, true);
        update_table_look_flag(element, "noVBand", &mut self.look.vertical_banding, true);
        self.refresh_opening();
    }

    fn capture_grid_column(&mut self, element: &BytesStart<'_>) -> Result<(), HcdError> {
        if self.grid_widths_twips.len() >= MAX_TABLE_GRID_COLUMNS {
            return Err(HcdError::ResourceLimit(format!(
                "table grid exceeds {MAX_TABLE_GRID_COLUMNS} columns"
            )));
        }
        let width = attribute_by_local_name(element, "w")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value <= 2_000_000)
            .unwrap_or(0);
        self.grid_widths_twips.push(width);
        Ok(())
    }

    fn refresh_opening(&mut self) {
        if self.rows_in_fragment == 0 && self.entries.is_empty() {
            self.rebuild_opening(self.continuation);
        }
    }

    fn rebuild_opening(&mut self, continuation: bool) {
        let style_class = self
            .style_id
            .as_deref()
            .map(|style_id| format!(" {}", word_style_class(style_id)))
            .unwrap_or_default();
        self.html = format!(
            "<table class=\"hcd-table{style_class}\" data-hcd-id=\"{}\"",
            escape_attribute(&self.table_id)
        );
        push_data_attribute(
            &mut self.html,
            "data-hcd-table-style",
            self.style_id.as_deref(),
        );
        push_data_bool(
            &mut self.html,
            "data-hcd-look-first-row",
            Some(self.look.first_row && !continuation),
        );
        push_data_bool(&mut self.html, "data-hcd-look-last-row", Some(false));
        push_data_bool(
            &mut self.html,
            "data-hcd-look-first-column",
            Some(self.look.first_column),
        );
        push_data_bool(
            &mut self.html,
            "data-hcd-look-last-column",
            Some(self.look.last_column),
        );
        push_data_bool(
            &mut self.html,
            "data-hcd-look-h-band",
            Some(self.look.horizontal_banding),
        );
        push_data_bool(
            &mut self.html,
            "data-hcd-look-v-band",
            Some(self.look.vertical_banding),
        );
        push_data_number(
            &mut self.html,
            "data-hcd-row-band-size",
            Some(self.row_band_size),
        );
        push_data_number(
            &mut self.html,
            "data-hcd-column-band-size",
            Some(self.column_band_size),
        );
        push_inline_style_attribute(&mut self.html, &self.table_css);
        if continuation {
            self.html.push_str(" data-hcd-continuation=\"true\"");
        }
        self.html.push('>');
        if !self.grid_widths_twips.is_empty() {
            self.html.push_str("<colgroup>");
            for width in &self.grid_widths_twips {
                self.html.push_str("<col");
                if *width > 0 {
                    push_inline_style_attribute(
                        &mut self.html,
                        &[format!("width:{:.2}pt", *width as f64 / 20.0)],
                    );
                }
                self.html.push_str("/>");
            }
            self.html.push_str("</colgroup>");
        }
        self.html.push_str("<tbody>");
    }

    fn begin_row(&mut self) {
        if self.row_number == 0 {
            self.refresh_opening();
        }
        self.row_number = self.row_number.saturating_add(1);
        self.current_grid_column = 0;
        self.row_css.clear();
        self.row_exception_cell_css.clear();
        self.row_has_property_exceptions = false;
        self.row_conditional_style = ConditionalStyleMask::default();
        self.row_opened = false;
        self.html.push_str("<tr");
        push_data_number(
            &mut self.html,
            "data-hcd-row-band",
            Some(table_band_index(
                self.row_number.saturating_sub(1),
                self.row_band_size,
            )),
        );
    }

    fn ensure_row_open(&mut self) {
        if self.row_opened {
            return;
        }
        push_data_bool(
            &mut self.html,
            "data-hcd-table-property-exception",
            self.row_has_property_exceptions.then_some(true),
        );
        push_conditional_style_attributes(&mut self.html, self.row_conditional_style);
        push_inline_style_attribute(&mut self.html, &self.row_css);
        self.html.push('>');
        self.row_opened = true;
    }

    fn open_cell(&mut self, cell: &mut CellBuilder) {
        if !cell.top_level {
            self.write_cell_start(cell, false, cell.grid_span, None);
            return;
        }
        let column = self.current_grid_column;
        let grid_span = cell.grid_span.unwrap_or(1);
        self.current_grid_column = self.current_grid_column.saturating_add(grid_span);
        match cell.vertical_merge.as_deref() {
            Some("restart") => {
                self.active_vertical_merges.remove(&column);
                self.html.push_str("<td");
                self.append_column_band(column);
                if grid_span > 1 {
                    self.html.push_str(&format!(" colspan=\"{grid_span}\""));
                }
                self.html
                    .push_str(" data-hcd-v-merge=\"restart\" rowspan=\"");
                let rowspan_digits_offset = self.html.len();
                self.html.push_str("0000000001\"");
                push_conditional_style_attributes(&mut self.html, cell.conditional_style);
                self.append_cell_style(cell, false);
                self.html.push('>');
                self.active_vertical_merges.insert(
                    column,
                    ActiveVerticalMerge {
                        grid_span,
                        rowspan_digits_offset,
                        rows: 1,
                        last_seen_row: self.row_number,
                    },
                );
            }
            Some("continue") => {
                if let Some(active) = self.active_vertical_merges.get_mut(&column) {
                    if active.grid_span == grid_span {
                        active.rows = active.rows.saturating_add(1);
                        active.last_seen_row = self.row_number;
                        let digits = format!("{:010}", active.rows.min(9_999_999_999));
                        self.html.replace_range(
                            active.rowspan_digits_offset..active.rowspan_digits_offset + 10,
                            &digits,
                        );
                        cell.read_only = true;
                        self.write_cell_start(cell, true, Some(grid_span), Some(column));
                        return;
                    }
                }
                self.write_cell_start(cell, false, Some(grid_span), Some(column));
            }
            _ => self.write_cell_start(cell, false, Some(grid_span), Some(column)),
        }
    }

    fn write_cell_start(
        &mut self,
        cell: &CellBuilder,
        hidden_continuation: bool,
        grid_span: Option<u32>,
        logical_column: Option<u32>,
    ) {
        self.html.push_str("<td");
        if let Some(column) = logical_column {
            self.append_column_band(column);
        }
        if let Some(span) = grid_span.filter(|span| *span > 1) {
            self.html.push_str(&format!(" colspan=\"{span}\""));
        }
        push_data_attribute(
            &mut self.html,
            "data-hcd-v-merge",
            cell.vertical_merge.as_deref(),
        );
        push_conditional_style_attributes(&mut self.html, cell.conditional_style);
        if hidden_continuation {
            self.html.push_str(" data-hcd-editable=\"false\"");
        }
        self.append_cell_style(cell, hidden_continuation);
        self.html.push('>');
    }

    fn append_column_band(&mut self, logical_column: u32) {
        push_data_number(
            &mut self.html,
            "data-hcd-column-band",
            Some(table_band_index(
                u64::from(logical_column),
                self.column_band_size,
            )),
        );
    }

    fn append_cell_style(&mut self, cell: &CellBuilder, hidden: bool) {
        let mut css = Vec::with_capacity(
            self.cell_css.len()
                + self.row_exception_cell_css.len()
                + cell.css.len()
                + usize::from(hidden),
        );
        if cell.top_level {
            css.extend(self.cell_css.iter().cloned());
            css.extend(self.row_exception_cell_css.iter().cloned());
        }
        css.extend(cell.css.iter().cloned());
        if hidden {
            css.push("display:none".to_string());
        }
        push_inline_style_attribute(&mut self.html, &css);
    }

    fn finish_row(&mut self) -> Result<(), HcdError> {
        let row = self.row_number;
        self.active_vertical_merges
            .retain(|_, merge| merge.last_seen_row == row);
        if !self.active_vertical_merges.is_empty() && self.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: vertically merged row group is {} bytes",
                self.html.len()
            )));
        }
        Ok(())
    }

    fn finish_table_merges(&mut self) {
        self.active_vertical_merges.clear();
    }

    fn mark_final_fragment(&mut self) {
        if self.look.last_row {
            self.html = self.html.replacen(
                "data-hcd-look-last-row=\"false\"",
                "data-hcd-look-last-row=\"true\"",
                1,
            );
        }
    }

    fn has_active_vertical_merges(&self) -> bool {
        !self.active_vertical_merges.is_empty()
    }

    fn reset_for_continuation(&mut self) {
        self.rebuild_opening(true);
        self.entries.clear();
        self.rows_in_fragment = 0;
        self.continuation = true;
        self.current_grid_column = 0;
        debug_assert!(self.active_vertical_merges.is_empty());
    }
}

fn update_table_look_flag(
    element: &BytesStart<'_>,
    attribute: &str,
    target: &mut bool,
    invert: bool,
) {
    if let Some(value) = attribute_by_local_name(element, attribute) {
        let enabled = on_off_value(Some(&value));
        *target = if invert { !enabled } else { enabled };
    }
}

fn capture_conditional_style(element: &BytesStart<'_>, target: &mut ConditionalStyleMask) {
    target.present = true;
    if let Some(value) = attribute_by_local_name(element, "val") {
        let bytes = value.as_bytes();
        if bytes.len() == CONDITIONAL_STYLE_FLAGS.len()
            && bytes.iter().all(|byte| matches!(byte, b'0' | b'1'))
        {
            target.specified = (1u16 << CONDITIONAL_STYLE_FLAGS.len()) - 1;
            target.enabled = 0;
            for (index, byte) in bytes.iter().enumerate() {
                if *byte == b'1' {
                    target.enabled |= 1u16 << index;
                }
            }
        }
    }
    for (index, flag) in CONDITIONAL_STYLE_FLAGS.iter().enumerate() {
        let Some(value) = attribute_by_local_name(element, flag.ooxml_attribute) else {
            continue;
        };
        let bit = 1u16 << index;
        target.specified |= bit;
        if on_off_value(Some(&value)) {
            target.enabled |= bit;
        } else {
            target.enabled &= !bit;
        }
    }
}

fn push_conditional_style_attributes(output: &mut String, mask: ConditionalStyleMask) {
    if !mask.present {
        return;
    }
    push_data_bool(output, "data-hcd-cnf-present", Some(true));
    for (index, flag) in CONDITIONAL_STYLE_FLAGS.iter().enumerate() {
        let bit = 1u16 << index;
        if mask.specified & bit != 0 {
            push_data_bool(output, flag.data_attribute, Some(mask.enabled & bit != 0));
        }
    }
}

fn conditional_style_data_attribute(condition: &str) -> Option<&'static str> {
    CONDITIONAL_STYLE_FLAGS
        .iter()
        .find(|flag| flag.condition == condition)
        .map(|flag| flag.data_attribute)
}

fn table_band_index(zero_based_position: u64, band_size: u32) -> u8 {
    let band_size = u64::from(band_size.max(1));
    ((zero_based_position / band_size) % 2 + 1) as u8
}

struct ChunkAccumulator<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    document_id: &'a str,
    part: &'a str,
    region: &'a str,
    soft_bytes: usize,
    max_blocks: usize,
    chunk_ordinal: usize,
    html: String,
    entries: Vec<NodeMapEntry>,
    block_count: usize,
    continuation: bool,
    requires_overflow_visible: bool,
    writer: &'a mut BundleWriter,
    emit: &'a mut F,
}

impl<'a, F> ChunkAccumulator<'a, F>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    fn new(
        document_id: &'a str,
        part: &'a str,
        region: &'a str,
        options: &ImportOptions,
        writer: &'a mut BundleWriter,
        emit: &'a mut F,
    ) -> Self {
        Self {
            document_id,
            part,
            region,
            soft_bytes: options.chunk_soft_bytes.min(MAX_CHUNK_BYTES),
            max_blocks: options.chunk_blocks.clamp(1, DEFAULT_CHUNK_BLOCKS),
            chunk_ordinal: 0,
            html: String::new(),
            entries: Vec::new(),
            block_count: 0,
            continuation: false,
            requires_overflow_visible: false,
            writer,
            emit,
        }
    }

    fn push_block(&mut self, block: RenderedBlock, continuation: bool) -> Result<(), HcdError> {
        if block.html.len() > MAX_CHUNK_BYTES {
            return Err(HcdError::ResourceLimit(format!(
                "NODE_TOO_LARGE: block in {} is {} bytes",
                self.part,
                block.html.len()
            )));
        }
        if !self.html.is_empty()
            && (self.html.len() + block.html.len() > self.soft_bytes
                || self.block_count >= self.max_blocks)
        {
            self.flush()?;
        }
        self.html.push_str(&block.html);
        self.entries.extend(block.entries);
        self.block_count += 1;
        self.continuation |= continuation;
        self.requires_overflow_visible |= block.requires_overflow_visible;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), HcdError> {
        if self.block_count == 0 {
            return Ok(());
        }
        let chunk_seed = format!("{}:{}", self.part, self.chunk_ordinal);
        let chunk_id =
            stable_node_id(&[self.document_id, &chunk_seed, "chunk"]).replacen("n_", "c_", 1);
        let overflow_class = if self.requires_overflow_visible {
            " hcd-chunk-overflow"
        } else {
            ""
        };
        let html = format!(
            "<section class=\"hcd-chunk{overflow_class}\" data-hcd-chunk-id=\"{}\" data-hcd-region=\"{}\">{}</section>",
            escape_attribute(&chunk_id),
            escape_attribute(self.region),
            self.html
        );
        let source_map = ChunkSourceMap {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            chunk_id: chunk_id.clone(),
            entries: std::mem::take(&mut self.entries),
        };
        let descriptor = self.writer.write_chunk(
            chunk_id,
            self.region.to_string(),
            html,
            source_map,
            self.block_count,
            self.continuation,
        )?;
        (self.emit)(&ImportEvent::ChunkReady { descriptor })?;
        self.chunk_ordinal += 1;
        self.html.clear();
        self.block_count = 0;
        self.continuation = false;
        self.requires_overflow_visible = false;
        Ok(())
    }
}

pub fn import_docx<F>(
    source: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: &ImportOptions,
    mut emit: F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let source = source.as_ref();
    let output = output.as_ref();
    let source_metadata = std::fs::metadata(source)?;
    if source_metadata.len() > MAX_SOURCE_BYTES {
        return Err(HcdError::ResourceLimit(format!(
            "compressed DOCX is {} bytes; maximum is {MAX_SOURCE_BYTES}",
            source_metadata.len()
        )));
    }
    if options.document_id.trim().is_empty() || options.document_id.len() > 256 {
        return Err(HcdError::InvalidBundle(
            "documentId must contain between 1 and 256 bytes".to_string(),
        ));
    }
    let source_hash = hash_file(source)?;
    emit(&ImportEvent::ImportStarted {
        document_id: options.document_id.clone(),
        source_sha256: source_hash.clone(),
    })?;

    let result = import_docx_inner(
        source,
        output,
        options,
        &source_hash,
        source_metadata.len(),
        &mut emit,
    );
    if let Err(error) = &result {
        let _ = emit(&ImportEvent::Failed {
            document_id: options.document_id.clone(),
            error: error.to_string(),
        });
    }
    result
}

fn import_docx_inner<F>(
    source: &Path,
    output: &Path,
    options: &ImportOptions,
    source_hash: &str,
    source_size: u64,
    emit: &mut F,
) -> Result<HcdManifest, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let mut archive = StreamingOxmlArchive::open(source).map_err(package_error)?;
    if !archive.contains("word/document.xml") {
        return Err(HcdError::InvalidBundle(
            "DOCX is missing word/document.xml".to_string(),
        ));
    }
    let theme = load_word_theme(&mut archive)?;
    let mut rendered_styles = load_word_styles(&mut archive, &theme)?;
    let page_layout = load_document_page_layout(&mut archive)?;
    rendered_styles
        .css
        .push_str(&page_layout.presentation_css());
    hcd_core::validate_css_text(&rendered_styles.css)?;
    let numbering = load_word_numbering(&mut archive)?;
    let mut writer = BundleWriter::create(output)?;
    writer.write_styles(&rendered_styles.css)?;

    let text_parts = ordered_text_parts(&archive);
    let referenced_media = referenced_media_parts(&mut archive, &text_parts)?;
    let (referenced_parts, deferred_parts): (Vec<_>, Vec<_>) = archive
        .entries()
        .iter()
        .filter(|entry| !entry.is_dir && entry.name.starts_with("word/media/"))
        .map(|entry| entry.name.clone())
        .partition(|part| referenced_media.contains(part));
    let mut assets = import_assets(&mut archive, &writer, emit, referenced_parts)?;
    let asset_by_part: HashMap<String, AssetRecord> = assets
        .iter()
        .cloned()
        .map(|asset| (asset.source_part.clone(), asset))
        .collect();

    let mut warnings = Vec::new();
    for (part, region) in text_parts {
        let relationships = load_relationships(&mut archive, &part, &asset_by_part)?;
        let mut accumulator = ChunkAccumulator::new(
            &options.document_id,
            &part,
            &region,
            options,
            &mut writer,
            emit,
        );
        archive
            .with_part(&part, |reader| {
                let context = TextPartContext {
                    document_id: &options.document_id,
                    part: &part,
                    relationships: &relationships,
                    numbering: &numbering,
                    paragraph_numbering: &rendered_styles.paragraph_numbering,
                    theme: &theme,
                    table_bands: &rendered_styles.table_bands,
                };
                parse_text_part(reader, &context, &mut accumulator, &mut warnings)
                    .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        accumulator.flush()?;
    }
    assets.extend(import_assets(&mut archive, &writer, emit, deferred_parts)?);
    write_asset_index(writer.root(), &assets)?;

    let manifest = HcdManifest {
        schema_version: HCD_SCHEMA_VERSION.to_string(),
        document_id: options.document_id.clone(),
        profile: "semantic-flow".to_string(),
        revision: 0,
        source: SourceDescriptor {
            format: "docx".to_string(),
            sha256: source_hash.to_string(),
            size_bytes: source_size,
        },
        root_hash: String::new(),
        annotation_root_hash: String::new(),
        annotation_href: None,
        index_prefix: String::new(),
        index_page_count: 0,
        chunk_count: 0,
        styles_href: "styles.css".to_string(),
        capabilities: HcdCapabilities::default(),
        fidelity: Some(FidelityReport {
            schema_version: HCD_SCHEMA_VERSION.to_string(),
            level: FidelityLevel::High,
            preserved: vec![
                "editable text with stable source anchors".to_string(),
                "direct paragraph, run, table, row and cell formatting in canonical HTML"
                    .to_string(),
                "common Word theme fonts, theme colors, tint/shade and basedOn style inheritance"
                    .to_string(),
                "linked character styles and common basedOn table styles with inherited/direct band sizes, Word cascade order, tblLook gating and cnfStyle row, cell and paragraph conditions"
                    .to_string(),
                "common row-level tblPrEx border, shading and cell-margin exceptions with historical snapshots ignored"
                    .to_string(),
                "language-selected East Asian and bidirectional theme font slots".to_string(),
                "common decimal, decimal-zero, letter, Roman and bullet list markers with level formatting, indentation, style inheritance and source continuation semantics"
                    .to_string(),
                "tables including bounded vertical merge row groups, hyperlinks, inline/anchored image geometry and document regions"
                    .to_string(),
                "common DrawingML textbox geometry, independent shape grouping, nested tables, fill, border, rotation, vertical text, body insets and z-order"
                    .to_string(),
                "tracked insert/delete/move semantics with historical property snapshots kept read-only"
                    .to_string(),
                "opaque OOXML parts in the immutable source".to_string(),
            ],
            flattened: vec![
                "Word physical pagination is represented as semantic flow".to_string(),
                "floating DrawingML offsets, wrap modes and z-order are materialized, while Word page collision, custom geometry, exact gradient/effect parameters and custom wrap polygons remain best-effort"
                    .to_string(),
                "unrecognized language scripts, diagonal borders, tblPrEx width/alignment/spacing/indent/layout/look exceptions and legacy Word table-style compatibility modes are best-effort"
                    .to_string(),
                "locale-specific numbering formats and advanced level restart rules are best-effort"
                    .to_string(),
            ],
            dropped: Vec::new(),
            warnings: warnings.clone(),
        }),
        state: "IMPORTING".to_string(),
        warnings,
    };
    let manifest = writer.finish(manifest)?;
    let _ = emit(&ImportEvent::Completed {
        manifest: manifest.clone(),
    });
    Ok(manifest)
}

fn load_document_page_layout(
    archive: &mut StreamingOxmlArchive,
) -> Result<DocumentPageLayout, HcdError> {
    archive
        .with_part("word/document.xml", |source| {
            let mut layout = DocumentPageLayout::default();
            let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
            reader.config_mut().check_end_names = true;
            let mut buffer = Vec::with_capacity(8 * 1024);
            let mut elements = 0usize;
            loop {
                let event = reader.read_event_into(&mut buffer).map_err(|error| {
                    PackageError::ReadPartError(format!(
                        "XML error while reading Word page layout: {error}"
                    ))
                })?;
                match event {
                    Event::Start(ref element) | Event::Empty(ref element) => {
                        elements += 1;
                        if elements > MAX_XML_ELEMENTS {
                            return Err(PackageError::ResourceLimit(
                                "word/document.xml exceeds XML safety limits while reading page layout"
                                    .to_string(),
                            ));
                        }
                        match local_name(element.name().as_ref()) {
                            "pgSz" => {
                                layout.width_twips = bounded_twips_attribute(
                                    element,
                                    "w",
                                    layout.width_twips,
                                );
                                layout.height_twips = bounded_twips_attribute(
                                    element,
                                    "h",
                                    layout.height_twips,
                                );
                            }
                            "pgMar" => {
                                layout.margin_top_twips = bounded_twips_attribute(
                                    element,
                                    "top",
                                    layout.margin_top_twips,
                                );
                                layout.margin_right_twips = bounded_twips_attribute(
                                    element,
                                    "right",
                                    layout.margin_right_twips,
                                );
                                layout.margin_bottom_twips = bounded_twips_attribute(
                                    element,
                                    "bottom",
                                    layout.margin_bottom_twips,
                                );
                                layout.margin_left_twips = bounded_twips_attribute(
                                    element,
                                    "left",
                                    layout.margin_left_twips,
                                );
                            }
                            _ => {}
                        }
                    }
                    Event::Eof => break,
                    _ => {}
                }
                buffer.clear();
            }
            Ok(layout)
        })
        .map_err(package_error)
}

fn bounded_twips_attribute(element: &BytesStart<'_>, name: &str, fallback: u64) -> u64 {
    attribute_by_local_name(element, name)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=200_000).contains(value))
        .unwrap_or(fallback)
}

fn import_assets<F>(
    archive: &mut StreamingOxmlArchive,
    writer: &BundleWriter,
    emit: &mut F,
    media_parts: Vec<String>,
) -> Result<Vec<AssetRecord>, HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let mut assets = Vec::with_capacity(media_parts.len());
    for part in media_parts {
        let extension = Path::new(&part)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        let (href, hash, byte_length) = archive
            .with_part(&part, |reader| {
                writer
                    .write_asset_from_reader(extension, reader)
                    .map_err(|error| PackageError::ReadPartError(error.to_string()))
            })
            .map_err(package_error)?;
        emit(&ImportEvent::AssetReady {
            hash: hash.clone(),
            href: href.clone(),
            byte_length,
        })?;
        assets.push(AssetRecord {
            source_part: part,
            hash,
            href,
            byte_length,
        });
    }
    Ok(assets)
}

fn parse_text_part<F>(
    source: &mut dyn Read,
    context: &TextPartContext<'_>,
    chunks: &mut ChunkAccumulator<'_, F>,
    warnings: &mut Vec<FidelityWarning>,
) -> Result<(), HcdError>
where
    F: FnMut(&ImportEvent) -> Result<(), HcdError>,
{
    let document_id = context.document_id;
    let part = context.part;
    let relationships = context.relationships;
    let numbering = context.numbering;
    let theme = context.theme;
    let table_bands = context.table_bands;
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, source));
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut paragraphs: Vec<ParagraphBuilder> = Vec::new();
    let mut runs: Vec<RunBuilder> = Vec::new();
    let mut cells: Vec<CellBuilder> = Vec::new();
    let mut drawings: Vec<DrawingBuilder> = Vec::new();
    let mut drawing_properties = DrawingPropertyState::default();
    let mut drawing_text: Option<PendingDrawingText> = None;
    let mut alternate_content_support: Vec<bool> = Vec::new();
    let mut skipped_fallback_depth = None;
    let mut hyperlink_tags: Vec<bool> = Vec::new();
    let mut fields: Vec<FieldFrame> = Vec::new();
    let mut current_text: Option<PendingText> = None;
    let mut revisions: Vec<RevisionFrame> = Vec::new();
    let mut property_change_depth = None;
    let mut paragraph_properties_depth = None;
    let mut run_properties_depth = None;
    let mut cell_properties_depth = None;
    let mut table_properties_depth = None;
    let mut table_property_exceptions_depth = None;
    let mut row_properties_depth = None;
    let mut table_borders_depth = None;
    let mut table_cell_margins_depth = None;
    let mut table_exception_borders_depth = None;
    let mut table_exception_cell_margins_depth = None;
    let mut table_grid_depth = None;
    let mut cell_borders_depth = None;
    let mut cell_margins_depth = None;
    let mut text_ordinal = 0u64;
    let mut image_ordinal = 0u64;
    let mut drawing_ordinal = 0u64;
    let mut paragraph_ordinal = 0u64;
    let mut table_ordinal = 0u64;
    let mut table_depth = 0usize;
    let mut table: Option<TableBuilder> = None;
    let mut element_count = 0usize;
    let mut xml_depth = 0usize;
    let mut numbering_state = NumberingState::default();

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| HcdError::InvalidBundle(format!("XML error in {part}: {error}")))?;
        match event {
            Event::Start(ref start) => {
                element_count += 1;
                if element_count > MAX_XML_ELEMENTS {
                    return Err(HcdError::ResourceLimit(format!(
                        "{part} exceeds {MAX_XML_ELEMENTS} XML elements"
                    )));
                }
                xml_depth += 1;
                if xml_depth > MAX_XML_DEPTH {
                    return Err(HcdError::ResourceLimit(format!(
                        "{part} exceeds the maximum XML depth {MAX_XML_DEPTH}"
                    )));
                }
                let qualified_name = start.name();
                let name = local_name(qualified_name.as_ref());
                if skipped_fallback_depth.is_some() {
                    buffer.clear();
                    continue;
                }
                if name == "AlternateContent" {
                    alternate_content_support.push(false);
                } else if name == "Choice" {
                    if attribute_by_local_name(start, "Requires").is_some_and(|requires| {
                        requires
                            .split_ascii_whitespace()
                            .any(|value| value == "wps")
                    }) {
                        if let Some(supported) = alternate_content_support.last_mut() {
                            *supported = true;
                        }
                    }
                } else if name == "Fallback"
                    && alternate_content_support.last().copied() == Some(true)
                {
                    skipped_fallback_depth = Some(xml_depth);
                    buffer.clear();
                    continue;
                }
                if matches!(
                    name,
                    "pPrChange"
                        | "rPrChange"
                        | "tblPrChange"
                        | "tblPrExChange"
                        | "trPrChange"
                        | "tcPrChange"
                ) {
                    property_change_depth = Some(xml_depth);
                } else if property_change_depth.is_none() {
                    if let Some(kind) = RevisionKind::from_ooxml(name) {
                        revisions.push(open_revision(start, kind, paragraphs.last_mut()));
                    }
                    match name {
                        "tbl" => {
                            ensure_current_cell_open(table.as_mut(), cells.last_mut());
                            table_depth += 1;
                            if table_depth == 1 {
                                table_ordinal += 1;
                                let table_id = stable_node_id(&[
                                    document_id,
                                    part,
                                    "table",
                                    &table_ordinal.to_string(),
                                ]);
                                table = Some(TableBuilder::new(table_id));
                            } else if let Some(table) = &mut table {
                                table
                                    .html
                                    .push_str("<table class=\"hcd-nested-table\"><tbody>");
                            }
                        }
                        "tr" if table_depth > 0 => {
                            let table_is_in_textbox =
                                drawings.last().is_some_and(|drawing| drawing.is_textbox);
                            if let Some(table) = &mut table {
                                if table_depth == 1 {
                                    if table.rows_in_fragment > 0
                                        && !table_is_in_textbox
                                        && !table.has_active_vertical_merges()
                                        && (table.rows_in_fragment >= TABLE_ROWS_PER_FRAGMENT
                                            || table.html.len() >= chunks.soft_bytes)
                                    {
                                        let block = take_table_fragment(table);
                                        let continuation = table.continuation;
                                        chunks.push_block(block, continuation)?;
                                        table.reset_for_continuation();
                                    }
                                    table.begin_row();
                                } else {
                                    table.html.push_str("<tr>");
                                }
                            }
                        }
                        "trPr" if table_depth == 1 => {
                            row_properties_depth = Some(xml_depth);
                        }
                        "tblPrEx" if table_depth == 1 => {
                            table_property_exceptions_depth = Some(xml_depth);
                            if let Some(table) = &mut table {
                                table.row_has_property_exceptions = true;
                            }
                        }
                        "tblPr" if table_depth == 1 => {
                            table_properties_depth = Some(xml_depth);
                        }
                        "tblGrid" if table_depth == 1 => {
                            table_grid_depth = Some(xml_depth);
                        }
                        "gridCol" if table_grid_depth.is_some() => {
                            if let Some(table) = &mut table {
                                table.capture_grid_column(start)?;
                            }
                        }
                        "tblBorders" if table_property_exceptions_depth.is_some() => {
                            table_exception_borders_depth = Some(xml_depth);
                        }
                        "tblCellMar" if table_property_exceptions_depth.is_some() => {
                            table_exception_cell_margins_depth = Some(xml_depth);
                        }
                        "tblBorders" if table_properties_depth.is_some() => {
                            table_borders_depth = Some(xml_depth);
                        }
                        "tblCellMar" if table_properties_depth.is_some() => {
                            table_cell_margins_depth = Some(xml_depth);
                        }
                        "tc" if table_depth > 0 => {
                            if table_depth == 1 {
                                if let Some(table) = &mut table {
                                    table.ensure_row_open();
                                }
                            }
                            cells.push(CellBuilder {
                                top_level: table_depth == 1,
                                ..Default::default()
                            });
                        }
                        "tcPr" if table_depth > 0 => {
                            cell_properties_depth = Some(xml_depth);
                        }
                        "tcBorders" if cell_properties_depth.is_some() => {
                            cell_borders_depth = Some(xml_depth);
                        }
                        "tcMar" if cell_properties_depth.is_some() => {
                            cell_margins_depth = Some(xml_depth);
                        }
                        "p" => {
                            ensure_current_cell_open(table.as_mut(), cells.last_mut());
                            paragraph_ordinal += 1;
                            paragraphs.push(ParagraphBuilder {
                                paragraph_id: attribute_by_local_name(start, "paraId"),
                                ordinal: paragraph_ordinal,
                                ..Default::default()
                            });
                        }
                        "pPr" if !paragraphs.is_empty() => {
                            paragraph_properties_depth = Some(xml_depth);
                        }
                        "r" => {
                            if let Some(paragraph) = paragraphs.last_mut() {
                                paragraph.run_ordinal += 1;
                                runs.push(RunBuilder {
                                    text_id: attribute_by_local_name(start, "textId"),
                                    ordinal: paragraph.run_ordinal,
                                    ..Default::default()
                                });
                            }
                        }
                        "rPr" if !runs.is_empty() && paragraph_properties_depth.is_none() => {
                            run_properties_depth = Some(xml_depth);
                        }
                        "hyperlink" => {
                            let is_anchor =
                                append_hyperlink_start(start, relationships, paragraphs.last_mut());
                            hyperlink_tags.push(is_anchor);
                        }
                        "fldSimple" => {
                            let instruction =
                                attribute_by_local_name(start, "instr").unwrap_or_default();
                            let wrapper_is_anchor =
                                append_field_hyperlink_start(&instruction, paragraphs.last_mut());
                            fields.push(FieldFrame {
                                instruction,
                                wrapper_is_anchor,
                                simple: true,
                            });
                        }
                        "drawing" => {
                            drawing_ordinal += 1;
                            drawings.push(DrawingBuilder {
                                ordinal: drawing_ordinal,
                                ..Default::default()
                            });
                            drawing_properties = DrawingPropertyState::default();
                        }
                        "anchor" | "inline" if !drawings.is_empty() => {
                            capture_drawing_layout(start, drawings.last_mut());
                        }
                        "extent" if !drawings.is_empty() => {
                            capture_drawing_extent(start, drawings.last_mut());
                        }
                        "docPr" if !drawings.is_empty() => {
                            capture_drawing_alt(start, drawings.last_mut());
                        }
                        "simplePos" if !drawings.is_empty() => {
                            capture_drawing_simple_position(start, drawings.last_mut());
                        }
                        "positionH" | "positionV" if !drawings.is_empty() => {
                            begin_drawing_position(start, drawings.last_mut());
                        }
                        "posOffset" | "align" if !drawings.is_empty() => {
                            drawing_text = begin_drawing_text(name, drawings.last());
                        }
                        "wrapSquare" | "wrapTight" | "wrapThrough" | "wrapTopAndBottom"
                        | "wrapNone"
                            if !drawings.is_empty() =>
                        {
                            capture_drawing_wrap(start, drawings.last_mut());
                        }
                        "spPr" if !drawings.is_empty() => {
                            drawing_properties.shape_properties_depth = Some(xml_depth);
                        }
                        "xfrm" if drawing_properties.shape_properties_depth.is_some() => {
                            drawing_properties.transform_depth = Some(xml_depth);
                            capture_textbox_transform(start, drawings.last_mut());
                        }
                        "prstGeom" if drawing_properties.shape_properties_depth.is_some() => {
                            capture_textbox_geometry(start, drawings.last_mut());
                        }
                        "ln" if drawing_properties.shape_properties_depth.is_some() => {
                            drawing_properties.line_depth = Some(xml_depth);
                            capture_textbox_line(start, drawings.last_mut());
                        }
                        "solidFill" if drawing_properties.line_depth.is_some() => {
                            drawing_properties.line_solid_fill_depth = Some(xml_depth);
                        }
                        "solidFill" if drawing_properties.shape_properties_depth.is_some() => {
                            drawing_properties.shape_solid_fill_depth = Some(xml_depth);
                        }
                        "gradFill" if drawing_properties.shape_properties_depth.is_some() => {
                            drawing_properties.gradient_fill_depth = Some(xml_depth);
                        }
                        "outerShdw" if drawing_properties.shape_properties_depth.is_some() => {
                            drawing_properties.outer_shadow_depth = Some(xml_depth);
                            if let Some(drawing) = drawings.last_mut() {
                                drawing.has_outer_shadow = true;
                            }
                        }
                        "srgbClr" | "schemeClr" if !drawings.is_empty() => {
                            if let Some(target) = capture_textbox_color(
                                start,
                                drawings.last_mut(),
                                &drawing_properties,
                                theme,
                            ) {
                                drawing_properties.color_scope = Some((xml_depth, target));
                            }
                        }
                        "alpha" if !drawings.is_empty() => {
                            capture_textbox_alpha(
                                start,
                                drawings.last_mut(),
                                drawing_properties.color_scope.map(|(_, target)| target),
                            );
                        }
                        "lin" if drawing_properties.gradient_fill_depth.is_some() => {
                            capture_textbox_gradient_angle(start, drawings.last_mut());
                        }
                        "prstDash" if drawing_properties.line_depth.is_some() => {
                            capture_textbox_line_dash(start, drawings.last_mut());
                        }
                        "noFill" if drawing_properties.line_depth.is_some() => {
                            if let Some(drawing) = drawings.last_mut() {
                                drawing.no_line = true;
                            }
                        }
                        "noFill" if drawing_properties.shape_properties_depth.is_some() => {
                            if let Some(drawing) = drawings.last_mut() {
                                drawing.no_shape_fill = true;
                            }
                        }
                        "txbx" | "txbxContent" if !drawings.is_empty() => {
                            if let Some(drawing) = drawings.last_mut() {
                                drawing.is_textbox = true;
                            }
                        }
                        "bodyPr" if !drawings.is_empty() => {
                            capture_textbox_body_properties(start, drawings.last_mut());
                        }
                        "t" | "delText" => {
                            ensure_run_open(paragraphs.last_mut(), runs.last_mut());
                            let deleted_revision = revisions.last().is_some_and(|revision| {
                                matches!(
                                    revision.kind,
                                    RevisionKind::Delete | RevisionKind::MoveFrom
                                )
                            });
                            current_text = Some(PendingText {
                                value: String::new(),
                                text_id: attribute_by_local_name(start, "textId")
                                    .or_else(|| runs.last().and_then(|run| run.text_id.clone())),
                                kind: if name == "delText" || deleted_revision {
                                    TextNodeKind::Deleted
                                } else {
                                    TextNodeKind::Editable
                                },
                            });
                        }
                        "instrText" => {
                            current_text = Some(PendingText {
                                value: String::new(),
                                text_id: None,
                                kind: TextNodeKind::FieldInstruction,
                            });
                        }
                        "blip" => {
                            ensure_run_open(paragraphs.last_mut(), runs.last_mut());
                            image_ordinal += 1;
                            append_image(
                                start,
                                document_id,
                                part,
                                image_ordinal,
                                relationships,
                                drawings.last(),
                                paragraphs.last_mut(),
                            );
                        }
                        _ => {}
                    }
                    if paragraph_properties_depth.is_some() && name != "pPr" {
                        capture_paragraph_property(start, paragraphs.last_mut());
                    }
                    if run_properties_depth.is_some() && name != "rPr" {
                        capture_run_property(start, runs.last_mut(), theme);
                    }
                    if cell_properties_depth.is_some() && name != "tcPr" {
                        capture_cell_property(
                            start,
                            cells.last_mut(),
                            theme,
                            cell_borders_depth.is_some(),
                            cell_margins_depth.is_some(),
                        );
                    }
                    if table_properties_depth.is_some() && name != "tblPr" {
                        capture_table_property(
                            start,
                            table.as_mut(),
                            theme,
                            table_bands,
                            table_borders_depth.is_some(),
                            table_cell_margins_depth.is_some(),
                        )?;
                    }
                    if table_property_exceptions_depth.is_some() && name != "tblPrEx" {
                        capture_table_exception_property(
                            start,
                            table.as_mut(),
                            theme,
                            table_exception_borders_depth.is_some(),
                            table_exception_cell_margins_depth.is_some(),
                        );
                    }
                    if row_properties_depth.is_some() && name != "trPr" {
                        capture_row_property(start, table.as_mut());
                    }
                }
            }
            Event::Empty(ref empty) => {
                element_count += 1;
                if element_count > MAX_XML_ELEMENTS {
                    return Err(HcdError::ResourceLimit(format!(
                        "{part} exceeds {MAX_XML_ELEMENTS} XML elements"
                    )));
                }
                let qualified_name = empty.name();
                let name = local_name(qualified_name.as_ref());
                if skipped_fallback_depth.is_some() {
                    buffer.clear();
                    continue;
                }
                if property_change_depth.is_some() {
                    buffer.clear();
                    continue;
                }
                if paragraph_properties_depth.is_some() {
                    capture_paragraph_property(empty, paragraphs.last_mut());
                }
                if run_properties_depth.is_some() {
                    capture_run_property(empty, runs.last_mut(), theme);
                }
                if cell_properties_depth.is_some() {
                    capture_cell_property(
                        empty,
                        cells.last_mut(),
                        theme,
                        cell_borders_depth.is_some(),
                        cell_margins_depth.is_some(),
                    );
                }
                if table_properties_depth.is_some() {
                    capture_table_property(
                        empty,
                        table.as_mut(),
                        theme,
                        table_bands,
                        table_borders_depth.is_some(),
                        table_cell_margins_depth.is_some(),
                    )?;
                }
                if table_property_exceptions_depth.is_some() {
                    capture_table_exception_property(
                        empty,
                        table.as_mut(),
                        theme,
                        table_exception_borders_depth.is_some(),
                        table_exception_cell_margins_depth.is_some(),
                    );
                }
                if row_properties_depth.is_some() {
                    capture_row_property(empty, table.as_mut());
                }
                match name {
                    "p" => {
                        ensure_current_cell_open(table.as_mut(), cells.last_mut());
                        paragraph_ordinal += 1;
                        let paragraph = ParagraphBuilder {
                            paragraph_id: attribute_by_local_name(empty, "paraId"),
                            ordinal: paragraph_ordinal,
                            ..Default::default()
                        };
                        let block = finish_paragraph(
                            document_id,
                            part,
                            paragraph,
                            numbering,
                            context.paragraph_numbering,
                            &mut numbering_state,
                        );
                        if let Some(table) = &mut table {
                            table.html.push_str(&block.html);
                            table.entries.extend(block.entries);
                        } else if paragraphs.last().is_some()
                            && drawings.last().is_some_and(|drawing| drawing.is_textbox)
                        {
                            if let Some(drawing) = drawings.last_mut() {
                                drawing.textbox_content.push(block);
                            }
                        } else if let Some(parent) = paragraphs.last_mut() {
                            parent.nested.push(block);
                        } else {
                            chunks.push_block(block, false)?;
                        }
                    }
                    "fldChar" => handle_complex_field_marker(
                        empty,
                        &mut fields,
                        paragraphs.last_mut(),
                        part,
                    )?,
                    "tblPrEx" if table_depth == 1 => {
                        if let Some(table) = &mut table {
                            table.row_has_property_exceptions = true;
                        }
                    }
                    "gridCol" if table_grid_depth.is_some() => {
                        if let Some(table) = &mut table {
                            table.capture_grid_column(empty)?;
                        }
                    }
                    "t" | "delText" => {
                        if name == "t" {
                            text_ordinal += 1;
                        }
                        ensure_run_open(paragraphs.last_mut(), runs.last_mut());
                        let text_id = attribute_by_local_name(empty, "textId")
                            .or_else(|| runs.last().and_then(|run| run.text_id.clone()));
                        let deleted_revision = revisions.last().is_some_and(|revision| {
                            matches!(revision.kind, RevisionKind::Delete | RevisionKind::MoveFrom)
                        });
                        if name == "t" && !deleted_revision {
                            let editable = !cells.iter().any(|cell| cell.read_only);
                            append_text_node(
                                &mut paragraphs,
                                document_id,
                                part,
                                text_ordinal,
                                text_id,
                                String::new(),
                                editable,
                            );
                        }
                    }
                    "tab" => {
                        ensure_run_open(paragraphs.last_mut(), runs.last_mut());
                        if let Some(paragraph) = paragraphs.last_mut() {
                            paragraph.html.push_str("&#9;");
                        }
                    }
                    "br" | "cr" => {
                        ensure_run_open(paragraphs.last_mut(), runs.last_mut());
                        if let Some(paragraph) = paragraphs.last_mut() {
                            paragraph.html.push_str("<br/>");
                        }
                    }
                    "extent" if !drawings.is_empty() => {
                        capture_drawing_extent(empty, drawings.last_mut());
                    }
                    "docPr" if !drawings.is_empty() => {
                        capture_drawing_alt(empty, drawings.last_mut());
                    }
                    "anchor" | "inline" if !drawings.is_empty() => {
                        capture_drawing_layout(empty, drawings.last_mut());
                    }
                    "simplePos" if !drawings.is_empty() => {
                        capture_drawing_simple_position(empty, drawings.last_mut());
                    }
                    "wrapSquare" | "wrapTight" | "wrapThrough" | "wrapTopAndBottom"
                    | "wrapNone"
                        if !drawings.is_empty() =>
                    {
                        capture_drawing_wrap(empty, drawings.last_mut());
                    }
                    "xfrm" if drawing_properties.shape_properties_depth.is_some() => {
                        capture_textbox_transform(empty, drawings.last_mut());
                    }
                    "prstGeom" if drawing_properties.shape_properties_depth.is_some() => {
                        capture_textbox_geometry(empty, drawings.last_mut());
                    }
                    "ln" if drawing_properties.shape_properties_depth.is_some() => {
                        capture_textbox_line(empty, drawings.last_mut());
                    }
                    "srgbClr" | "schemeClr" if !drawings.is_empty() => {
                        capture_textbox_color(
                            empty,
                            drawings.last_mut(),
                            &drawing_properties,
                            theme,
                        );
                    }
                    "alpha" if !drawings.is_empty() => {
                        capture_textbox_alpha(
                            empty,
                            drawings.last_mut(),
                            drawing_properties.color_scope.map(|(_, target)| target),
                        );
                    }
                    "lin" if drawing_properties.gradient_fill_depth.is_some() => {
                        capture_textbox_gradient_angle(empty, drawings.last_mut());
                    }
                    "prstDash" if drawing_properties.line_depth.is_some() => {
                        capture_textbox_line_dash(empty, drawings.last_mut());
                    }
                    "noFill" if drawing_properties.line_depth.is_some() => {
                        if let Some(drawing) = drawings.last_mut() {
                            drawing.no_line = true;
                        }
                    }
                    "noFill" if drawing_properties.shape_properties_depth.is_some() => {
                        if let Some(drawing) = drawings.last_mut() {
                            drawing.no_shape_fill = true;
                        }
                    }
                    "txbx" | "txbxContent" if !drawings.is_empty() => {
                        if let Some(drawing) = drawings.last_mut() {
                            drawing.is_textbox = true;
                        }
                    }
                    "bodyPr" if !drawings.is_empty() => {
                        capture_textbox_body_properties(empty, drawings.last_mut());
                    }
                    "blip" => {
                        ensure_run_open(paragraphs.last_mut(), runs.last_mut());
                        image_ordinal += 1;
                        append_image(
                            empty,
                            document_id,
                            part,
                            image_ordinal,
                            relationships,
                            drawings.last(),
                            paragraphs.last_mut(),
                        );
                    }
                    _ => {}
                }
            }
            Event::Text(ref text) => {
                if skipped_fallback_depth.is_some() {
                    buffer.clear();
                    continue;
                }
                if text.as_ref().len() > MAX_CHUNK_BYTES {
                    return Err(HcdError::ResourceLimit(format!(
                        "NODE_TOO_LARGE: XML text in {part} exceeds {MAX_CHUNK_BYTES} bytes"
                    )));
                }
                if let Some(pending) = &mut drawing_text {
                    let decoded = text.unescape().map_err(|error| {
                        HcdError::InvalidBundle(format!(
                            "invalid DrawingML position text in {part}: {error}"
                        ))
                    })?;
                    pending.value.push_str(&decoded);
                    if pending.value.len() > 128 {
                        return Err(HcdError::ResourceLimit(format!(
                            "DrawingML position value in {part} exceeds 128 bytes"
                        )));
                    }
                } else if let Some(PendingText { value, .. }) = &mut current_text {
                    let decoded = text.unescape().map_err(|error| {
                        HcdError::InvalidBundle(format!("invalid text in {part}: {error}"))
                    })?;
                    value.push_str(&decoded);
                    if value.len() > MAX_CHUNK_BYTES {
                        return Err(HcdError::ResourceLimit(format!(
                            "NODE_TOO_LARGE: XML text in {part} exceeds {MAX_CHUNK_BYTES} bytes"
                        )));
                    }
                }
            }
            Event::CData(ref text) => {
                if skipped_fallback_depth.is_some() {
                    buffer.clear();
                    continue;
                }
                if let Some(pending) = &mut drawing_text {
                    pending
                        .value
                        .push_str(&String::from_utf8_lossy(text.as_ref()));
                    if pending.value.len() > 128 {
                        return Err(HcdError::ResourceLimit(format!(
                            "DrawingML position value in {part} exceeds 128 bytes"
                        )));
                    }
                } else if let Some(PendingText { value, .. }) = &mut current_text {
                    value.push_str(&String::from_utf8_lossy(text.as_ref()));
                    if value.len() > MAX_CHUNK_BYTES {
                        return Err(HcdError::ResourceLimit(format!(
                            "NODE_TOO_LARGE: XML text in {part} exceeds {MAX_CHUNK_BYTES} bytes"
                        )));
                    }
                }
            }
            Event::End(ref end) => {
                let qualified_name = end.name();
                let name = local_name(qualified_name.as_ref());
                if let Some(skip_depth) = skipped_fallback_depth {
                    if xml_depth == skip_depth && name == "Fallback" {
                        skipped_fallback_depth = None;
                    }
                    xml_depth = xml_depth.checked_sub(1).ok_or_else(|| {
                        HcdError::InvalidBundle(format!("unbalanced XML depth in {part}"))
                    })?;
                    buffer.clear();
                    continue;
                }
                if property_change_depth.is_some() {
                    if property_change_depth == Some(xml_depth) {
                        property_change_depth = None;
                    }
                } else {
                    match name {
                        "t" | "delText" | "instrText" => {
                            if name == "t" {
                                text_ordinal += 1;
                            }
                            if let Some(pending) = current_text.take() {
                                if pending.kind == TextNodeKind::FieldInstruction {
                                    if let Some(field) = fields.last_mut() {
                                        field.instruction.push_str(&pending.value);
                                    }
                                } else if pending.kind == TextNodeKind::Editable && name == "t" {
                                    let editable = !cells.iter().any(|cell| cell.read_only);
                                    append_text_node(
                                        &mut paragraphs,
                                        document_id,
                                        part,
                                        text_ordinal,
                                        pending.text_id,
                                        pending.value,
                                        editable,
                                    );
                                } else {
                                    append_read_only_text(paragraphs.last_mut(), &pending.value);
                                }
                            }
                        }
                        "r" => {
                            if let Some(run) = runs.pop() {
                                if run.opened {
                                    if let Some(paragraph) = paragraphs.last_mut() {
                                        paragraph.html.push_str("</span>");
                                    }
                                }
                            }
                        }
                        "rPr" if run_properties_depth == Some(xml_depth) => {
                            run_properties_depth = None;
                        }
                        "hyperlink" => {
                            if let Some(is_anchor) = hyperlink_tags.pop() {
                                if let Some(paragraph) = paragraphs.last_mut() {
                                    paragraph.html.push_str(if is_anchor {
                                        "</a>"
                                    } else {
                                        "</span>"
                                    });
                                }
                            }
                        }
                        "fldSimple" => {
                            let field = fields.pop().ok_or_else(|| {
                                HcdError::InvalidBundle(format!(
                                    "unbalanced simple field in {part}"
                                ))
                            })?;
                            if !field.simple {
                                return Err(HcdError::InvalidBundle(format!(
                                    "simple field closed inside a complex field in {part}"
                                )));
                            }
                            append_field_hyperlink_end(
                                field.wrapper_is_anchor,
                                paragraphs.last_mut(),
                            );
                        }
                        "drawing" => {
                            if let Some(drawing) = drawings.pop() {
                                if drawing.is_textbox {
                                    let block = render_textbox(document_id, part, drawing);
                                    if let Some(paragraph) = paragraphs.last_mut() {
                                        paragraph.nested.push(block);
                                    }
                                }
                            }
                            drawing_properties = DrawingPropertyState::default();
                        }
                        "posOffset" | "align" => {
                            finish_drawing_text(drawing_text.take(), drawings.last_mut(), part)?;
                        }
                        "positionH" | "positionV" => {
                            if let Some(drawing) = drawings.last_mut() {
                                drawing.active_position_axis = None;
                            }
                        }
                        "pPr" if paragraph_properties_depth == Some(xml_depth) => {
                            paragraph_properties_depth = None;
                        }
                        "p" => {
                            let paragraph = paragraphs.pop().ok_or_else(|| {
                                HcdError::InvalidBundle(format!("unbalanced paragraph in {part}"))
                            })?;
                            let block = finish_paragraph(
                                document_id,
                                part,
                                paragraph,
                                numbering,
                                context.paragraph_numbering,
                                &mut numbering_state,
                            );
                            if let Some(table) = &mut table {
                                table.html.push_str(&block.html);
                                table.entries.extend(block.entries);
                            } else if paragraphs.last().is_some()
                                && drawings.last().is_some_and(|drawing| drawing.is_textbox)
                            {
                                if let Some(drawing) = drawings.last_mut() {
                                    drawing.textbox_content.push(block);
                                }
                            } else if let Some(parent) = paragraphs.last_mut() {
                                parent.nested.push(block);
                            } else {
                                chunks.push_block(block, false)?;
                            }
                        }
                        "tcBorders" if cell_borders_depth == Some(xml_depth) => {
                            cell_borders_depth = None;
                        }
                        "tcMar" if cell_margins_depth == Some(xml_depth) => {
                            cell_margins_depth = None;
                        }
                        "tcPr" if cell_properties_depth == Some(xml_depth) => {
                            ensure_current_cell_open(table.as_mut(), cells.last_mut());
                            cell_properties_depth = None;
                        }
                        "tblBorders" if table_borders_depth == Some(xml_depth) => {
                            table_borders_depth = None;
                        }
                        "tblCellMar" if table_cell_margins_depth == Some(xml_depth) => {
                            table_cell_margins_depth = None;
                        }
                        "tblBorders" if table_exception_borders_depth == Some(xml_depth) => {
                            table_exception_borders_depth = None;
                        }
                        "tblCellMar" if table_exception_cell_margins_depth == Some(xml_depth) => {
                            table_exception_cell_margins_depth = None;
                        }
                        "tblPrEx" if table_property_exceptions_depth == Some(xml_depth) => {
                            table_property_exceptions_depth = None;
                        }
                        "tblPr" if table_properties_depth == Some(xml_depth) => {
                            table_properties_depth = None;
                        }
                        "tblGrid" if table_grid_depth == Some(xml_depth) => {
                            table_grid_depth = None;
                        }
                        "trPr" if row_properties_depth == Some(xml_depth) => {
                            row_properties_depth = None;
                        }
                        "tc" if table_depth > 0 => {
                            ensure_current_cell_open(table.as_mut(), cells.last_mut());
                            if let Some(table) = &mut table {
                                table.html.push_str("</td>");
                            }
                            cells.pop();
                        }
                        "tr" if table_depth > 0 => {
                            if let Some(table) = &mut table {
                                if table_depth == 1 {
                                    table.ensure_row_open();
                                }
                                table.html.push_str("</tr>");
                                if table_depth == 1 {
                                    table.finish_row()?;
                                    table.rows_in_fragment += 1;
                                }
                            }
                        }
                        "tbl" if table_depth > 0 => {
                            if table_depth > 1 {
                                if let Some(table) = &mut table {
                                    table.html.push_str("</tbody></table>");
                                }
                            } else if let Some(mut finished) = table.take() {
                                finished.finish_table_merges();
                                if finished.rows_in_fragment > 0 || !finished.entries.is_empty() {
                                    finished.mark_final_fragment();
                                    let continuation = finished.continuation;
                                    let block = take_table_fragment(&mut finished);
                                    if drawings.last().is_some_and(|drawing| drawing.is_textbox) {
                                        if let Some(drawing) = drawings.last_mut() {
                                            drawing.textbox_content.push(block);
                                        }
                                    } else {
                                        chunks.push_block(block, continuation)?;
                                    }
                                }
                            }
                            table_depth -= 1;
                        }
                        "srgbClr" | "schemeClr"
                            if drawing_properties
                                .color_scope
                                .is_some_and(|(depth, _)| depth == xml_depth) =>
                        {
                            drawing_properties.color_scope = None;
                        }
                        "solidFill"
                            if drawing_properties.line_solid_fill_depth == Some(xml_depth) =>
                        {
                            drawing_properties.line_solid_fill_depth = None;
                        }
                        "solidFill"
                            if drawing_properties.shape_solid_fill_depth == Some(xml_depth) =>
                        {
                            drawing_properties.shape_solid_fill_depth = None;
                        }
                        "gradFill" if drawing_properties.gradient_fill_depth == Some(xml_depth) => {
                            drawing_properties.gradient_fill_depth = None;
                        }
                        "outerShdw" if drawing_properties.outer_shadow_depth == Some(xml_depth) => {
                            drawing_properties.outer_shadow_depth = None;
                        }
                        "ln" if drawing_properties.line_depth == Some(xml_depth) => {
                            drawing_properties.line_depth = None;
                        }
                        "xfrm" if drawing_properties.transform_depth == Some(xml_depth) => {
                            drawing_properties.transform_depth = None;
                        }
                        "spPr" if drawing_properties.shape_properties_depth == Some(xml_depth) => {
                            drawing_properties.shape_properties_depth = None;
                        }
                        _ => {
                            if let Some(kind) = RevisionKind::from_ooxml(name) {
                                close_revision(kind, &mut revisions, paragraphs.last_mut(), part)?;
                            }
                        }
                    }
                }
                if name == "AlternateContent" {
                    alternate_content_support.pop();
                }
                xml_depth = xml_depth.checked_sub(1).ok_or_else(|| {
                    HcdError::InvalidBundle(format!("unbalanced XML depth in {part}"))
                })?;
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if xml_depth != 0
        || !paragraphs.is_empty()
        || table_depth != 0
        || current_text.is_some()
        || !fields.is_empty()
        || drawing_text.is_some()
        || !revisions.is_empty()
        || property_change_depth.is_some()
        || table_properties_depth.is_some()
        || table_property_exceptions_depth.is_some()
        || table_borders_depth.is_some()
        || table_cell_margins_depth.is_some()
        || table_exception_borders_depth.is_some()
        || table_exception_cell_margins_depth.is_some()
        || table_grid_depth.is_some()
        || row_properties_depth.is_some()
        || cell_properties_depth.is_some()
        || cell_borders_depth.is_some()
        || cell_margins_depth.is_some()
    {
        return Err(HcdError::InvalidBundle(format!(
            "unbalanced text structure in {part}"
        )));
    }
    if part == "word/document.xml" {
        warnings.push(FidelityWarning {
            code: "OPAQUE_OFFICE_OBJECTS_PRESERVED".to_string(),
            message: "Fields, macros, OLE and unsupported DrawingML remain in the immutable source and are not editable in HCD v1".to_string(),
            node_id: None,
            source_part: Some(part.to_string()),
        });
        warnings.push(FidelityWarning {
            code: "STYLE_AND_STRUCTURE_PATCH_UNSUPPORTED".to_string(),
            message: "Formatting and structure are rendered from OOXML but remain read-only in HCD v1; only text and annotations are patchable".to_string(),
            node_id: None,
            source_part: Some(part.to_string()),
        });
    }
    Ok(())
}

fn append_text_node(
    paragraphs: &mut [ParagraphBuilder],
    document_id: &str,
    part: &str,
    text_ordinal: u64,
    text_id: Option<String>,
    text: String,
    editable: bool,
) {
    let Some(paragraph) = paragraphs.last_mut() else {
        return;
    };
    let paragraph_id = paragraph
        .paragraph_id
        .clone()
        .unwrap_or_else(|| paragraph.ordinal.to_string());
    let local_ordinal = paragraph.entries.len() + 1;
    let source_identity = text_id
        .as_deref()
        .map(|id| format!("{paragraph_id}:{id}"))
        .unwrap_or_else(|| format!("{paragraph_id}:{local_ordinal}"));
    let node_id = stable_node_id(&[document_id, part, "text", &source_identity]);
    let (canonical_text, rendered_text) =
        render_legacy_hyperlink_text(&text).unwrap_or_else(|| (text.clone(), escape_text(&text)));
    let node_hash = hash_bytes(canonical_text.as_bytes());
    paragraph.has_visible_text |= !canonical_text.is_empty();
    paragraph.html.push_str(&format!(
        "<span data-hcd-id=\"{}\" data-hcd-node-hash=\"{}\">{}</span>",
        escape_attribute(&node_id),
        node_hash,
        rendered_text
    ));
    paragraph.entries.push(NodeMapEntry {
        node_id,
        node_hash,
        source: SourceAnchor {
            part: part.to_string(),
            text_ordinal,
            paragraph_id: paragraph.paragraph_id.clone(),
            text_id,
            node_kind: "text".to_string(),
            editable,
        },
    });
}

/// Recover field-like hyperlink text produced by legacy `.doc` readers that
/// flattened Word field codes into a normal `w:t`. The canonical node text is
/// the visible field result; the immutable source remains the export boundary
/// until that node is patched.
fn render_legacy_hyperlink_text(text: &str) -> Option<(String, String)> {
    const MARKER: &str = "HYPERLINK \"";
    let mut cursor = 0usize;
    let mut canonical = String::with_capacity(text.len());
    let mut html = String::with_capacity(text.len());
    let mut recovered = 0usize;
    while let Some(relative_marker) = text[cursor..].find(MARKER) {
        let marker = cursor + relative_marker;
        let plain = &text[cursor..marker];
        canonical.push_str(plain);
        html.push_str(&escape_text(plain));

        let target_start = marker + MARKER.len();
        let Some(relative_target_end) = text[target_start..].find('"') else {
            canonical.push_str(&text[marker..]);
            html.push_str(&escape_text(&text[marker..]));
            cursor = text.len();
            break;
        };
        let target_end = target_start + relative_target_end;
        let mut target = text[target_start..target_end].to_string();
        let mut label_start = target_end + 1;
        let mut anchor = None;
        loop {
            let switch_start = skip_ascii_whitespace(text, label_start);
            let switch = text.get(switch_start..switch_start + 2);
            if !matches!(switch, Some("\\l" | "\\t" | "\\o")) {
                break;
            }
            let value_start = skip_ascii_whitespace(text, switch_start + 2);
            if text.as_bytes().get(value_start) != Some(&b'"') {
                break;
            }
            let quoted_start = value_start + 1;
            let Some(relative_end) = text[quoted_start..].find('"') else {
                break;
            };
            let quoted_end = quoted_start + relative_end;
            if switch == Some("\\l") {
                anchor = Some(text[quoted_start..quoted_end].to_string());
            }
            label_start = quoted_end + 1;
        }
        if let Some(anchor) = anchor.filter(|value| value.chars().all(is_safe_fragment_character)) {
            if !target.is_empty() {
                target.push('#');
                target.push_str(&anchor);
            } else {
                target = format!("#{anchor}");
            }
        }
        let next_marker = text[label_start..]
            .find(MARKER)
            .map(|offset| label_start + offset)
            .unwrap_or(text.len());
        let label = &text[label_start..next_marker];
        canonical.push_str(label);
        if label.is_empty() {
            // A malformed field without a result is retained literally.
            canonical.push_str(&text[marker..label_start]);
            html.push_str(&escape_text(&text[marker..label_start]));
        } else if let Some(href) = safe_hyperlink_href(&target) {
            html.push_str(&format!(
                "<a class=\"hcd-hyperlink hcd-legacy-field-hyperlink\" href=\"{}\">{}</a>",
                escape_attribute(href),
                escape_text(label)
            ));
            recovered += 1;
        } else {
            html.push_str(&format!(
                "<a class=\"hcd-hyperlink hcd-legacy-field-hyperlink hcd-hyperlink-blocked\">{}</a>",
                escape_text(label)
            ));
            recovered += 1;
        }
        cursor = next_marker;
    }
    if cursor < text.len() {
        canonical.push_str(&text[cursor..]);
        html.push_str(&escape_text(&text[cursor..]));
    }
    (recovered > 0).then_some((canonical, html))
}

fn skip_ascii_whitespace(value: &str, mut offset: usize) -> usize {
    while value
        .as_bytes()
        .get(offset)
        .is_some_and(u8::is_ascii_whitespace)
    {
        offset += 1;
    }
    offset
}

fn append_read_only_text(paragraph: Option<&mut ParagraphBuilder>, text: &str) {
    let Some(paragraph) = paragraph else {
        return;
    };
    paragraph.has_visible_text |= !text.is_empty();
    paragraph
        .html
        .push_str("<span class=\"hcd-revision-text\" data-hcd-editable=\"false\">");
    paragraph.html.push_str(&escape_text(text));
    paragraph.html.push_str("</span>");
}

fn open_revision(
    element: &BytesStart<'_>,
    kind: RevisionKind,
    paragraph: Option<&mut ParagraphBuilder>,
) -> RevisionFrame {
    let Some(paragraph) = paragraph else {
        return RevisionFrame {
            kind,
            opened: false,
        };
    };
    let mut attributes = format!(
        " class=\"hcd-revision {}\" data-hcd-revision=\"{}\"",
        kind.css_class(),
        kind.html_value()
    );
    for (source, target) in [
        ("id", "data-hcd-revision-id"),
        ("author", "data-hcd-revision-author"),
        ("date", "data-hcd-revision-date"),
    ] {
        let value = attribute_by_local_name(element, source)
            .filter(|value| value.len() <= 256 && !value.chars().any(char::is_control));
        push_data_attribute(&mut attributes, target, value.as_deref());
    }
    paragraph.html.push_str("<span");
    paragraph.html.push_str(&attributes);
    paragraph.html.push('>');
    RevisionFrame { kind, opened: true }
}

fn close_revision(
    kind: RevisionKind,
    revisions: &mut Vec<RevisionFrame>,
    paragraph: Option<&mut ParagraphBuilder>,
    part: &str,
) -> Result<(), HcdError> {
    let frame = revisions
        .pop()
        .ok_or_else(|| HcdError::InvalidBundle(format!("unbalanced revision marker in {part}")))?;
    if frame.kind != kind {
        return Err(HcdError::InvalidBundle(format!(
            "mismatched revision marker in {part}"
        )));
    }
    if frame.opened {
        let paragraph = paragraph.ok_or_else(|| {
            HcdError::InvalidBundle(format!("revision crossed a paragraph boundary in {part}"))
        })?;
        paragraph.html.push_str("</span>");
    }
    Ok(())
}

fn ensure_run_open(paragraph: Option<&mut ParagraphBuilder>, run: Option<&mut RunBuilder>) {
    let (Some(paragraph), Some(run)) = (paragraph, run) else {
        return;
    };
    if run.opened {
        return;
    }
    paragraph
        .html
        .push_str(&run_html_start(&run.format, run.ordinal));
    run.opened = true;
}

fn run_html_start(format: &RunFormat, ordinal: usize) -> String {
    let style_class = format
        .style_id
        .as_deref()
        .map(|style_id| format!(" {}", word_style_class(style_id)))
        .unwrap_or_default();
    let mut attributes =
        format!(" class=\"hcd-run{style_class}\" data-hcd-run-index=\"{ordinal}\"");
    push_data_attribute(
        &mut attributes,
        "data-hcd-word-style",
        format.style_id.as_deref(),
    );
    push_data_attribute(&mut attributes, "data-hcd-font", format.font.as_deref());
    push_data_attribute(
        &mut attributes,
        "data-hcd-font-latin",
        format.resolved_latin_font.as_deref(),
    );
    push_data_attribute(
        &mut attributes,
        "data-hcd-font-east-asia",
        format.resolved_east_asia_font.as_deref(),
    );
    push_data_attribute(
        &mut attributes,
        "data-hcd-font-bidi",
        format.resolved_bidi_font.as_deref(),
    );
    push_data_attribute(
        &mut attributes,
        "data-hcd-underline",
        format.underline.as_deref(),
    );

    let css = run_css_declarations(format);
    if !css.is_empty() {
        attributes.push_str(" style=\"");
        attributes.push_str(&css.join(";"));
        attributes.push('"');
    }
    format!("<span{attributes}>")
}

fn capture_run_property(element: &BytesStart<'_>, run: Option<&mut RunBuilder>, theme: &WordTheme) {
    let Some(run) = run else {
        return;
    };
    capture_run_format_property(element, &mut run.format, theme);
}

fn capture_run_format_property(
    element: &BytesStart<'_>,
    format: &mut RunFormat,
    theme: &WordTheme,
) {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    let value = attribute_by_local_name(element, "val");
    match name {
        "rStyle" => format.style_id = value,
        "b" => format.bold = on_off_value(value.as_deref()),
        "i" => format.italic = on_off_value(value.as_deref()),
        "strike" | "dstrike" => format.strike = on_off_value(value.as_deref()),
        "u" if value.as_deref() != Some("none") => {
            format.underline = Some(value.unwrap_or_else(|| "single".to_string()))
        }
        "color" => format.color = resolve_run_color(element, theme).or(value),
        "highlight" => format.highlight = value,
        "rFonts" => {
            format.latin_font = attribute_by_local_name(element, "ascii")
                .or_else(|| attribute_by_local_name(element, "hAnsi"))
                .filter(|value| safe_font_family(value).is_some());
            format.east_asia_font = attribute_by_local_name(element, "eastAsia")
                .filter(|value| safe_font_family(value).is_some());
            format.bidi_font = attribute_by_local_name(element, "cs")
                .filter(|value| safe_font_family(value).is_some());
            format.latin_theme = attribute_by_local_name(element, "asciiTheme")
                .or_else(|| attribute_by_local_name(element, "hAnsiTheme"));
            format.east_asia_theme = attribute_by_local_name(element, "eastAsiaTheme");
            format.bidi_theme = attribute_by_local_name(element, "cstheme");
            refresh_run_fonts(format, theme);
        }
        "lang" => {
            format.latin_language =
                attribute_by_local_name(element, "val").filter(|value| is_safe_language_tag(value));
            format.east_asia_language = attribute_by_local_name(element, "eastAsia")
                .filter(|value| is_safe_language_tag(value));
            format.bidi_language = attribute_by_local_name(element, "bidi")
                .filter(|value| is_safe_language_tag(value));
            refresh_run_fonts(format, theme);
        }
        "sz" => format.size_half_points = value.and_then(|value| value.parse().ok()),
        "vertAlign" => format.vertical_align = value,
        "rtl" => {
            format.rtl = on_off_value(value.as_deref());
            refresh_run_fonts(format, theme);
        }
        "vanish" => format.hidden = on_off_value(value.as_deref()),
        _ => {}
    }
}

fn capture_paragraph_property(element: &BytesStart<'_>, paragraph: Option<&mut ParagraphBuilder>) {
    let Some(paragraph) = paragraph else {
        return;
    };
    capture_paragraph_format_property(element, &mut paragraph.format);
}

fn capture_paragraph_format_property(element: &BytesStart<'_>, format: &mut ParagraphFormat) {
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    let value = attribute_by_local_name(element, "val");
    match name {
        "pStyle" => format.style_id = value,
        "cnfStyle" => capture_conditional_style(element, &mut format.conditional_style),
        "jc" => format.alignment = value,
        "bidi" => format.bidi = on_off_value(value.as_deref()),
        "keepNext" => format.keep_next = on_off_value(value.as_deref()),
        "keepLines" => format.keep_lines = on_off_value(value.as_deref()),
        "pageBreakBefore" => format.page_break_before = on_off_value(value.as_deref()),
        "ind" => {
            format.left_twips =
                signed_attribute(element, "left").or_else(|| signed_attribute(element, "start"));
            format.right_twips =
                signed_attribute(element, "right").or_else(|| signed_attribute(element, "end"));
            format.first_line_twips = signed_attribute(element, "firstLine");
            format.hanging_twips = signed_attribute(element, "hanging");
        }
        "spacing" => {
            format.before_twips = signed_attribute(element, "before");
            format.after_twips = signed_attribute(element, "after");
            format.line_twips = signed_attribute(element, "line");
            format.line_rule = attribute_by_local_name(element, "lineRule");
        }
        "numId" => format.numbering_id = value,
        "ilvl" => format.numbering_level = value,
        _ => {}
    }
}

fn paragraph_html_attributes(format: &ParagraphFormat) -> String {
    let mut attributes = String::new();
    push_data_attribute(
        &mut attributes,
        "data-hcd-word-style",
        format.style_id.as_deref(),
    );
    push_data_attribute(
        &mut attributes,
        "data-hcd-num-id",
        format.numbering_id.as_deref(),
    );
    push_data_attribute(
        &mut attributes,
        "data-hcd-num-level",
        format.numbering_level.as_deref(),
    );
    if format.keep_next {
        attributes.push_str(" data-hcd-keep-next=\"true\"");
    }
    if format.keep_lines {
        attributes.push_str(" data-hcd-keep-lines=\"true\"");
    }
    if format.page_break_before {
        attributes.push_str(" data-hcd-page-break-before=\"true\"");
    }
    push_conditional_style_attributes(&mut attributes, format.conditional_style);

    let css = paragraph_css_declarations(format);
    if !css.is_empty() {
        attributes.push_str(" style=\"");
        attributes.push_str(&css.join(";"));
        attributes.push('"');
    }
    attributes
}

fn run_css_declarations(format: &RunFormat) -> Vec<String> {
    let mut css = Vec::new();
    if format.bold {
        css.push("font-weight:700".to_string());
    }
    if format.italic {
        css.push("font-style:italic".to_string());
    }
    let mut decorations = Vec::new();
    if format.underline.is_some() {
        decorations.push("underline");
    }
    if format.strike {
        decorations.push("line-through");
    }
    if !decorations.is_empty() {
        css.push(format!("text-decoration:{}", decorations.join(" ")));
    }
    if let Some(color) = format.color.as_deref().and_then(strict_hex_color) {
        css.push(format!("color:#{color}"));
    }
    if let Some(color) = format.highlight.as_deref().and_then(highlight_css_color) {
        css.push(format!("background-color:{color}"));
    }
    if let Some(size) = format
        .size_half_points
        .filter(|size| (2..=3276).contains(size))
    {
        css.push(format!("font-size:{:.1}pt", size as f64 / 2.0));
    }
    let fonts = run_font_families(format);
    if !fonts.is_empty() {
        css.push(format!("font-family:{}", word_font_stack(&fonts)));
    }
    match format.vertical_align.as_deref() {
        Some("superscript") => css.push("vertical-align:super;font-size:.75em".to_string()),
        Some("subscript") => css.push("vertical-align:sub;font-size:.75em".to_string()),
        _ => {}
    }
    if format.rtl {
        css.push("direction:rtl".to_string());
        css.push("unicode-bidi:isolate".to_string());
    }
    if format.hidden {
        css.push("display:none".to_string());
    }
    css
}

fn run_font_families(format: &RunFormat) -> Vec<&str> {
    let mut fonts = Vec::new();
    for font in [
        format.font.as_deref(),
        format.resolved_latin_font.as_deref(),
        format.resolved_east_asia_font.as_deref(),
        format.resolved_bidi_font.as_deref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(safe_font_family)
    {
        if !fonts.contains(&font) {
            fonts.push(font);
        }
    }
    fonts
}

fn word_font_stack(fonts: &[&str]) -> String {
    let mut stack = fonts
        .iter()
        .map(|font| format!("'{}'", font.replace('\'', "")))
        .collect::<Vec<_>>();
    match fonts.first().map(|font| font.to_ascii_lowercase()) {
        Some(font) if matches!(font.as_str(), "calibri" | "arial") => {
            stack.push("-apple-system".to_string());
            stack.push("sans-serif".to_string());
        }
        Some(font) if font == "times new roman" => {
            stack.push("Georgia".to_string());
            stack.push("serif".to_string());
        }
        _ => {
            stack.push("'Songti SC'".to_string());
            stack.push("'STSong'".to_string());
            stack.push("sans-serif".to_string());
        }
    }
    stack.join(",")
}

fn paragraph_css_declarations(format: &ParagraphFormat) -> Vec<String> {
    let mut css = Vec::new();
    if let Some(alignment) = format.alignment.as_deref().and_then(html_alignment) {
        css.push(format!("text-align:{alignment}"));
    }
    if format.bidi {
        css.push("direction:rtl".to_string());
    }
    push_twips_css(&mut css, "margin-left", format.left_twips);
    push_twips_css(&mut css, "margin-right", format.right_twips);
    push_twips_css(&mut css, "margin-top", format.before_twips);
    push_twips_css(&mut css, "margin-bottom", format.after_twips);
    if let Some(first_line) = format.first_line_twips {
        push_twips_css(&mut css, "text-indent", Some(first_line));
    } else if let Some(hanging) = format.hanging_twips {
        push_twips_css(&mut css, "text-indent", Some(-hanging));
    }
    if let Some(line) = format
        .line_twips
        .filter(|value| (1..=100_000).contains(value))
    {
        if format.line_rule.as_deref().unwrap_or("auto") == "auto" {
            css.push(format!("line-height:{:.4}", line as f64 / 240.0));
        } else {
            css.push(format!("line-height:{:.2}pt", line as f64 / 20.0));
        }
    }
    if format.page_break_before {
        css.push("break-before:page".to_string());
    }
    css
}

fn word_style_class(style_id: &str) -> String {
    let hash = hash_bytes(style_id.as_bytes());
    format!("hcd-ws-{}", &hash[..16])
}

fn capture_cell_property(
    element: &BytesStart<'_>,
    cell: Option<&mut CellBuilder>,
    theme: &WordTheme,
    in_borders: bool,
    in_margins: bool,
) {
    let Some(cell) = cell else {
        return;
    };
    if in_borders {
        capture_table_border(element, &mut cell.css);
        return;
    }
    if in_margins {
        capture_table_cell_margin(element, &mut cell.css);
        return;
    }
    match local_name(element.name().as_ref()) {
        "cnfStyle" => capture_conditional_style(element, &mut cell.conditional_style),
        "gridSpan" => {
            cell.grid_span = attribute_by_local_name(element, "val")
                .and_then(|value| value.parse::<u32>().ok())
                .filter(|value| (2..=1024).contains(value));
        }
        "vMerge" => {
            cell.vertical_merge = Some(
                attribute_by_local_name(element, "val").unwrap_or_else(|| "continue".to_string()),
            );
        }
        "shd" => capture_table_shading(element, theme, &mut cell.css),
        "tcW" => capture_table_width(element, &mut cell.css),
        "vAlign" => {
            if let Some(value) = attribute_by_local_name(element, "val")
                .and_then(|value| table_cell_vertical_align(&value))
            {
                cell.css.push(format!("vertical-align:{value}"));
            }
        }
        _ => {}
    }
}

fn capture_table_property(
    element: &BytesStart<'_>,
    table: Option<&mut TableBuilder>,
    theme: &WordTheme,
    table_bands: &TableBandCatalog,
    in_borders: bool,
    in_cell_margins: bool,
) -> Result<(), HcdError> {
    let Some(table) = table else {
        return Ok(());
    };
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    match name {
        "tblStyle" => table.capture_style(element, table_bands),
        "tblLook" => table.capture_look(element),
        _ if in_borders => capture_table_border(element, &mut table.cell_css),
        _ if in_cell_margins => capture_table_cell_margin(element, &mut table.cell_css),
        "shd" => capture_table_shading(element, theme, &mut table.table_css),
        "tblW" => capture_table_width(element, &mut table.table_css),
        "jc" => capture_table_alignment(element, &mut table.table_css),
        "tblLayout" if attribute_by_local_name(element, "type").as_deref() == Some("fixed") => {
            table.table_css.push("table-layout:fixed".to_string());
        }
        "tblStyleRowBandSize" => {
            table.row_band_size = parse_table_band_size(element, name)?;
            table.row_band_size_direct = true;
        }
        "tblStyleColBandSize" => {
            table.column_band_size = parse_table_band_size(element, name)?;
            table.column_band_size_direct = true;
        }
        _ => return Ok(()),
    }
    if !matches!(name, "tblStyle" | "tblLook") {
        table.refresh_opening();
    }
    Ok(())
}

fn capture_table_exception_property(
    element: &BytesStart<'_>,
    table: Option<&mut TableBuilder>,
    theme: &WordTheme,
    in_borders: bool,
    in_cell_margins: bool,
) {
    let Some(table) = table.filter(|table| !table.row_opened) else {
        return;
    };
    table.row_has_property_exceptions = true;
    if in_borders {
        capture_table_border(element, &mut table.row_exception_cell_css);
        return;
    }
    if in_cell_margins {
        capture_table_cell_margin(element, &mut table.row_exception_cell_css);
        return;
    }
    if local_name(element.name().as_ref()) == "shd" {
        capture_table_shading(element, theme, &mut table.row_exception_cell_css);
    }
}

fn parse_table_band_size(element: &BytesStart<'_>, property: &str) -> Result<u32, HcdError> {
    let value = attribute_by_local_name(element, "val").ok_or_else(|| {
        HcdError::InvalidBundle(format!("{property} is missing its val attribute"))
    })?;
    let value = value.parse::<u32>().map_err(|_| {
        HcdError::InvalidBundle(format!("{property} has invalid band size {value}"))
    })?;
    if value == 0 {
        return Err(HcdError::InvalidBundle(format!(
            "{property} band size must be positive"
        )));
    }
    if value > MAX_TABLE_BAND_SIZE {
        return Err(HcdError::ResourceLimit(format!(
            "{property} band size {value} exceeds {MAX_TABLE_BAND_SIZE}"
        )));
    }
    Ok(value)
}

fn capture_row_property(element: &BytesStart<'_>, table: Option<&mut TableBuilder>) {
    let Some(table) = table.filter(|table| !table.row_opened) else {
        return;
    };
    match local_name(element.name().as_ref()) {
        "cnfStyle" => capture_conditional_style(element, &mut table.row_conditional_style),
        "trHeight" => {
            if let Some(twips) = attribute_by_local_name(element, "val")
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value <= 2_000_000)
            {
                let property =
                    if attribute_by_local_name(element, "hRule").as_deref() == Some("exact") {
                        "height"
                    } else {
                        "min-height"
                    };
                table
                    .row_css
                    .push(format!("{property}:{:.2}pt", twips as f64 / 20.0));
            }
        }
        "cantSplit" if on_off_value(attribute_by_local_name(element, "val").as_deref()) => {
            table.row_css.push("break-inside:avoid".to_string());
        }
        _ => {}
    }
}

fn ensure_current_cell_open(table: Option<&mut TableBuilder>, cell: Option<&mut CellBuilder>) {
    let (Some(table), Some(cell)) = (table, cell) else {
        return;
    };
    if cell.opened {
        return;
    }
    table.open_cell(cell);
    cell.opened = true;
}

fn capture_drawing_extent(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.width_emu = unsigned_drawing_emu_attribute(element, "cx");
    drawing.height_emu = unsigned_drawing_emu_attribute(element, "cy");
}

fn capture_drawing_alt(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.drawing_id = attribute_by_local_name(element, "id")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= u32::MAX.into());
    drawing.alt = attribute_by_local_name(element, "descr")
        .or_else(|| attribute_by_local_name(element, "title"))
        .or_else(|| attribute_by_local_name(element, "name"))
        .filter(|value| value.len() <= 1024 && !value.chars().any(char::is_control));
}

fn capture_drawing_layout(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.layout = match local_name(element.name().as_ref()) {
        "anchor" => Some(DrawingLayout::Anchor),
        "inline" => Some(DrawingLayout::Inline),
        _ => drawing.layout,
    };
    drawing.distance_top_emu = unsigned_drawing_emu_attribute(element, "distT");
    drawing.distance_bottom_emu = unsigned_drawing_emu_attribute(element, "distB");
    drawing.distance_left_emu = unsigned_drawing_emu_attribute(element, "distL");
    drawing.distance_right_emu = unsigned_drawing_emu_attribute(element, "distR");
    if drawing.layout == Some(DrawingLayout::Anchor) {
        drawing.behind_document = optional_on_off_attribute(element, "behindDoc");
        drawing.layout_in_cell = optional_on_off_attribute(element, "layoutInCell");
        drawing.allow_overlap = optional_on_off_attribute(element, "allowOverlap");
        drawing.relative_height = attribute_by_local_name(element, "relativeHeight")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value <= u32::MAX.into());
    }
}

fn capture_drawing_simple_position(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.simple_x_emu = signed_drawing_emu_attribute(element, "x");
    drawing.simple_y_emu = signed_drawing_emu_attribute(element, "y");
}

fn begin_drawing_position(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    let axis = match local_name(element.name().as_ref()) {
        "positionH" => DrawingAxis::Horizontal,
        "positionV" => DrawingAxis::Vertical,
        _ => return,
    };
    let relative_from = attribute_by_local_name(element, "relativeFrom")
        .filter(|value| safe_drawing_relative_from(axis, value));
    match axis {
        DrawingAxis::Horizontal => drawing.horizontal_relative_from = relative_from,
        DrawingAxis::Vertical => drawing.vertical_relative_from = relative_from,
    }
    drawing.active_position_axis = Some(axis);
}

fn begin_drawing_text(name: &str, drawing: Option<&DrawingBuilder>) -> Option<PendingDrawingText> {
    let axis = drawing?.active_position_axis?;
    let kind = match name {
        "posOffset" => DrawingTextKind::Offset,
        "align" => DrawingTextKind::Align,
        _ => return None,
    };
    Some(PendingDrawingText {
        axis,
        kind,
        value: String::new(),
    })
}

fn finish_drawing_text(
    pending: Option<PendingDrawingText>,
    drawing: Option<&mut DrawingBuilder>,
    part: &str,
) -> Result<(), HcdError> {
    let (Some(pending), Some(drawing)) = (pending, drawing) else {
        return Ok(());
    };
    let value = pending.value.trim();
    match pending.kind {
        DrawingTextKind::Offset => {
            let offset = value.parse::<i64>().map_err(|_| {
                HcdError::InvalidBundle(format!(
                    "invalid DrawingML position offset {value:?} in {part}"
                ))
            })?;
            let offset = (-1_000_000_000..=1_000_000_000)
                .contains(&offset)
                .then_some(offset)
                .ok_or_else(|| {
                    HcdError::ResourceLimit(format!(
                        "DrawingML position offset in {part} exceeds the supported range"
                    ))
                })?;
            match pending.axis {
                DrawingAxis::Horizontal => drawing.horizontal_offset_emu = Some(offset),
                DrawingAxis::Vertical => drawing.vertical_offset_emu = Some(offset),
            }
        }
        DrawingTextKind::Align => {
            let align = safe_drawing_alignment(pending.axis, value)
                .then(|| value.to_string())
                .ok_or_else(|| {
                    HcdError::InvalidBundle(format!(
                        "invalid DrawingML alignment {value:?} in {part}"
                    ))
                })?;
            match pending.axis {
                DrawingAxis::Horizontal => drawing.horizontal_align = Some(align),
                DrawingAxis::Vertical => drawing.vertical_align = Some(align),
            }
        }
    }
    Ok(())
}

fn capture_drawing_wrap(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.wrap_kind = Some(
        match local_name(element.name().as_ref()) {
            "wrapSquare" => "square",
            "wrapTight" => "tight",
            "wrapThrough" => "through",
            "wrapTopAndBottom" => "top-and-bottom",
            "wrapNone" => "none",
            _ => return,
        }
        .to_string(),
    );
    drawing.wrap_side = attribute_by_local_name(element, "wrapText")
        .filter(|value| matches!(value.as_str(), "bothSides" | "left" | "right" | "largest"));
}

fn capture_textbox_transform(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.shape_rotation = attribute_by_local_name(element, "rot")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (-21_600_000..=21_600_000).contains(value));
}

fn capture_textbox_geometry(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.shape_geometry = attribute_by_local_name(element, "prst").filter(|value| {
        matches!(
            value.as_str(),
            "rect" | "roundRect" | "ellipse" | "triangle" | "rtTriangle"
        )
    });
}

fn capture_textbox_line(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.line_width_emu = unsigned_drawing_emu_attribute(element, "w");
}

fn capture_textbox_color(
    element: &BytesStart<'_>,
    drawing: Option<&mut DrawingBuilder>,
    state: &DrawingPropertyState,
    theme: &WordTheme,
) -> Option<DrawingColorTarget> {
    let drawing = drawing?;
    let qualified_name = element.name();
    let name = local_name(qualified_name.as_ref());
    let color = attribute_by_local_name(element, "val").and_then(|value| match name {
        "srgbClr" => normalized_hex_color(&value),
        "schemeClr" => theme.color(&value).map(str::to_string),
        _ => None,
    })?;
    let target = if state.line_solid_fill_depth.is_some() {
        drawing.line_color = Some(color);
        DrawingColorTarget::Line
    } else if state.gradient_fill_depth.is_some() {
        if drawing.gradient_colors.len() < 8 {
            drawing.gradient_colors.push(color);
        }
        DrawingColorTarget::Shape
    } else if state.shape_solid_fill_depth.is_some() {
        drawing.shape_fill = Some(color);
        DrawingColorTarget::Shape
    } else {
        return None;
    };
    Some(target)
}

fn capture_textbox_alpha(
    element: &BytesStart<'_>,
    drawing: Option<&mut DrawingBuilder>,
    target: Option<DrawingColorTarget>,
) {
    let (Some(drawing), Some(DrawingColorTarget::Shape)) = (drawing, target) else {
        return;
    };
    drawing.shape_fill_alpha = attribute_by_local_name(element, "val")
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value <= 100_000);
}

fn capture_textbox_gradient_angle(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.gradient_angle = attribute_by_local_name(element, "ang")
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (0..=21_600_000).contains(value));
}

fn capture_textbox_line_dash(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.line_dash = attribute_by_local_name(element, "val").filter(|value| {
        matches!(
            value.as_str(),
            "solid" | "dash" | "dashDot" | "dot" | "lgDash" | "lgDashDot"
        )
    });
}

fn capture_textbox_body_properties(element: &BytesStart<'_>, drawing: Option<&mut DrawingBuilder>) {
    let Some(drawing) = drawing else {
        return;
    };
    drawing.body_left_inset_emu = unsigned_drawing_emu_attribute(element, "lIns");
    drawing.body_top_inset_emu = unsigned_drawing_emu_attribute(element, "tIns");
    drawing.body_right_inset_emu = unsigned_drawing_emu_attribute(element, "rIns");
    drawing.body_bottom_inset_emu = unsigned_drawing_emu_attribute(element, "bIns");
    drawing.body_vertical = attribute_by_local_name(element, "vert").filter(|value| {
        matches!(
            value.as_str(),
            "horz" | "vert" | "vert270" | "eaVert" | "wordArtVert"
        )
    });
    drawing.body_anchor = attribute_by_local_name(element, "anchor")
        .filter(|value| matches!(value.as_str(), "t" | "ctr" | "b" | "just" | "dist"));
}

fn signed_drawing_emu_attribute(element: &BytesStart<'_>, name: &str) -> Option<i64> {
    attribute_by_local_name(element, name)
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (-1_000_000_000..=1_000_000_000).contains(value))
}

fn unsigned_drawing_emu_attribute(element: &BytesStart<'_>, name: &str) -> Option<u64> {
    attribute_by_local_name(element, name)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= 1_000_000_000)
}

fn optional_on_off_attribute(element: &BytesStart<'_>, name: &str) -> Option<bool> {
    attribute_by_local_name(element, name).map(|value| on_off_value(Some(&value)))
}

fn safe_drawing_relative_from(axis: DrawingAxis, value: &str) -> bool {
    match axis {
        DrawingAxis::Horizontal => matches!(
            value,
            "page"
                | "margin"
                | "column"
                | "character"
                | "leftMargin"
                | "rightMargin"
                | "insideMargin"
                | "outsideMargin"
        ),
        DrawingAxis::Vertical => matches!(
            value,
            "page"
                | "margin"
                | "paragraph"
                | "line"
                | "topMargin"
                | "bottomMargin"
                | "insideMargin"
                | "outsideMargin"
        ),
    }
}

fn safe_drawing_alignment(axis: DrawingAxis, value: &str) -> bool {
    match axis {
        DrawingAxis::Horizontal => {
            matches!(value, "left" | "right" | "center" | "inside" | "outside")
        }
        DrawingAxis::Vertical => {
            matches!(value, "top" | "bottom" | "center" | "inside" | "outside")
        }
    }
}

fn append_hyperlink_start(
    element: &BytesStart<'_>,
    relationships: &PartRelationships,
    paragraph: Option<&mut ParagraphBuilder>,
) -> bool {
    let Some(paragraph) = paragraph else {
        return false;
    };
    let relationship_target = attribute_by_local_name(element, "id")
        .and_then(|id| relationships.hyperlinks.get(&id).cloned());
    let anchor = attribute_by_local_name(element, "anchor")
        .filter(|value| value.chars().all(is_safe_fragment_character))
        .map(|value| format!("#{value}"));
    let href = relationship_target.or(anchor);
    if let Some(href) = href.as_deref().and_then(safe_hyperlink_href) {
        paragraph.html.push_str(&format!(
            "<a class=\"hcd-hyperlink\" href=\"{}\">",
            escape_attribute(href)
        ));
        true
    } else {
        paragraph
            .html
            .push_str("<span class=\"hcd-hyperlink hcd-hyperlink-blocked\">");
        false
    }
}

fn handle_complex_field_marker(
    element: &BytesStart<'_>,
    fields: &mut Vec<FieldFrame>,
    paragraph: Option<&mut ParagraphBuilder>,
    part: &str,
) -> Result<(), HcdError> {
    match attribute_by_local_name(element, "fldCharType").as_deref() {
        Some("begin") => fields.push(FieldFrame::default()),
        Some("separate") => {
            let field = fields.last_mut().ok_or_else(|| {
                HcdError::InvalidBundle(format!("field separator without begin in {part}"))
            })?;
            field.wrapper_is_anchor = append_field_hyperlink_start(&field.instruction, paragraph);
        }
        Some("end") => {
            let field = fields.pop().ok_or_else(|| {
                HcdError::InvalidBundle(format!("field end without begin in {part}"))
            })?;
            if field.simple {
                return Err(HcdError::InvalidBundle(format!(
                    "complex field end closed a simple field in {part}"
                )));
            }
            append_field_hyperlink_end(field.wrapper_is_anchor, paragraph);
        }
        _ => {}
    }
    Ok(())
}

fn append_field_hyperlink_start(
    instruction: &str,
    paragraph: Option<&mut ParagraphBuilder>,
) -> Option<bool> {
    let href = hyperlink_href_from_field_instruction(instruction)?;
    let paragraph = paragraph?;
    if let Some(href) = safe_hyperlink_href(&href) {
        paragraph.html.push_str(&format!(
            "<a class=\"hcd-hyperlink hcd-field-hyperlink\" href=\"{}\">",
            escape_attribute(href)
        ));
        Some(true)
    } else {
        paragraph
            .html
            .push_str("<span class=\"hcd-hyperlink hcd-field-hyperlink hcd-hyperlink-blocked\">");
        Some(false)
    }
}

fn append_field_hyperlink_end(
    wrapper_is_anchor: Option<bool>,
    paragraph: Option<&mut ParagraphBuilder>,
) {
    if let (Some(wrapper_is_anchor), Some(paragraph)) = (wrapper_is_anchor, paragraph) {
        paragraph
            .html
            .push_str(if wrapper_is_anchor { "</a>" } else { "</span>" });
    }
}

fn hyperlink_href_from_field_instruction(instruction: &str) -> Option<String> {
    let tokens = field_instruction_tokens(instruction);
    if !tokens
        .first()
        .is_some_and(|token| token.eq_ignore_ascii_case("HYPERLINK"))
    {
        return None;
    }
    let mut target = None;
    let mut anchor = None;
    let mut index = 1usize;
    while index < tokens.len() {
        let token = &tokens[index];
        if token.eq_ignore_ascii_case("\\l") {
            index += 1;
            anchor = tokens.get(index).cloned();
        } else if token.starts_with('\\') {
            // Word field switches such as \t and \o consume their following value.
            index += 1;
        } else if target.is_none() {
            target = Some(token.clone());
        }
        index += 1;
    }
    let safe_anchor = anchor.filter(|value| value.chars().all(is_safe_fragment_character));
    match (target, safe_anchor) {
        (Some(mut target), Some(anchor)) if !target.is_empty() => {
            target.push('#');
            target.push_str(&anchor);
            Some(target)
        }
        (Some(target), _) if !target.is_empty() => Some(target),
        (None, Some(anchor)) => Some(format!("#{anchor}")),
        _ => None,
    }
}

fn field_instruction_tokens(instruction: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in instruction.chars() {
        match character {
            '"' => quoted = !quoted,
            character if character.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn push_data_attribute(output: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        output.push(' ');
        output.push_str(name);
        output.push_str("=\"");
        output.push_str(&escape_attribute(value));
        output.push('"');
    }
}

fn on_off_value(value: Option<&str>) -> bool {
    !matches!(value, Some("0" | "false" | "off" | "no"))
}

fn signed_attribute(element: &BytesStart<'_>, name: &str) -> Option<i64> {
    attribute_by_local_name(element, name)
        .and_then(|value| value.parse().ok())
        .filter(|value| (-2_000_000..=2_000_000).contains(value))
}

fn push_twips_css(css: &mut Vec<String>, property: &str, value: Option<i64>) {
    if let Some(value) = value.filter(|value| (-2_000_000..=2_000_000).contains(value)) {
        css.push(format!("{property}:{:.2}pt", value as f64 / 20.0));
    }
}

fn html_alignment(value: &str) -> Option<&'static str> {
    match value {
        "left" | "start" => Some("left"),
        "right" | "end" => Some("right"),
        "center" => Some("center"),
        "both" | "distribute" => Some("justify"),
        _ => None,
    }
}

fn refresh_run_fonts(format: &mut RunFormat, theme: &WordTheme) {
    let latin = format.latin_font.clone().or_else(|| {
        format
            .latin_theme
            .as_deref()
            .and_then(|slot| theme.font(slot, format.latin_language.as_deref()))
            .map(str::to_string)
    });
    let east_asia = format.east_asia_font.clone().or_else(|| {
        format
            .east_asia_theme
            .as_deref()
            .and_then(|slot| theme.font(slot, format.east_asia_language.as_deref()))
            .map(str::to_string)
    });
    let bidi = format.bidi_font.clone().or_else(|| {
        format
            .bidi_theme
            .as_deref()
            .and_then(|slot| theme.font(slot, format.bidi_language.as_deref()))
            .map(str::to_string)
    });
    format.resolved_latin_font = latin;
    format.resolved_east_asia_font = east_asia;
    format.resolved_bidi_font = bidi;

    let east_asia_primary = format
        .east_asia_language
        .as_deref()
        .and_then(language_theme_script)
        .is_some_and(|script| matches!(script, "Hans" | "Hant" | "Jpan" | "Hang"));
    format.font = if format.rtl || format.bidi_language.is_some() {
        format
            .resolved_bidi_font
            .clone()
            .or_else(|| format.resolved_latin_font.clone())
            .or_else(|| format.resolved_east_asia_font.clone())
    } else if east_asia_primary {
        format
            .resolved_east_asia_font
            .clone()
            .or_else(|| format.resolved_latin_font.clone())
            .or_else(|| format.resolved_bidi_font.clone())
    } else {
        format
            .resolved_latin_font
            .clone()
            .or_else(|| format.resolved_east_asia_font.clone())
            .or_else(|| format.resolved_bidi_font.clone())
    };
}

fn is_safe_theme_script(value: &str) -> bool {
    value.len() == 4 && value.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn is_safe_language_tag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 35
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn language_theme_script(language: &str) -> Option<&'static str> {
    let normalized = language.replace('_', "-");
    for component in normalized.split('-').skip(1) {
        if component.eq_ignore_ascii_case("hans") {
            return Some("Hans");
        }
        if component.eq_ignore_ascii_case("hant") {
            return Some("Hant");
        }
        if component.eq_ignore_ascii_case("jpan") {
            return Some("Jpan");
        }
        if component.eq_ignore_ascii_case("hang") || component.eq_ignore_ascii_case("kore") {
            return Some("Hang");
        }
        if component.eq_ignore_ascii_case("arab") {
            return Some("Arab");
        }
        if component.eq_ignore_ascii_case("hebr") {
            return Some("Hebr");
        }
        if component.eq_ignore_ascii_case("deva") {
            return Some("Deva");
        }
    }
    let language = normalized.split('-').next()?.to_ascii_lowercase();
    match language.as_str() {
        "zh" => {
            let lower = normalized.to_ascii_lowercase();
            if lower.contains("-tw") || lower.contains("-hk") || lower.contains("-mo") {
                Some("Hant")
            } else {
                Some("Hans")
            }
        }
        "ja" => Some("Jpan"),
        "ko" => Some("Hang"),
        "ar" | "fa" | "ur" | "ps" | "sd" | "ug" => Some("Arab"),
        "he" | "iw" | "yi" => Some("Hebr"),
        "hi" | "mr" | "ne" | "sa" => Some("Deva"),
        "bn" | "as" => Some("Beng"),
        "pa" => Some("Guru"),
        "gu" => Some("Gujr"),
        "or" => Some("Orya"),
        "ta" => Some("Taml"),
        "te" => Some("Telu"),
        "kn" => Some("Knda"),
        "ml" => Some("Mlym"),
        "th" => Some("Thai"),
        "lo" => Some("Laoo"),
        "km" => Some("Khmr"),
        _ => None,
    }
}

fn resolve_run_color(element: &BytesStart<'_>, theme: &WordTheme) -> Option<String> {
    let slot = attribute_by_local_name(element, "themeColor")?;
    let base = theme.color(&slot)?;
    let shade_attribute = attribute_by_local_name(element, "themeShade");
    let tint_attribute = attribute_by_local_name(element, "themeTint");
    // Word materializes its own finite-precision HSL result in w:val. Prefer
    // that value for transformed colors, while retaining a bounded HSL
    // fallback for hand-authored documents that omit it.
    if shade_attribute.is_some() || tint_attribute.is_some() {
        if let Some(materialized) =
            attribute_by_local_name(element, "val").and_then(|value| normalized_hex_color(&value))
        {
            return Some(materialized);
        }
    }
    let shade = shade_attribute.as_deref().and_then(parse_hex_byte);
    let tint = tint_attribute.as_deref().and_then(parse_hex_byte);
    transform_theme_color(base, shade, tint)
}

fn transform_theme_color(base: &str, shade: Option<u8>, tint: Option<u8>) -> Option<String> {
    let normalized = normalized_hex_color(base)?;
    let red = f64::from(u8::from_str_radix(&normalized[0..2], 16).ok()?) / 255.0;
    let green = f64::from(u8::from_str_radix(&normalized[2..4], 16).ok()?) / 255.0;
    let blue = f64::from(u8::from_str_radix(&normalized[4..6], 16).ok()?) / 255.0;
    let (hue, saturation, mut luminance) = rgb_to_hsl(red, green, blue);
    // Word gives themeTint precedence if both transform attributes are present.
    if let Some(tint) = tint {
        let factor = f64::from(tint) / 255.0;
        luminance = luminance * factor + (1.0 - factor);
    } else if let Some(shade) = shade {
        luminance *= f64::from(shade) / 255.0;
    }
    let channels = hsl_to_rgb(hue, saturation, luminance);
    Some(format!(
        "{:02X}{:02X}{:02X}",
        channels[0], channels[1], channels[2]
    ))
}

fn rgb_to_hsl(red: f64, green: f64, blue: f64) -> (f64, f64, f64) {
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let luminance = (maximum + minimum) / 2.0;
    if (maximum - minimum).abs() < f64::EPSILON {
        return (0.0, 0.0, luminance);
    }
    let delta = maximum - minimum;
    let saturation = if luminance > 0.5 {
        delta / (2.0 - maximum - minimum)
    } else {
        delta / (maximum + minimum)
    };
    let mut hue = if (maximum - red).abs() < f64::EPSILON {
        (green - blue) / delta + if green < blue { 6.0 } else { 0.0 }
    } else if (maximum - green).abs() < f64::EPSILON {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    };
    hue /= 6.0;
    (hue, saturation, luminance)
}

fn hsl_to_rgb(hue: f64, saturation: f64, luminance: f64) -> [u8; 3] {
    let (red, green, blue) = if saturation.abs() < f64::EPSILON {
        (luminance, luminance, luminance)
    } else {
        let q = if luminance < 0.5 {
            luminance * (1.0 + saturation)
        } else {
            luminance + saturation - luminance * saturation
        };
        let p = 2.0 * luminance - q;
        (
            hue_to_rgb(p, q, hue + 1.0 / 3.0),
            hue_to_rgb(p, q, hue),
            hue_to_rgb(p, q, hue - 1.0 / 3.0),
        )
    };
    [
        (red.clamp(0.0, 1.0) * 255.0).round() as u8,
        (green.clamp(0.0, 1.0) * 255.0).round() as u8,
        (blue.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

fn hue_to_rgb(p: f64, q: f64, mut hue: f64) -> f64 {
    if hue < 0.0 {
        hue += 1.0;
    }
    if hue > 1.0 {
        hue -= 1.0;
    }
    if hue < 1.0 / 6.0 {
        p + (q - p) * 6.0 * hue
    } else if hue < 0.5 {
        q
    } else if hue < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - hue) * 6.0
    } else {
        p
    }
}

fn parse_hex_byte(value: &str) -> Option<u8> {
    (value.len() == 2)
        .then(|| u8::from_str_radix(value, 16).ok())
        .flatten()
}

fn normalized_hex_color(value: &str) -> Option<String> {
    strict_hex_color(value).map(str::to_ascii_uppercase)
}

fn strict_hex_color(value: &str) -> Option<&str> {
    (value.len() == 6 && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(value)
}

fn highlight_css_color(value: &str) -> Option<&'static str> {
    match value {
        "black" => Some("#000000"),
        "blue" => Some("#0000ff"),
        "cyan" => Some("#00ffff"),
        "green" => Some("#00ff00"),
        "magenta" => Some("#ff00ff"),
        "red" => Some("#ff0000"),
        "yellow" => Some("#ffff00"),
        "white" => Some("#ffffff"),
        "darkBlue" => Some("#000080"),
        "darkCyan" => Some("#008080"),
        "darkGreen" => Some("#008000"),
        "darkMagenta" => Some("#800080"),
        "darkRed" => Some("#800000"),
        "darkYellow" => Some("#808000"),
        "darkGray" => Some("#808080"),
        "lightGray" => Some("#c0c0c0"),
        _ => None,
    }
}

fn safe_font_family(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|character| character.is_alphanumeric() || " -_,.'".contains(character)))
    .then_some(value)
}

fn word_marker_line_height(font: Option<&str>) -> f64 {
    match font.unwrap_or("Calibri").to_ascii_lowercase().as_str() {
        "calibri" => 1.25,
        "times new roman" | "arial" => 1.15,
        "simsun" | "宋体" | "microsoft yahei" | "微软雅黑" => 1.30,
        "dengxian" | "等线" => 1.20,
        _ => 1.15,
    }
}

fn safe_hyperlink_href(value: &str) -> Option<&str> {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    (value.starts_with('#')
        || lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:"))
    .then_some(value)
}

fn is_safe_fragment_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '.' | ':')
}

fn finish_paragraph(
    document_id: &str,
    part: &str,
    mut paragraph: ParagraphBuilder,
    numbering: &NumberingCatalog,
    paragraph_numbering: &BTreeMap<String, (String, Option<String>)>,
    numbering_state: &mut NumberingState,
) -> RenderedBlock {
    let identity = paragraph
        .paragraph_id
        .clone()
        .unwrap_or_else(|| paragraph.ordinal.to_string());
    let block_id = stable_node_id(&[document_id, part, "paragraph", &identity]);
    let mut effective_format = paragraph.format.clone();
    if let Some((number_id, level)) = effective_format
        .style_id
        .as_deref()
        .and_then(|style_id| paragraph_numbering.get(style_id))
    {
        if effective_format.numbering_id.is_none() {
            effective_format.numbering_id = Some(number_id.clone());
        }
        if effective_format.numbering_level.is_none() {
            effective_format.numbering_level = level.clone().or_else(|| Some("0".to_string()));
        }
    }
    let style_class = paragraph
        .format
        .style_id
        .as_deref()
        .map(|style_id| format!(" {}", word_style_class(style_id)))
        .unwrap_or_default();
    let marker = numbering_state.render_marker_details(
        numbering,
        effective_format.numbering_id.as_deref(),
        effective_format.numbering_level.as_deref(),
    );
    if let Some(marker) = &marker {
        if effective_format.left_twips.is_none() {
            effective_format.left_twips = marker.definition.left_twips;
        }
        if effective_format.hanging_twips.is_none() && effective_format.first_line_twips.is_none() {
            effective_format.hanging_twips = marker.definition.hanging_twips;
        }
    }
    let format_attributes = paragraph_html_attributes(&effective_format);
    let is_empty = !paragraph.has_visible_text && marker.is_none() && paragraph.nested.is_empty();
    let empty_class = if is_empty { " hcd-empty-paragraph" } else { "" };
    let marker = marker.map(|marker| marker.html).unwrap_or_default();
    let mut requires_overflow_visible = false;
    let html = if paragraph.nested.is_empty() {
        format!(
            "<p class=\"hcd-paragraph{style_class}{empty_class}\" data-hcd-id=\"{}\"{}>{marker}{}</p>",
            escape_attribute(&block_id),
            format_attributes,
            paragraph.html
        )
    } else {
        let mut nested_html = String::new();
        let mut flow_html = String::new();
        let mut canvas_height_px = 0.0f64;
        let mut canvas_width_px = 0.0f64;
        for nested in paragraph.nested.drain(..) {
            requires_overflow_visible |= nested.requires_overflow_visible;
            if let Some(placement) = nested.visual_placement {
                canvas_height_px = canvas_height_px.max(placement.top_px + placement.height_px);
                canvas_width_px = canvas_width_px.max(placement.left_px + placement.width_px);
                nested_html.push_str(&nested.html);
            } else {
                flow_html.push_str(&nested.html);
            }
            paragraph.entries.extend(nested.entries);
        }
        let canvas = if nested_html.is_empty() {
            String::new()
        } else {
            format!(
                "<div class=\"hcd-textbox-canvas\" style=\"height:{:.2}px;width:{:.2}px\">{nested_html}</div>",
                canvas_height_px.max(1.0),
                canvas_width_px.max(1.0)
            )
        };
        format!(
            "<div class=\"hcd-paragraph-group\" data-hcd-id=\"{}\"><p class=\"hcd-paragraph{style_class}{empty_class}\"{}>{marker}{}</p>{canvas}{flow_html}</div>",
            escape_attribute(&block_id),
            format_attributes,
            paragraph.html
        )
    };
    RenderedBlock {
        html,
        entries: paragraph.entries,
        visual_placement: None,
        requires_overflow_visible,
    }
}

fn take_table_fragment(table: &mut TableBuilder) -> RenderedBlock {
    table.html.push_str("</tbody></table>");
    RenderedBlock {
        html: std::mem::take(&mut table.html),
        entries: std::mem::take(&mut table.entries),
        visual_placement: None,
        requires_overflow_visible: false,
    }
}

fn render_textbox(document_id: &str, part: &str, mut drawing: DrawingBuilder) -> RenderedBlock {
    let identity = drawing
        .drawing_id
        .map(|id| format!("drawing-{id}"))
        .unwrap_or_else(|| format!("drawing-{}", drawing.ordinal));
    let node_id = stable_node_id(&[document_id, part, "textbox", &identity]);
    let width_px = drawing
        .width_emu
        .map(|value| emu_to_px(value as i64))
        .unwrap_or(1.0)
        .max(1.0);
    let height_px = drawing
        .height_emu
        .map(|value| emu_to_px(value as i64))
        .unwrap_or(1.0)
        .max(1.0);
    let left_px = drawing
        .horizontal_offset_emu
        .or(drawing.simple_x_emu)
        .map(emu_to_px)
        .unwrap_or(0.0);
    let top_px = drawing
        .vertical_offset_emu
        .or(drawing.simple_y_emu)
        .map(emu_to_px)
        .unwrap_or(0.0);

    let mut content = String::new();
    let mut entries = Vec::new();
    for block in drawing.textbox_content.drain(..) {
        content.push_str(&block.html);
        entries.extend(block.entries);
    }

    let mut attributes = format!(
        " class=\"hcd-textbox hcd-drawing\" data-hcd-id=\"{}\" data-hcd-node-kind=\"textbox\" data-hcd-editable=\"false\" data-hcd-source-part=\"{}\" data-hcd-source-path=\"/drawing[{}]\"",
        escape_attribute(&node_id),
        escape_attribute(part),
        drawing.ordinal
    );
    push_data_number(&mut attributes, "data-hcd-drawing-id", drawing.drawing_id);
    push_data_number(&mut attributes, "data-hcd-width-emu", drawing.width_emu);
    push_data_number(&mut attributes, "data-hcd-height-emu", drawing.height_emu);
    push_data_number(
        &mut attributes,
        "data-hcd-position-h-offset-emu",
        drawing.horizontal_offset_emu,
    );
    push_data_number(
        &mut attributes,
        "data-hcd-position-v-offset-emu",
        drawing.vertical_offset_emu,
    );
    push_data_number(
        &mut attributes,
        "data-hcd-relative-height",
        drawing.relative_height,
    );
    push_data_bool(
        &mut attributes,
        "data-hcd-behind-document",
        drawing.behind_document,
    );
    push_data_attribute(
        &mut attributes,
        "data-hcd-shape-geometry",
        drawing.shape_geometry.as_deref(),
    );
    push_data_attribute(
        &mut attributes,
        "data-hcd-text-direction",
        drawing.body_vertical.as_deref(),
    );

    let mut style = vec![
        "position:absolute".to_string(),
        format!("left:{left_px:.2}px"),
        format!("top:{top_px:.2}px"),
        format!("width:{width_px:.2}px"),
        format!("height:{height_px:.2}px"),
        "overflow:hidden".to_string(),
    ];
    if let Some(relative_height) = drawing.relative_height {
        style.push(format!("z-index:{}", relative_height.min(i32::MAX as u64)));
    }
    if let Some(rotation) = drawing.shape_rotation {
        style.push(format!(
            "transform:rotate({:.3}deg)",
            rotation as f64 / 60_000.0
        ));
        style.push("transform-origin:center".to_string());
    }
    match drawing.shape_geometry.as_deref() {
        Some("roundRect") => style.push("border-radius:12px".to_string()),
        Some("ellipse") => style.push("border-radius:50%".to_string()),
        _ => {}
    }
    if drawing.no_shape_fill {
        style.push("background-color:transparent".to_string());
    } else if drawing.gradient_colors.len() >= 2 {
        let angle = drawing
            .gradient_angle
            .map(|value| value as f64 / 60_000.0 + 90.0)
            .unwrap_or(90.0);
        style.push(format!(
            "background-image:linear-gradient({angle:.2}deg,#{},#{})",
            drawing.gradient_colors[0],
            drawing.gradient_colors[drawing.gradient_colors.len() - 1]
        ));
    } else if let Some(fill) = &drawing.shape_fill {
        if let Some(alpha) = drawing.shape_fill_alpha.filter(|value| *value < 100_000) {
            if let Some(color) = css_hex_alpha(fill, alpha) {
                style.push(format!("background-color:{color}"));
            } else {
                style.push(format!("background-color:#{fill}"));
            }
        } else {
            style.push(format!("background-color:#{fill}"));
        }
    }
    if drawing.no_line {
        for side in ["top", "right", "bottom", "left"] {
            style.push(format!("border-{side}:none"));
        }
    } else if drawing.line_color.is_some() || drawing.line_width_emu.is_some() {
        let width_pt = drawing.line_width_emu.unwrap_or(12_700) as f64 / 12_700.0;
        let dash = match drawing.line_dash.as_deref() {
            Some("dash" | "lgDash") => "dashed",
            Some("dot") => "dotted",
            Some("dashDot" | "lgDashDot") => "dashed",
            _ => "solid",
        };
        for side in ["top", "right", "bottom", "left"] {
            style.push(format!(
                "border-{side}:{width_pt:.2}pt {dash} #{}",
                drawing.line_color.as_deref().unwrap_or("000000")
            ));
        }
    } else {
        for side in ["top", "right", "bottom", "left"] {
            style.push(format!("border-{side}:none"));
        }
    }
    if drawing.has_outer_shadow {
        style.push("box-shadow:4px 4px 8px rgba(0,0,0,.35)".to_string());
    }
    let insets = [
        drawing.body_top_inset_emu.unwrap_or(0),
        drawing.body_right_inset_emu.unwrap_or(0),
        drawing.body_bottom_inset_emu.unwrap_or(0),
        drawing.body_left_inset_emu.unwrap_or(0),
    ];
    for (side, inset) in ["top", "right", "bottom", "left"].into_iter().zip(insets) {
        style.push(format!("padding-{side}:{:.2}px", emu_to_px(inset as i64)));
    }

    let mut content_style = vec!["height:100%".to_string()];
    match drawing.body_vertical.as_deref() {
        Some("vert" | "eaVert" | "wordArtVert") => {
            content_style.push("writing-mode:vertical-rl".to_string());
            content_style.push("text-orientation:mixed".to_string());
        }
        Some("vert270") => {
            content_style.push("writing-mode:vertical-lr".to_string());
            content_style.push("text-orientation:mixed".to_string());
        }
        _ => {}
    }
    if matches!(drawing.body_anchor.as_deref(), Some("ctr" | "b")) {
        content_style.push("display:flex".to_string());
        content_style.push("flex-direction:column".to_string());
        content_style.push(format!(
            "justify-content:{}",
            if drawing.body_anchor.as_deref() == Some("b") {
                "flex-end"
            } else {
                "center"
            }
        ));
    }
    attributes.push_str(" style=\"");
    attributes.push_str(&style.join(";"));
    attributes.push('"');
    let html = format!(
        "<aside{attributes}><div class=\"hcd-textbox-content\" style=\"{}\">{content}</div></aside>",
        content_style.join(";")
    );
    RenderedBlock {
        html,
        entries,
        visual_placement: Some(VisualPlacement {
            left_px,
            top_px,
            width_px,
            height_px,
        }),
        requires_overflow_visible: true,
    }
}

fn css_hex_alpha(hex: &str, alpha: u32) -> Option<String> {
    strict_hex_color(hex)?;
    let alpha = ((alpha.min(100_000) as f64 / 100_000.0) * 255.0).round() as u8;
    Some(format!("#{hex}{alpha:02X}"))
}

fn append_image(
    element: &BytesStart<'_>,
    document_id: &str,
    part: &str,
    image_ordinal: u64,
    relationships: &PartRelationships,
    drawing: Option<&DrawingBuilder>,
    paragraph: Option<&mut ParagraphBuilder>,
) {
    let Some(paragraph) = paragraph else {
        return;
    };
    let Some(relationship_id) = attribute_by_local_name(element, "embed") else {
        return;
    };
    if let Some(asset) = relationships.assets.get(&relationship_id) {
        let source_identity = drawing
            .and_then(|drawing| drawing.drawing_id)
            .map(|drawing_id| format!("drawing-{drawing_id}"))
            .unwrap_or_else(|| format!("image-{image_ordinal}-{relationship_id}"));
        let node_id = stable_node_id(&[document_id, part, "image", &source_identity]);
        let mut classes = vec!["hcd-drawing"];
        if let Some(layout) = drawing.and_then(|drawing| drawing.layout) {
            classes.push(match layout {
                DrawingLayout::Inline => "hcd-drawing-inline",
                DrawingLayout::Anchor => "hcd-drawing-anchor",
            });
        }
        if let Some(wrap_class) = drawing.and_then(drawing_wrap_class) {
            classes.push(wrap_class);
        }
        let mut attributes = format!(
            " class=\"{}\" data-hcd-id=\"{}\" data-hcd-node-kind=\"image\" data-hcd-editable=\"false\" data-hcd-source-part=\"{}\" data-hcd-source-path=\"/drawing[{}]\" src=\"asset://sha256/{}\" data-hcd-asset-href=\"{}\"",
            classes.join(" "),
            escape_attribute(&node_id),
            escape_attribute(part),
            image_ordinal,
            asset.hash,
            escape_attribute(&asset.href)
        );
        let alt = drawing
            .and_then(|drawing| drawing.alt.as_deref())
            .unwrap_or("");
        attributes.push_str(" alt=\"");
        attributes.push_str(&escape_attribute(alt));
        attributes.push('"');
        if let Some(drawing) = drawing {
            push_data_attribute(
                &mut attributes,
                "data-hcd-drawing-layout",
                drawing.layout.map(DrawingLayout::as_str),
            );
            push_data_attribute(
                &mut attributes,
                "data-hcd-position-h-relative-from",
                drawing.horizontal_relative_from.as_deref(),
            );
            push_data_attribute(
                &mut attributes,
                "data-hcd-position-v-relative-from",
                drawing.vertical_relative_from.as_deref(),
            );
            push_data_attribute(
                &mut attributes,
                "data-hcd-position-h-align",
                drawing.horizontal_align.as_deref(),
            );
            push_data_attribute(
                &mut attributes,
                "data-hcd-position-v-align",
                drawing.vertical_align.as_deref(),
            );
            push_data_attribute(
                &mut attributes,
                "data-hcd-wrap",
                drawing.wrap_kind.as_deref(),
            );
            push_data_attribute(
                &mut attributes,
                "data-hcd-wrap-side",
                drawing.wrap_side.as_deref(),
            );
            push_data_number(&mut attributes, "data-hcd-drawing-id", drawing.drawing_id);
            push_data_number(&mut attributes, "data-hcd-width-emu", drawing.width_emu);
            push_data_number(&mut attributes, "data-hcd-height-emu", drawing.height_emu);
            push_data_number(
                &mut attributes,
                "data-hcd-position-h-offset-emu",
                drawing.horizontal_offset_emu,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-position-v-offset-emu",
                drawing.vertical_offset_emu,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-simple-x-emu",
                drawing.simple_x_emu,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-simple-y-emu",
                drawing.simple_y_emu,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-relative-height",
                drawing.relative_height,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-distance-top-emu",
                drawing.distance_top_emu,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-distance-bottom-emu",
                drawing.distance_bottom_emu,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-distance-left-emu",
                drawing.distance_left_emu,
            );
            push_data_number(
                &mut attributes,
                "data-hcd-distance-right-emu",
                drawing.distance_right_emu,
            );
            push_data_bool(
                &mut attributes,
                "data-hcd-behind-document",
                drawing.behind_document,
            );
            push_data_bool(
                &mut attributes,
                "data-hcd-layout-in-cell",
                drawing.layout_in_cell,
            );
            push_data_bool(
                &mut attributes,
                "data-hcd-allow-overlap",
                drawing.allow_overlap,
            );
        }
        let mut style = Vec::new();
        let width_px = drawing
            .and_then(|drawing| drawing.width_emu)
            .filter(|value| (1..=100_000_000).contains(value))
            .map(|value| value as f64 * 96.0 / 914_400.0);
        let height_px = drawing
            .and_then(|drawing| drawing.height_emu)
            .filter(|value| (1..=100_000_000).contains(value))
            .map(|value| value as f64 * 96.0 / 914_400.0);
        if let Some(width) = width_px {
            style.push(format!("width:{width:.2}px"));
        }
        if let Some(height) = height_px {
            style.push(format!("height:{height:.2}px"));
        }
        if drawing.and_then(|drawing| drawing.layout) == Some(DrawingLayout::Anchor) {
            style.push("position:relative".to_string());
            let horizontal =
                drawing.and_then(|drawing| drawing.horizontal_offset_emu.or(drawing.simple_x_emu));
            let vertical =
                drawing.and_then(|drawing| drawing.vertical_offset_emu.or(drawing.simple_y_emu));
            if let Some(left) = horizontal
                .filter(|value| (-100_000_000..=100_000_000).contains(value))
                .map(emu_to_px)
            {
                style.push(format!("left:{left:.2}px"));
            }
            if let Some(top) = vertical
                .filter(|value| (-100_000_000..=100_000_000).contains(value))
                .map(emu_to_px)
            {
                style.push(format!("top:{top:.2}px"));
            }
        }
        if !style.is_empty() {
            attributes.push_str(" style=\"");
            attributes.push_str(&style.join(";"));
            attributes.push('"');
        }
        paragraph.html.push_str(&format!("<img{attributes}/>"));
    }
}

fn drawing_wrap_class(drawing: &DrawingBuilder) -> Option<&'static str> {
    match drawing.wrap_kind.as_deref() {
        Some("top-and-bottom") => Some("hcd-drawing-wrap-top-bottom"),
        Some("square" | "tight" | "through") => match drawing.wrap_side.as_deref() {
            Some("left") => Some("hcd-drawing-wrap-left"),
            Some("right") => Some("hcd-drawing-wrap-right"),
            _ if drawing.horizontal_align.as_deref() == Some("right") => {
                Some("hcd-drawing-wrap-left")
            }
            _ => Some("hcd-drawing-wrap-both"),
        },
        _ => None,
    }
}

fn push_data_number<T: std::fmt::Display>(output: &mut String, name: &str, value: Option<T>) {
    if let Some(value) = value {
        output.push(' ');
        output.push_str(name);
        output.push_str("=\"");
        output.push_str(&value.to_string());
        output.push('"');
    }
}

fn push_data_bool(output: &mut String, name: &str, value: Option<bool>) {
    if let Some(value) = value {
        push_data_attribute(output, name, Some(if value { "true" } else { "false" }));
    }
}

fn push_inline_style_attribute(output: &mut String, declarations: &[String]) {
    if declarations.is_empty() {
        return;
    }
    output.push_str(" style=\"");
    output.push_str(&escape_attribute(&declarations.join(";")));
    output.push('"');
}

fn emu_to_px(value: i64) -> f64 {
    value as f64 * 96.0 / 914_400.0
}

fn ordered_text_parts(archive: &StreamingOxmlArchive) -> Vec<(String, String)> {
    let mut parts = vec![("word/document.xml".to_string(), "body".to_string())];
    let mut extras: Vec<(String, String)> = archive
        .entries()
        .iter()
        .filter_map(|entry| {
            let region = if entry.name.starts_with("word/header") && entry.name.ends_with(".xml") {
                "header"
            } else if entry.name.starts_with("word/footer") && entry.name.ends_with(".xml") {
                "footer"
            } else if entry.name == "word/footnotes.xml" {
                "footnote"
            } else if entry.name == "word/endnotes.xml" {
                "endnote"
            } else if entry.name == "word/comments.xml" {
                "comment"
            } else {
                return None;
            };
            Some((entry.name.clone(), region.to_string()))
        })
        .collect();
    extras.sort();
    parts.extend(extras);
    parts
}

fn referenced_media_parts(
    archive: &mut StreamingOxmlArchive,
    text_parts: &[(String, String)],
) -> Result<HashSet<String>, HcdError> {
    let mut referenced = HashSet::new();
    for (source_part, _) in text_parts {
        let rels_path = relationship_part_path(source_part);
        if !archive.contains(&rels_path) {
            continue;
        }
        let xml = archive
            .read_control_part(&rels_path, MAX_CONTROL_PART_BYTES)
            .map_err(package_error)?;
        let mut reader = Reader::from_reader(xml.as_slice());
        reader.config_mut().check_end_names = true;
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer) {
                Ok(Event::Empty(ref element)) | Ok(Event::Start(ref element))
                    if local_name(element.name().as_ref()) == "Relationship" =>
                {
                    let external = attribute_by_local_name(element, "TargetMode")
                        .is_some_and(|mode| mode.eq_ignore_ascii_case("External"));
                    if !external {
                        if let Some(target) = attribute_by_local_name(element, "Target") {
                            let resolved = resolve_relationship_target(source_part, &target)?;
                            if resolved.starts_with("word/media/") && archive.contains(&resolved) {
                                referenced.insert(resolved);
                            }
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(error) => {
                    return Err(HcdError::InvalidBundle(format!(
                        "invalid relationships {rels_path}: {error}"
                    )))
                }
            }
            buffer.clear();
        }
    }
    Ok(referenced)
}

fn load_relationships(
    archive: &mut StreamingOxmlArchive,
    source_part: &str,
    assets: &HashMap<String, AssetRecord>,
) -> Result<PartRelationships, HcdError> {
    let rels_path = relationship_part_path(source_part);
    if !archive.contains(&rels_path) {
        return Ok(PartRelationships::default());
    }
    let xml = archive
        .read_control_part(&rels_path, MAX_CONTROL_PART_BYTES)
        .map_err(package_error)?;
    let mut reader = Reader::from_reader(xml.as_slice());
    let mut buffer = Vec::new();
    let mut relationships = PartRelationships::default();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Empty(ref element)) | Ok(Event::Start(ref element))
                if local_name(element.name().as_ref()) == "Relationship" =>
            {
                let id = attribute_by_local_name(element, "Id");
                let target = attribute_by_local_name(element, "Target");
                let external = attribute_by_local_name(element, "TargetMode")
                    .is_some_and(|mode| mode.eq_ignore_ascii_case("External"));
                if let (Some(id), Some(target)) = (id, target) {
                    if external {
                        relationships.hyperlinks.insert(id, target);
                    } else if let Ok(resolved) = resolve_relationship_target(source_part, &target) {
                        if let Some(asset) = assets.get(&resolved) {
                            relationships.assets.insert(id, asset.clone());
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(HcdError::InvalidBundle(format!(
                    "invalid relationships {rels_path}: {error}"
                )))
            }
        }
        buffer.clear();
    }
    Ok(relationships)
}

fn relationship_part_path(source_part: &str) -> String {
    let path = Path::new(source_part);
    let parent = path.parent().and_then(Path::to_str).unwrap_or("");
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if parent.is_empty() {
        format!("_rels/{name}.rels")
    } else {
        format!("{parent}/_rels/{name}.rels")
    }
}

fn resolve_relationship_target(source_part: &str, target: &str) -> Result<String, HcdError> {
    let source_parent = Path::new(source_part)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let combined = if target.starts_with('/') {
        PathBuf::from(target.trim_start_matches('/'))
    } else {
        source_parent.join(target)
    };
    let mut normalized = PathBuf::new();
    for component in combined.components() {
        match component {
            Component::Normal(value) => normalized.push(value),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(HcdError::InvalidBundle(format!(
                        "relationship escapes package: {target}"
                    )));
                }
            }
            Component::CurDir => {}
            _ => {
                return Err(HcdError::InvalidBundle(format!(
                    "unsafe relationship target: {target}"
                )))
            }
        }
    }
    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

fn write_asset_index(root: &Path, assets: &[AssetRecord]) -> Result<(), HcdError> {
    let directory = root.join("assets");
    std::fs::create_dir_all(&directory)?;
    let file = std::fs::File::create(directory.join("index.json"))?;
    serde_json::to_writer(file, assets)?;
    Ok(())
}

fn attribute_by_local_name(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .with_checks(false)
        .filter_map(Result::ok)
        .find(|attribute| local_name(attribute.key.as_ref()) == name)
        .and_then(|attribute| {
            attribute
                .unescape_value()
                .ok()
                .map(|value| value.into_owned())
        })
}

fn local_name(name: &[u8]) -> &str {
    let local = name
        .iter()
        .rposition(|byte| *byte == b':')
        .map(|index| &name[index + 1..])
        .unwrap_or(name);
    std::str::from_utf8(local).unwrap_or("")
}

fn package_error(error: PackageError) -> HcdError {
    match error {
        PackageError::ResourceLimit(message) => HcdError::ResourceLimit(message),
        other => HcdError::InvalidBundle(other.to_string()),
    }
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attribute(text: &str) -> String {
    escape_text(text)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn default_styles() -> &'static str {
    r#"article,.hcd-chunk{display:block}.hcd-chunk-overflow{content-visibility:visible;contain:none;overflow:visible}.hcd-paragraph{white-space:pre-wrap;min-height:1em;margin:0}.hcd-list-marker{display:inline-block;min-width:2em;white-space:nowrap}.hcd-table{border-collapse:collapse;margin:.6em 0}.hcd-table td{padding:.25em .4em;vertical-align:top}.hcd-paragraph-group{min-width:0}.hcd-textbox-canvas{position:relative;max-width:100%}.hcd-textbox{box-sizing:border-box;margin:0}.hcd-textbox-content>.hcd-table{margin:0;max-width:100%}.hcd-revision-insert{text-decoration:underline}.hcd-revision-delete{text-decoration:line-through;opacity:.65}.hcd-chunk img{max-width:100%;height:auto}.hcd-drawing-wrap-left{float:right}.hcd-drawing-wrap-right,.hcd-drawing-wrap-both{float:left}.hcd-drawing-wrap-top-bottom{display:block;clear:both}body:not([data-hcd-image-hitboxes=\"off\"]) img.hcd-drawing[data-hcd-id]{cursor:crosshair}body:not([data-hcd-image-hitboxes=\"off\"]) img.hcd-drawing[data-hcd-id]:hover{outline:2px solid rgba(255,59,48,.95);outline-offset:1px}body:not([data-hcd-text-hitboxes=\"off\"]) [data-hcd-node-hash]:hover{background:rgba(10,132,255,.12);outline:1px solid rgba(10,132,255,.8)}"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use zip::write::SimpleFileOptions;

    #[test]
    fn flattened_legacy_hyperlink_fields_render_results_and_block_scripts() {
        let source = "案由： HYPERLINK \"https://example.test/law?a=1&b=2\" \\l \"article-31\" \\t \"_blank\" 法条> HYPERLINK \"javascript:void(0)\" 注";
        let (canonical, html) = render_legacy_hyperlink_text(source).unwrap();
        assert_eq!(canonical, "案由：  法条>  注");
        assert!(html.contains("href=\"https://example.test/law?a=1&amp;b=2#article-31\""));
        assert!(html.contains("> 法条&gt; </a>"));
        assert!(html.contains("hcd-hyperlink-blocked\"> 注</a>"));
        assert!(!html.contains("javascript:"));
        assert!(!html.contains("HYPERLINK"));
        let hash = hash_bytes(canonical.as_bytes());
        let fragment = format!(
            "<span data-hcd-id=\"n_0123456789abcdef0123456789abcdef\" data-hcd-node-hash=\"{hash}\">{html}</span>"
        );
        let extracted = hcd_core::extract_html_text_nodes(&fragment).unwrap();
        assert_eq!(extracted["n_0123456789abcdef0123456789abcdef"], canonical);
    }

    #[test]
    fn word_theme_resolves_font_slots_colors_tint_and_shade() {
        let xml = r#"<a:theme xmlns:a="a"><a:themeElements><a:clrScheme><a:dk1><a:sysClr val="windowText" lastClr="010203"/></a:dk1><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="C0504D"/></a:accent2></a:clrScheme><a:fontScheme><a:majorFont><a:latin typeface="Aptos Display"/><a:ea typeface=""/><a:cs typeface="Noto Sans Arabic"/><a:font script="Hans" typeface="等线"/><a:font script="Hant" typeface="新細明體"/></a:majorFont><a:minorFont><a:latin typeface="Aptos"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#.as_bytes();
        let theme = parse_word_theme(xml, "word/theme/theme1.xml").unwrap();
        assert_eq!(theme.font("majorHAnsi", None), Some("Aptos Display"));
        assert_eq!(theme.font("majorEastAsia", Some("zh-CN")), Some("等线"));
        assert_eq!(
            theme.font("majorEastAsia", Some("zh-Hant")),
            Some("新細明體")
        );
        assert_eq!(theme.font("majorEastAsia", None), Some("Aptos Display"));
        assert_eq!(
            theme.font("majorBidi", Some("ar-SA")),
            Some("Noto Sans Arabic")
        );
        assert_eq!(theme.font("minorAscii", None), Some("Aptos"));
        assert_eq!(theme.color("text1"), Some("010203"));
        assert_eq!(theme.color("accent1"), Some("4472C4"));
        assert_eq!(
            transform_theme_color("C0504D", Some(0xBF), None).as_deref(),
            Some("953735")
        );
        assert_eq!(
            transform_theme_color("4F81BD", None, Some(0x99)).as_deref(),
            Some("95B3D7")
        );
        let mut color_reader = Reader::from_str(
            r#"<w:color xmlns:w="w" w:val="943634" w:themeColor="accent2" w:themeShade="BF"/>"#,
        );
        let color = match color_reader.read_event().unwrap() {
            Event::Empty(element) => resolve_run_color(&element, &theme),
            other => panic!("unexpected color event: {other:?}"),
        };
        assert_eq!(color.as_deref(), Some("943634"));
        assert_eq!(
            transform_theme_color("4F81BD", Some(0x10), Some(0x99)).as_deref(),
            Some("95B3D7")
        );
    }

    #[test]
    fn word_theme_rejects_excessive_xml_depth() {
        let mut xml = String::from("<a:theme xmlns:a=\"a\">");
        for _ in 0..=MAX_XML_DEPTH {
            xml.push_str("<a:x>");
        }
        for _ in 0..=MAX_XML_DEPTH {
            xml.push_str("</a:x>");
        }
        xml.push_str("</a:theme>");
        let error = parse_word_theme(xml.as_bytes(), "word/theme/theme1.xml").unwrap_err();
        assert!(matches!(error, HcdError::ResourceLimit(_)));
    }

    #[test]
    fn word_styles_reject_excessive_xml_depth_before_css_generation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("deep-styles.docx");
        let mut xml = String::from("<w:styles xmlns:w=\"w\">");
        for _ in 0..=MAX_XML_DEPTH {
            xml.push_str("<w:x>");
        }
        for _ in 0..=MAX_XML_DEPTH {
            xml.push_str("</w:x>");
        }
        xml.push_str("</w:styles>");
        let file = std::fs::File::create(&source).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("word/styles.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap();

        let mut archive = StreamingOxmlArchive::open(&source).unwrap();
        let error = load_word_styles(&mut archive, &WordTheme::default()).unwrap_err();
        assert!(matches!(error, HcdError::ResourceLimit(_)));
    }

    #[test]
    fn word_styles_reject_excessive_table_band_sizes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("band-size-styles.docx");
        let xml = format!(
            "<w:styles xmlns:w=\"w\"><w:style w:type=\"table\" w:styleId=\"Unsafe\"><w:tblPr><w:tblStyleColBandSize w:val=\"{}\"/></w:tblPr></w:style></w:styles>",
            MAX_TABLE_BAND_SIZE + 1
        );
        let file = std::fs::File::create(&source).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("word/styles.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
        zip.finish().unwrap();

        let mut archive = StreamingOxmlArchive::open(&source).unwrap();
        let error = load_word_styles(&mut archive, &WordTheme::default()).unwrap_err();

        assert!(matches!(error, HcdError::ResourceLimit(_)));
        assert!(error.to_string().contains("tblStyleColBandSize"));
    }

    #[test]
    fn default_paragraph_style_applies_auto_line_spacing_to_unstyled_paragraphs() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("default-paragraph-style.docx");
        let styles_xml = r#"<w:styles xmlns:w="w">
          <w:docDefaults><w:pPrDefault/></w:docDefaults>
          <w:style w:type="paragraph" w:styleId="Normal" w:default="true">
            <w:pPr><w:spacing w:after="0" w:line="276" w:lineRule="auto"/></w:pPr>
          </w:style>
        </w:styles>"#;
        let file = std::fs::File::create(&source).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("word/styles.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(styles_xml.as_bytes()).unwrap();
        zip.finish().unwrap();

        let mut archive = StreamingOxmlArchive::open(&source).unwrap();
        let rendered = load_word_styles(&mut archive, &WordTheme::default()).unwrap();

        assert!(
            rendered
                .css
                .contains(".hcd-paragraph{margin-bottom:0.00pt;line-height:1.1500}"),
            "{}",
            rendered.css
        );
        assert!(!rendered.css.contains("line-height:13.80pt"));
        assert!(rendered
            .css
            .contains(".hcd-empty-paragraph{min-height:1lh}"));
        assert!(rendered
            .css
            .contains(".hcd-list-marker{box-sizing:border-box;text-indent:0}"));
    }

    #[test]
    fn paragraph_line_rule_distinguishes_auto_from_fixed_spacing() {
        let auto = ParagraphFormat {
            line_twips: Some(360),
            line_rule: Some("auto".to_string()),
            ..Default::default()
        };
        let exact = ParagraphFormat {
            line_twips: Some(360),
            line_rule: Some("exact".to_string()),
            ..Default::default()
        };

        assert_eq!(
            paragraph_css_declarations(&auto),
            vec!["line-height:1.5000"]
        );
        assert_eq!(
            paragraph_css_declarations(&exact),
            vec!["line-height:18.00pt"]
        );
    }

    #[test]
    fn default_docx_table_css_does_not_invent_grid_borders() {
        let css = default_styles();
        let cell_rule = css
            .split(".hcd-table td{")
            .nth(1)
            .and_then(|tail| tail.split('}').next())
            .unwrap();

        assert!(!cell_rule.contains("border"), "{cell_rule}");
        assert!(cell_rule.contains("padding:.25em .4em"));
        assert!(
            css.contains(".hcd-list-marker{display:inline-block;min-width:2em;white-space:nowrap}")
        );
    }

    #[test]
    fn linked_and_conditional_table_styles_generate_scoped_css() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("table-styles.docx");
        let styles_xml = r#"<w:styles xmlns:w="w">
          <w:style w:type="paragraph" w:styleId="HeadingBase"><w:rPr><w:b/><w:color w:val="112233"/></w:rPr></w:style>
          <w:style w:type="character" w:styleId="HeadingChar"><w:link w:val="HeadingBase"/><w:rPr><w:i/></w:rPr></w:style>
          <w:style w:type="character" w:styleId="CycleA"><w:link w:val="CycleB"/><w:rPr><w:b/></w:rPr></w:style>
          <w:style w:type="character" w:styleId="CycleB"><w:link w:val="CycleA"/><w:rPr><w:i/></w:rPr></w:style>
          <w:style w:type="table" w:styleId="BaseTable"><w:tblPr><w:tblStyleRowBandSize w:val="2"/><w:tblStyleColBandSize w:val="3"/><w:tblW w:type="pct" w:w="5000"/><w:jc w:val="center"/><w:tblBorders><w:top w:val="single" w:sz="8" w:color="445566"/><w:insideH w:val="dotted" w:sz="4" w:color="778899"/></w:tblBorders><w:tblCellMar><w:top w:w="100"/><w:start w:w="120"/></w:tblCellMar></w:tblPr><w:tcPr><w:shd w:fill="F2F2F2"/><w:vAlign w:val="center"/></w:tcPr><w:rPr><w:color w:val="112233"/></w:rPr></w:style>
          <w:style w:type="table" w:styleId="FancyTable"><w:basedOn w:val="BaseTable"/><w:tblStylePr w:type="firstRow"><w:rPr><w:b/><w:color w:val="FFFFFF"/><w:rPrChange><w:rPr><w:color w:val="000000"/></w:rPr></w:rPrChange></w:rPr><w:tcPr><w:shd w:fill="4472C4"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="band1Horz"><w:tcPr><w:shd w:fill="DDEBF7"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="band1Vert"><w:tcPr><w:shd w:fill="E2F0D9"/></w:tcPr></w:tblStylePr><w:tblStylePr w:type="firstCol"><w:rPr><w:i/></w:rPr></w:tblStylePr><w:tblStylePr w:type="nwCell"><w:tcPr><w:shd w:fill="FF0000"/></w:tcPr></w:tblStylePr></w:style>
        </w:styles>"#;
        let file = std::fs::File::create(&source).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("word/styles.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(styles_xml.as_bytes()).unwrap();
        zip.finish().unwrap();
        let mut archive = StreamingOxmlArchive::open(&source).unwrap();

        let rendered = load_word_styles(&mut archive, &WordTheme::default()).unwrap();
        let css = &rendered.css;

        let linked_class = word_style_class("HeadingChar");
        let table_class = word_style_class("FancyTable");
        assert!(css.contains(&format!(
            ".{linked_class}{{font-weight:700;color:#112233;font-style:italic}}"
        )));
        assert!(css.contains(&format!(
            ".{}{{font-style:italic;font-weight:700}}",
            word_style_class("CycleA")
        )));
        assert!(css.contains(&format!(
            ".{}{{font-weight:700;font-style:italic}}",
            word_style_class("CycleB")
        )));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}{{width:100.00%;margin-left:auto;margin-right:auto"
        )));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}>tbody>tr>td{{border-top:1.00pt solid #445566"
        )));
        assert!(css.contains("padding-top:5.00pt"));
        assert!(css.contains("padding-left:6.00pt"));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}[data-hcd-look-first-row=\"true\"]>tbody>tr:first-child>td"
        )));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}>tbody>tr[data-hcd-cnf-first-row=\"true\"]>td{{"
        )));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}>tbody>tr>td[data-hcd-cnf-first-row=\"true\"]{{"
        )));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}>tbody>tr>td .hcd-paragraph[data-hcd-cnf-first-row=\"true\"]{{"
        )));
        assert!(css.contains("background-color:#4472C4"));
        assert!(css.contains("color:#FFFFFF"));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}[data-hcd-look-h-band=\"true\"]>tbody>tr[data-hcd-row-band=\"1\"]>td"
        )));
        assert!(css.contains("background-color:#DDEBF7"));
        assert!(css.contains(&format!(
            ".hcd-table.{table_class}[data-hcd-look-v-band=\"true\"]>tbody>tr>td[data-hcd-column-band=\"1\"]"
        )));
        assert!(css.contains("background-color:#E2F0D9"));
        assert!(!css.contains("color:#000000"));

        let row_band_rule = css
            .find(&format!(
                ".hcd-table.{table_class}[data-hcd-look-h-band=\"true\"]>tbody>tr[data-hcd-row-band=\"1\"]>td{{"
            ))
            .unwrap();
        let column_band_rule = css
            .find(&format!(
                ".hcd-table.{table_class}[data-hcd-look-v-band=\"true\"]>tbody>tr>td[data-hcd-column-band=\"1\"]{{"
            ))
            .unwrap();
        let first_column_rule = css
            .find(&format!(
                ".hcd-table.{table_class}[data-hcd-look-first-column=\"true\"]>tbody>tr>td:first-child{{"
            ))
            .unwrap();
        let first_row_rule = css
            .find(&format!(
                ".hcd-table.{table_class}[data-hcd-look-first-row=\"true\"]>tbody>tr:first-child>td{{"
            ))
            .unwrap();
        let northwest_rule = css
            .find(&format!(
                ".hcd-table.{table_class}[data-hcd-look-first-row=\"true\"][data-hcd-look-first-column=\"true\"]>tbody>tr:first-child>td:first-child{{"
            ))
            .unwrap();
        assert!(row_band_rule < column_band_rule);
        assert!(column_band_rule < first_column_rule);
        assert!(first_column_rule < first_row_rule);
        assert!(first_row_rule < northwest_rule);

        let inherited_bands = rendered.table_bands.get("FancyTable").unwrap();
        assert_eq!(inherited_bands.row, 2);
        assert_eq!(inherited_bands.column, 3);

        let table_xml = r#"<w:document xmlns:w="w"><w:body><w:tbl><w:tblPr><w:tblStyle w:val="FancyTable"/></w:tblPr><w:tr><w:tc><w:p><w:r><w:t>styled bands</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
        let (html, _) = render_test_part_with_table_bands(
            table_xml,
            &WordTheme::default(),
            &rendered.table_bands,
            None,
        );
        assert!(html.contains("data-hcd-row-band-size=\"2\""));
        assert!(html.contains("data-hcd-column-band-size=\"3\""));

        let override_xml = r#"<w:document xmlns:w="w"><w:body><w:tbl><w:tblPr><w:tblStyle w:val="FancyTable"/><w:tblStyleRowBandSize w:val="4"/></w:tblPr><w:tr><w:tc><w:p><w:r><w:t>direct override</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;
        let (override_html, _) = render_test_part_with_table_bands(
            override_xml,
            &WordTheme::default(),
            &rendered.table_bands,
            None,
        );
        assert!(override_html.contains("data-hcd-row-band-size=\"4\""));
        assert!(override_html.contains("data-hcd-column-band-size=\"3\""));
    }

    #[test]
    fn relationship_targets_are_normalized() {
        assert_eq!(
            resolve_relationship_target("word/document.xml", "media/image1.png").unwrap(),
            "word/media/image1.png"
        );
        assert_eq!(
            resolve_relationship_target("word/header1.xml", "../media/image1.png").unwrap(),
            "media/image1.png"
        );
    }

    #[test]
    fn word_numbering_materializes_multilevel_markers_and_start_overrides() {
        let xml = br#"<w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1.%2)"/></w:lvl></w:abstractNum><w:num w:numId="42"><w:abstractNumId w:val="7"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="3"/></w:lvlOverride></w:num><w:num w:numId="43"><w:abstractNumId w:val="7"/></w:num></w:numbering>"#;
        let catalog = parse_word_numbering(xml).unwrap();
        let mut state = NumberingState::default();

        assert!(state
            .render_marker(&catalog, Some("42"), Some("0"))
            .unwrap()
            .contains(">3.</span>"));
        assert!(state
            .render_marker(&catalog, Some("42"), Some("1"))
            .unwrap()
            .contains(">3.a)</span>"));
        assert!(state
            .render_marker(&catalog, Some("42"), Some("1"))
            .unwrap()
            .contains(">3.b)</span>"));
        assert!(state
            .render_marker(&catalog, Some("42"), Some("0"))
            .unwrap()
            .contains(">4.</span>"));
        assert!(state
            .render_marker(&catalog, Some("43"), Some("0"))
            .unwrap()
            .contains(">5.</span>"));
    }

    #[test]
    fn word_numbering_preserves_marker_formatting_indent_and_style_inheritance() {
        let xml = br#"<w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="right"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Arial"/><w:sz w:val="28"/><w:color w:val="C00000"/><w:b/><w:i/></w:rPr></w:lvl></w:abstractNum><w:num w:numId="42"><w:abstractNumId w:val="7"/></w:num></w:numbering>"#;
        let catalog = parse_word_numbering(xml).unwrap();
        let marker = NumberingState::default()
            .render_marker_details(&catalog, Some("42"), Some("0"))
            .unwrap();
        assert!(
            marker.html.contains("font-family:&apos;Arial&apos;"),
            "{}",
            marker.html
        );
        assert!(marker.html.contains("font-size:14.0pt"));
        assert!(marker.html.contains("line-height:1.1500"));
        assert!(marker.html.contains("color:#C00000"));
        assert!(marker.html.contains("font-weight:700"));
        assert!(marker.html.contains("font-style:italic"));
        assert!(marker.html.contains("text-align:right"));
        assert!(marker.html.contains("min-width:18.0pt"));
        assert!(marker.html.contains("padding-right:0.5em"));
        assert!(!marker.html.contains(";width:18.0pt"));
        assert_eq!(marker.definition.left_twips, Some(720));
        assert_eq!(marker.definition.hanging_twips, Some(360));

        let mut styles = BTreeMap::new();
        styles.insert(
            "BaseList".to_string(),
            WordStyleDefinition {
                paragraph: ParagraphFormat {
                    numbering_id: Some("42".to_string()),
                    numbering_level: Some("0".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        styles.insert(
            "DerivedList".to_string(),
            WordStyleDefinition {
                based_on: Some("BaseList".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            resolve_paragraph_style_numbering(&styles).get("DerivedList"),
            Some(&("42".to_string(), Some("0".to_string())))
        );
    }

    #[test]
    fn canonical_html_exposes_safe_word_formatting_and_structure() {
        let xml = r#"<w:document xmlns:w="w" xmlns:w14="w14" xmlns:r="r" xmlns:wp="wp" xmlns:a="a"><w:body>
          <w:p w14:paraId="P1"><w:pPr><w:pStyle w:val="Heading1"/><w:jc w:val="center"/><w:ind w:left="720"/><w:spacing w:after="240"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="42"/></w:numPr></w:pPr>
            <w:hyperlink r:id="rIdLink"><w:r w14:textId="T1"><w:rPr><w:b/><w:i/><w:u w:val="single"/><w:color w:val="FF0000"/><w:sz w:val="28"/><w:rFonts w:ascii="Arial"/></w:rPr><w:t>Styled link</w:t></w:r></w:hyperlink>
            <w:fldSimple w:instr="HYPERLINK &quot;https://simple.example/path&quot;"><w:r><w:t>Simple field link</w:t></w:r></w:fldSimple>
            <w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK "https://complex.example/doc" \l "section-2" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Complex field link</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>
            <w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="457200"/><wp:docPr descr="Chart preview"/><a:graphic><a:blip r:embed="rIdImage"/></a:graphic></wp:inline></w:drawing></w:r>
            <w:r><w:drawing><wp:anchor distT="91440" distB="182880" distL="274320" distR="365760" relativeHeight="7" behindDoc="0" layoutInCell="1" allowOverlap="0"><wp:simplePos x="100" y="200"/><wp:positionH relativeFrom="margin"><wp:posOffset>914400</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>-457200</wp:posOffset></wp:positionV><wp:extent cx="1828800" cy="914400"/><wp:wrapSquare wrapText="right"/><wp:docPr id="42" descr="Floating preview"/><a:graphic><a:blip r:embed="rIdImage"/></a:graphic></wp:anchor></w:drawing></w:r>
          </w:p>
          <w:tbl><w:tblPr><w:tblStyle w:val="FancyTable"/><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/><w:tblW w:type="dxa" w:w="7200"/><w:jc w:val="center"/><w:tblLayout w:type="fixed"/><w:shd w:fill="EEEEEE"/><w:tblBorders><w:top w:val="double" w:sz="12" w:color="123456"/></w:tblBorders><w:tblCellMar><w:left w:w="100"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w="2400"/><w:gridCol w:w="4800"/></w:tblGrid><w:tr><w:trPr><w:trHeight w:val="360" w:hRule="exact"/><w:cantSplit/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val="2"/><w:vMerge w:val="restart"/><w:tcW w:type="dxa" w:w="2400"/><w:shd w:fill="FFF2CC"/><w:vAlign w:val="bottom"/><w:tcBorders><w:left w:val="dashed" w:sz="8" w:color="FF0000"/></w:tcBorders><w:tcMar><w:right w:w="120"/></w:tcMar></w:tcPr><w:p><w:r><w:t>merged</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
        </w:body></w:document>"#;
        let temp = tempfile::tempdir().unwrap();
        let mut writer = BundleWriter::create(temp.path().join("bundle")).unwrap();
        writer.write_styles(default_styles()).unwrap();
        let mut html_hrefs = Vec::new();
        let options = ImportOptions::new("format-test");
        let mut relationships = PartRelationships::default();
        relationships.hyperlinks.insert(
            "rIdLink".to_string(),
            "https://example.test/path".to_string(),
        );
        relationships.assets.insert(
            "rIdImage".to_string(),
            AssetRecord {
                source_part: "word/media/image1.png".to_string(),
                hash: "ab".repeat(32),
                href: format!("assets/sha256/{}.png", "ab".repeat(32)),
                byte_length: 1,
            },
        );
        let numbering = parse_word_numbering(br#"<w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0"><w:start w:val="3"/><w:numFmt w:val="upperRoman"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="42"><w:abstractNumId w:val="7"/></w:num></w:numbering>"#).unwrap();
        let root = writer.root().to_path_buf();
        {
            let mut emit = |event: &ImportEvent| {
                if let ImportEvent::ChunkReady { descriptor } = event {
                    html_hrefs.push(descriptor.html_href.clone());
                }
                Ok(())
            };
            let mut accumulator = ChunkAccumulator::new(
                "format-test",
                "word/document.xml",
                "body",
                &options,
                &mut writer,
                &mut emit,
            );
            parse_text_part(
                &mut Cursor::new(xml.as_bytes()),
                &TextPartContext {
                    document_id: "format-test",
                    part: "word/document.xml",
                    relationships: &relationships,
                    numbering: &numbering,
                    paragraph_numbering: &BTreeMap::new(),
                    theme: &WordTheme::default(),
                    table_bands: &BTreeMap::new(),
                },
                &mut accumulator,
                &mut Vec::new(),
            )
            .unwrap();
            accumulator.flush().unwrap();
        }

        let html = std::fs::read_to_string(root.join(&html_hrefs[0])).unwrap();
        assert!(html.contains("data-hcd-word-style=\"Heading1\""));
        assert!(html.contains("data-hcd-num-id=\"42\""));
        assert!(html.contains("data-hcd-num-format=\"upperRoman\""));
        assert!(html.contains(">III.</span>"));
        assert!(html.contains("text-align:center"));
        assert!(html.contains("margin-left:36.00pt"));
        assert!(html.contains("font-weight:700"));
        assert!(html.contains("font-style:italic"));
        assert!(html.contains("font-size:14.0pt"));
        assert!(html.contains("href=\"https://example.test/path\""));
        assert!(html.contains("href=\"https://simple.example/path\""));
        assert!(html.contains("href=\"https://complex.example/doc#section-2\""));
        assert!(html.contains(">Simple field link</span>"));
        assert!(html.contains(">Complex field link</span>"));
        assert!(!html.contains("HYPERLINK"));
        assert!(html.contains("colspan=\"2\""));
        assert!(html.contains("data-hcd-v-merge=\"restart\""));
        assert!(html.contains("data-hcd-table-style=\"FancyTable\""));
        assert!(html.contains(&word_style_class("FancyTable")));
        assert!(html.contains("data-hcd-look-first-row=\"true\""));
        assert!(html.contains("data-hcd-look-first-column=\"true\""));
        assert!(html.contains("data-hcd-look-h-band=\"true\""));
        assert!(html.contains("data-hcd-look-v-band=\"false\""));
        assert!(html.contains("data-hcd-row-band-size=\"1\""));
        assert!(html.contains("data-hcd-column-band-size=\"1\""));
        assert!(html.contains("data-hcd-row-band=\"1\""));
        assert!(html.contains("data-hcd-column-band=\"1\""));
        assert!(html.contains("width:360.00pt;margin-left:auto;margin-right:auto;table-layout:fixed;background-color:#EEEEEE"));
        assert!(html
            .contains("<tr data-hcd-row-band=\"1\" style=\"height:18.00pt;break-inside:avoid\">"));
        assert!(html.contains("border-top:1.50pt double #123456"));
        assert!(html.contains("padding-left:5.00pt"));
        assert!(html.contains("width:120.00pt"));
        assert!(html.contains("background-color:#FFF2CC"));
        assert!(html.contains("vertical-align:bottom"));
        assert!(html.contains("border-left:1.00pt dashed #FF0000"));
        assert!(html.contains("padding-right:6.00pt"));
        assert!(html.contains(
            "<colgroup><col style=\"width:120.00pt\"/><col style=\"width:240.00pt\"/></colgroup>"
        ));
        assert!(html.contains("data-hcd-width-emu=\"914400\""));
        assert!(html.contains("alt=\"Chart preview\""));
        assert!(html.contains("data-hcd-drawing-layout=\"inline\""));
        assert!(html.contains("data-hcd-drawing-layout=\"anchor\""));
        assert!(html.contains("data-hcd-position-h-relative-from=\"margin\""));
        assert!(html.contains("data-hcd-position-v-relative-from=\"paragraph\""));
        assert!(html.contains("data-hcd-position-h-offset-emu=\"914400\""));
        assert!(html.contains("data-hcd-position-v-offset-emu=\"-457200\""));
        assert!(html.contains("data-hcd-wrap=\"square\""));
        assert!(html.contains("data-hcd-wrap-side=\"right\""));
        assert!(html.contains("data-hcd-drawing-id=\"42\""));
        assert!(html.contains("data-hcd-node-kind=\"image\""));
        assert!(html.contains("data-hcd-editable=\"false\""));
        assert!(html.contains("data-hcd-source-part=\"word/document.xml\""));
        assert!(html.contains("data-hcd-source-path=\"/drawing[1]\""));
        assert!(html.contains("data-hcd-behind-document=\"false\""));
        assert!(html.contains("data-hcd-layout-in-cell=\"true\""));
        assert!(html.contains("data-hcd-allow-overlap=\"false\""));
        assert!(html.contains("class=\"hcd-drawing hcd-drawing-anchor hcd-drawing-wrap-right\""));
        assert!(html.contains("position:relative;left:96.00px;top:-48.00px"));
        let styles = std::fs::read_to_string(root.join("styles.css")).unwrap();
        assert!(styles.contains("data-hcd-image-hitboxes"));
        assert!(styles.contains("data-hcd-text-hitboxes"));
    }

    #[test]
    fn drawingml_textboxes_preserve_geometry_grouping_and_nested_tables() {
        let xml = r#"<w:document xmlns:w="w" xmlns:mc="mc" xmlns:wps="wps" xmlns:wp="wp" xmlns:a="a"><w:body><w:p>
          <w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor relativeHeight="9" behindDoc="0"><wp:positionH relativeFrom="column"><wp:posOffset>1900000</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>400000</wp:posOffset></wp:positionV><wp:extent cx="1700000" cy="1400000"/><wp:docPr id="77"/><wps:wsp><wps:spPr><a:xfrm rot="2700000"/><a:prstGeom prst="roundRect"/><a:solidFill><a:srgbClr val="E3F2FD"/></a:solidFill><a:ln w="12700"><a:solidFill><a:srgbClr val="1565C0"/></a:solidFill><a:prstDash val="dash"/></a:ln></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>inside</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:txbxContent></wps:txbx><wps:bodyPr vert="eaVert" lIns="91440" tIns="45720" rIns="91440" bIns="45720" anchor="ctr"/></wps:wsp></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><w:txbxContent><w:p><w:r><w:t>fallback duplicate</w:t></w:r></w:p></w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:r>
          <w:r><w:drawing><wp:anchor relativeHeight="10"><wp:positionH relativeFrom="column"><wp:posOffset>3800000</wp:posOffset></wp:positionH><wp:positionV relativeFrom="paragraph"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="1700000" cy="1400000"/><wp:docPr id="78"/><wps:wsp><wps:spPr><a:prstGeom prst="rect"/><a:noFill/><a:ln><a:noFill/></a:ln></wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>second</w:t></w:r></w:p></w:txbxContent></wps:txbx><wps:bodyPr/></wps:wsp></wp:anchor></w:drawing></w:r>
        </w:p></w:body></w:document>"#;

        let (html, map) = render_test_part(xml, &WordTheme::default(), None);
        assert!(html.contains("class=\"hcd-chunk hcd-chunk-overflow\""));
        assert_eq!(html.matches("data-hcd-node-kind=\"textbox\"").count(), 2);
        assert!(html.contains("data-hcd-drawing-id=\"77\""));
        assert!(html.contains("left:199.48px;top:41.99px;width:178.48px;height:146.98px"));
        assert!(html.contains("transform:rotate(45.000deg)"));
        assert!(html.contains("background-color:#E3F2FD"));
        assert!(html.contains("border-top:1.00pt dashed #1565C0"));
        assert!(html.contains("border-radius:12px"));
        assert!(html.contains("writing-mode:vertical-rl"));
        assert!(html.contains("<aside class=\"hcd-textbox hcd-drawing\""));
        let first_aside = html.find("data-hcd-drawing-id=\"77\"").unwrap();
        let nested_table = html.find("<table class=\"hcd-table").unwrap();
        let first_aside_end = html[first_aside..].find("</aside>").unwrap() + first_aside;
        assert!(nested_table > first_aside && nested_table < first_aside_end);
        assert!(!html.contains("fallback duplicate"));
        assert!(html.contains("background-color:transparent"));
        assert_eq!(map.entries.len(), 3);
    }

    #[test]
    fn tracked_revisions_preserve_current_semantics_and_keep_deletions_read_only() {
        let xml = r#"<w:document xmlns:w="w" xmlns:w14="w14"><w:body><w:p w14:paraId="P1">
          <w:ins w:id="7" w:author="Alice" w:date="2026-08-30T12:00:00Z"><w:r w14:textId="I1"><w:t>added</w:t></w:r></w:ins>
          <w:del w:id="8" w:author="Bob"><w:r><w:delText>removed</w:delText></w:r></w:del>
          <w:r w14:textId="C1"><w:rPr><w:b/><w:rPrChange w:id="9"><w:rPr><w:i/><w:color w:val="FF0000"/></w:rPr></w:rPrChange></w:rPr><w:t>current</w:t></w:r>
        </w:p></w:body></w:document>"#;

        let (html, map) = render_test_part(xml, &WordTheme::default(), None);
        assert!(html.contains("data-hcd-revision=\"insert\""));
        assert!(html.contains("data-hcd-revision-author=\"Alice\""));
        assert!(html.contains("data-hcd-revision=\"delete\""));
        assert!(html.contains("data-hcd-editable=\"false\">removed</span>"));
        assert!(html.contains("font-weight:700"));
        assert!(!html.contains("font-style:italic"));
        assert!(!html.contains("color:#FF0000"));
        assert_eq!(map.entries.len(), 2);
        assert!(map.entries.iter().all(|entry| entry.source.editable));
        assert_eq!(map.entries[0].source.text_ordinal, 1);
        assert_eq!(map.entries[1].source.text_ordinal, 2);
        let canonical = hcd_core::extract_html_text_nodes(&html).unwrap();
        assert_eq!(canonical.len(), 2);
        assert!(canonical.values().any(|value| value == "added"));
        assert!(canonical.values().any(|value| value == "current"));
        assert!(!canonical.values().any(|value| value == "removed"));
    }

    #[test]
    fn vertical_merge_materializes_rowspan_and_locks_continuation_text() {
        let xml = r#"<w:document xmlns:w="w"><w:body><w:tbl>
          <w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>top</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc></w:tr>
          <w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p><w:r><w:t>hidden one</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr>
          <w:tr><w:tc><w:tcPr><w:vMerge w:val="continue"/></w:tcPr><w:p><w:r><w:t>hidden two</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>C</w:t></w:r></w:p></w:tc></w:tr>
        </w:tbl></w:body></w:document>"#;

        let (html, map) = render_test_part(xml, &WordTheme::default(), Some(64));
        assert!(html.contains("data-hcd-v-merge=\"restart\" rowspan=\"0000000003\""));
        assert_eq!(html.matches("style=\"display:none\"").count(), 2);
        let by_ordinal: BTreeMap<_, _> = map
            .entries
            .iter()
            .map(|entry| (entry.source.text_ordinal, entry.source.editable))
            .collect();
        assert_eq!(by_ordinal.len(), 6);
        assert_eq!(by_ordinal.get(&1), Some(&true));
        assert_eq!(by_ordinal.get(&3), Some(&false));
        assert_eq!(by_ordinal.get(&5), Some(&false));
        assert_eq!(by_ordinal.get(&6), Some(&true));
    }

    #[test]
    fn table_style_conditions_keep_global_row_semantics_across_fragments() {
        let xml = r#"<w:document xmlns:w="w"><w:body><w:tbl>
          <w:tblPr><w:tblStyle w:val="FancyTable"/><w:tblLook w:val="0060"/><w:tblStyleRowBandSize w:val="2"/><w:tblStyleColBandSize w:val="2"/><w:tblW w:type="dxa" w:w="6000"/><w:tblBorders><w:top w:val="single" w:sz="8" w:color="123456"/></w:tblBorders><w:tblCellMar><w:left w:w="100"/></w:tblCellMar></w:tblPr><w:tblGrid><w:gridCol w:w="6000"/></w:tblGrid>
          <w:tr><w:trPr><w:trHeight w:val="360"/><w:cantSplit/></w:trPr><w:tc><w:tcPr><w:shd w:fill="FFF2CC"/><w:vAlign w:val="center"/></w:tcPr><w:p><w:r><w:t>row one padding text</w:t></w:r></w:p></w:tc></w:tr>
          <w:tr><w:trPr><w:trHeight w:val="360"/><w:cantSplit/></w:trPr><w:tc><w:tcPr><w:shd w:fill="FFF2CC"/><w:vAlign w:val="center"/></w:tcPr><w:p><w:r><w:t>row two padding text</w:t></w:r></w:p></w:tc></w:tr>
          <w:tr><w:trPr><w:trHeight w:val="360"/><w:cantSplit/></w:trPr><w:tc><w:tcPr><w:shd w:fill="FFF2CC"/><w:vAlign w:val="center"/></w:tcPr><w:p><w:r><w:t>row three padding text</w:t></w:r></w:p></w:tc></w:tr>
        </w:tbl></w:body></w:document>"#;

        let chunks = render_test_parts(xml, &WordTheme::default(), Some(64));
        assert_eq!(chunks.len(), 3);
        let first = &chunks[0].0;
        let middle = &chunks[1].0;
        let last = &chunks[2].0;

        for (index, (html, map)) in chunks.iter().enumerate() {
            assert!(html.contains("data-hcd-table-style=\"FancyTable\""));
            assert!(html.contains(&word_style_class("FancyTable")));
            assert!(html.contains("style=\"width:300.00pt\""));
            assert!(html.contains("data-hcd-row-band-size=\"2\""));
            assert!(html.contains("data-hcd-column-band-size=\"2\""));
            assert!(html.contains("<colgroup><col style=\"width:300.00pt\"/></colgroup>"));
            assert!(html.contains("style=\"min-height:18.00pt;break-inside:avoid\""));
            assert!(html.contains("data-hcd-column-band=\"1\""));
            assert!(html.contains("border-top:1.00pt solid #123456"));
            assert!(html.contains("padding-left:5.00pt"));
            assert!(html.contains("background-color:#FFF2CC"));
            assert!(html.contains("vertical-align:middle"));
            assert_eq!(map.entries.len(), 1, "fragment {index}");
        }
        assert!(first.contains("data-hcd-look-first-row=\"true\""));
        assert!(first.contains("data-hcd-look-last-row=\"false\""));
        assert!(first.contains("<tr data-hcd-row-band=\"1\""));
        assert!(!first.contains("data-hcd-continuation=\"true\""));

        assert!(middle.contains("data-hcd-look-first-row=\"false\""));
        assert!(middle.contains("data-hcd-look-last-row=\"false\""));
        assert!(middle.contains("<tr data-hcd-row-band=\"1\""));
        assert!(middle.contains("data-hcd-continuation=\"true\""));

        assert!(last.contains("data-hcd-look-first-row=\"false\""));
        assert!(last.contains("data-hcd-look-last-row=\"true\""));
        assert!(last.contains("<tr data-hcd-row-band=\"2\""));
        assert!(last.contains("data-hcd-continuation=\"true\""));
    }

    #[test]
    fn logical_column_bands_account_for_grid_span() {
        let xml = r#"<w:document xmlns:w="w"><w:body><w:tbl><w:tblPr><w:tblLook w:val="0000"/><w:tblStyleColBandSize w:val="1"/></w:tblPr><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>wide</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>logical column three</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>logical column four</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#;

        let (html, map) = render_test_part(xml, &WordTheme::default(), None);

        assert_eq!(html.matches("data-hcd-column-band=\"1\"").count(), 2);
        assert_eq!(html.matches("data-hcd-column-band=\"2\"").count(), 1);
        assert!(html.contains("data-hcd-column-band=\"1\" colspan=\"2\""));
        assert_eq!(map.entries.len(), 3);
    }

    #[test]
    fn conditional_style_masks_are_materialized_on_rows_cells_and_paragraphs() {
        let xml = r#"<w:document xmlns:w="w"><w:body><w:tbl><w:tr>
          <w:trPr><w:cnfStyle w:val="100000100000" w:firstRow="0" w:evenHBand="1"/></w:trPr>
          <w:tc><w:tcPr><w:cnfStyle w:val="001000000100"/></w:tcPr><w:p><w:pPr><w:cnfStyle w:val="101000000100"/></w:pPr><w:r><w:t>conditional</w:t></w:r></w:p></w:tc>
        </w:tr></w:tbl></w:body></w:document>"#;

        let (html, map) = render_test_part(xml, &WordTheme::default(), None);

        assert!(html.contains(
            "<tr data-hcd-row-band=\"1\" data-hcd-cnf-present=\"true\" data-hcd-cnf-first-row=\"false\""
        ));
        assert!(html.contains("data-hcd-cnf-band1-horizontal=\"true\""));
        assert!(html.contains("data-hcd-cnf-band2-horizontal=\"true\""));
        assert!(html.contains(
            "<td data-hcd-column-band=\"1\" data-hcd-cnf-present=\"true\" data-hcd-cnf-first-row=\"false\" data-hcd-cnf-last-row=\"false\" data-hcd-cnf-first-column=\"true\""
        ));
        assert!(html.contains("data-hcd-cnf-nw-cell=\"true\""));
        assert!(html.contains("class=\"hcd-paragraph\" data-hcd-id="));
        assert!(html.contains(
            "data-hcd-cnf-first-row=\"true\" data-hcd-cnf-last-row=\"false\" data-hcd-cnf-first-column=\"true\""
        ));
        assert_eq!(map.entries.len(), 1);
    }

    #[test]
    fn table_property_exceptions_override_row_cells_and_ignore_history() {
        let xml = r#"<w:document xmlns:w="w"><w:body><w:tbl>
          <w:tblPr><w:tblCellMar><w:left w:w="100"/></w:tblCellMar></w:tblPr>
          <w:tr><w:tblPrEx><w:tblBorders><w:top w:val="single" w:sz="8" w:color="00AA00"/></w:tblBorders><w:tblCellMar><w:left w:w="200"/><w:right w:w="300"/></w:tblCellMar><w:shd w:fill="DDEEFF"/><w:tblPrExChange><w:tblPrEx><w:shd w:fill="FF0000"/></w:tblPrEx></w:tblPrExChange></w:tblPrEx><w:tc><w:tcPr><w:shd w:fill="FFFFFF"/><w:tcBorders><w:left w:val="dashed" w:sz="8" w:color="112233"/></w:tcBorders></w:tcPr><w:p><w:r><w:t>exception row</w:t></w:r></w:p></w:tc></w:tr>
          <w:tr><w:tc><w:p><w:r><w:t>ordinary row</w:t></w:r></w:p></w:tc></w:tr>
        </w:tbl></w:body></w:document>"#;

        let (html, map) = render_test_part(xml, &WordTheme::default(), None);
        let first_row_start = html
            .find("<tr data-hcd-row-band=\"1\" data-hcd-table-property-exception=\"true\"")
            .unwrap();
        let second_row_start = html[first_row_start + 1..]
            .find("<tr data-hcd-row-band=\"2\"")
            .map(|offset| first_row_start + 1 + offset)
            .unwrap();
        let first_row = &html[first_row_start..second_row_start];
        let second_row = &html[second_row_start..];

        let table_padding = first_row.find("padding-left:5.00pt").unwrap();
        let exception_border = first_row.find("border-top:1.00pt solid #00AA00").unwrap();
        let exception_padding = first_row.find("padding-left:10.00pt").unwrap();
        let exception_shading = first_row.find("background-color:#DDEEFF").unwrap();
        let direct_shading = first_row.find("background-color:#FFFFFF").unwrap();
        let direct_border = first_row.find("border-left:1.00pt dashed #112233").unwrap();
        assert!(table_padding < exception_border);
        assert!(exception_border < exception_padding);
        assert!(exception_padding < exception_shading);
        assert!(exception_shading < direct_shading);
        assert!(direct_shading < direct_border);
        assert!(first_row.contains("padding-right:15.00pt"));
        assert!(!first_row.contains("#FF0000"));
        assert!(!second_row.contains("data-hcd-table-property-exception"));
        assert!(second_row.contains("padding-left:5.00pt"));
        assert!(!second_row.contains("#00AA00"));
        assert_eq!(map.entries.len(), 2);
    }

    #[test]
    fn direct_table_formats_produce_a_valid_file_backed_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("direct-table.docx");
        let output = temp.path().join("direct-table.hcd");
        let document_xml = r#"<w:document xmlns:w="w"><w:body><w:tbl>
          <w:tblPr><w:tblW w:type="dxa" w:w="6000"/><w:tblLayout w:type="fixed"/><w:tblBorders><w:top w:val="double" w:sz="12" w:color="123456"/></w:tblBorders><w:tblCellMar><w:left w:w="100"/></w:tblCellMar></w:tblPr>
          <w:tblGrid><w:gridCol w:w="6000"/></w:tblGrid>
          <w:tr><w:trPr><w:cnfStyle w:val="100000100000"/><w:trHeight w:val="360"/><w:cantSplit/></w:trPr><w:tblPrEx><w:tblBorders><w:bottom w:val="single" w:sz="8" w:color="00AA00"/></w:tblBorders><w:tblCellMar><w:top w:w="80"/></w:tblCellMar></w:tblPrEx><w:tc><w:tcPr><w:cnfStyle w:val="101000000100"/><w:shd w:fill="FFF2CC"/><w:vAlign w:val="center"/><w:tcBorders><w:left w:val="dashed" w:sz="8" w:color="FF0000"/></w:tcBorders><w:tcMar><w:right w:w="120"/></w:tcMar></w:tcPr><w:p><w:pPr><w:cnfStyle w:val="101000000100"/></w:pPr><w:r><w:t>direct formatting</w:t></w:r></w:p></w:tc></w:tr>
        </w:tbl></w:body></w:document>"#;
        let file = std::fs::File::create(&source).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();
        zip.finish().unwrap();

        let manifest = import_docx(
            &source,
            &output,
            &ImportOptions::new("direct-table-file"),
            |_| Ok(()),
        )
        .unwrap();
        let bundle = hcd_core::Bundle::open(&output).unwrap();
        let report = hcd_core::validate_bundle(&bundle).unwrap();
        assert!(report.valid, "{:?}", report.issues);
        let page = bundle.read_index_page(&manifest, 0).unwrap();
        let html = bundle.read_chunk(&page.chunks[0]).unwrap();
        assert!(html.contains("table-layout:fixed"));
        assert!(html.contains("break-inside:avoid"));
        assert!(html.contains("border-top:1.50pt double #123456"));
        assert!(html.contains("border-left:1.00pt dashed #FF0000"));
        assert!(html.contains("border-bottom:1.00pt solid #00AA00"));
        assert!(html.contains("padding-left:5.00pt"));
        assert!(html.contains("padding-top:4.00pt"));
        assert!(html.contains("padding-right:6.00pt"));
        assert!(html.contains("data-hcd-table-property-exception=\"true\""));
        assert!(html.contains("data-hcd-cnf-present=\"true\""));
        assert!(html.contains("data-hcd-cnf-first-row=\"true\""));
        assert!(html.contains("data-hcd-cnf-first-column=\"true\""));
        assert!(html.contains("data-hcd-cnf-nw-cell=\"true\""));
        assert!(html.contains("<colgroup><col style=\"width:300.00pt\"/></colgroup>"));
        assert!(html.contains("data-hcd-row-band=\"1\""));
        assert!(html.contains("data-hcd-column-band=\"1\""));
        assert!(html.contains("data-hcd-look-first-row=\"false\""));
        assert!(html.contains("data-hcd-look-last-row=\"false\""));
        assert!(html.contains("data-hcd-look-first-column=\"false\""));
        assert!(html.contains("data-hcd-look-last-column=\"false\""));
        assert!(html.contains("data-hcd-look-h-band=\"true\""));
        assert!(html.contains("data-hcd-look-v-band=\"true\""));
    }

    #[test]
    fn run_fonts_follow_east_asia_and_bidi_language_theme_slots() {
        let theme_xml = r#"<a:theme xmlns:a="a"><a:themeElements><a:fontScheme><a:majorFont><a:latin typeface="Aptos Display"/><a:ea typeface=""/><a:cs typeface="Generic Arabic"/><a:font script="Hans" typeface="等线"/><a:font script="Arab" typeface="Noto Naskh Arabic"/></a:majorFont><a:minorFont><a:latin typeface="Aptos"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;
        let theme = parse_word_theme(theme_xml.as_bytes(), "word/theme/theme1.xml").unwrap();
        let xml = r#"<w:document xmlns:w="w"><w:body><w:p>
          <w:r><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:eastAsiaTheme="majorEastAsia"/><w:lang w:val="en-US" w:eastAsia="zh-CN"/></w:rPr><w:t>中文</w:t></w:r>
          <w:r><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:cstheme="majorBidi"/><w:lang w:val="en-US" w:bidi="ar-SA"/><w:rtl/></w:rPr><w:t>العربية</w:t></w:r>
        </w:p></w:body></w:document>"#;

        let (html, _) = render_test_part(xml, &theme, None);
        assert!(html.contains("data-hcd-font=\"等线\""));
        assert!(html.contains("data-hcd-font-east-asia=\"等线\""));
        assert!(html.contains("font-family:'等线','Aptos Display'"));
        assert!(html.contains("data-hcd-font=\"Noto Naskh Arabic\""));
        assert!(html.contains("data-hcd-font-bidi=\"Noto Naskh Arabic\""));
        assert!(html.contains("direction:rtl"));
    }

    #[test]
    fn word_font_stacks_keep_metric_compatible_fallbacks() {
        assert_eq!(
            word_font_stack(&["Calibri"]),
            "'Calibri',-apple-system,sans-serif"
        );
        assert_eq!(
            word_font_stack(&["Times New Roman"]),
            "'Times New Roman',Georgia,serif"
        );
        assert_eq!(
            word_font_stack(&["等线", "Aptos Display"]),
            "'等线','Aptos Display','Songti SC','STSong',sans-serif"
        );
    }

    #[test]
    fn first_chunk_is_emitted_before_source_part_eof() {
        let mut xml = String::from(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
        );
        for index in 0..2_000 {
            xml.push_str(&format!(
                "<w:p><w:r><w:t>paragraph-{index:04}-abcdefghijklmnopqrstuvwxyz</w:t></w:r></w:p>"
            ));
        }
        xml.push_str("</w:body></w:document>");

        let eof_seen = Arc::new(AtomicBool::new(false));
        let mut source = EofTrackingReader {
            inner: Cursor::new(xml.into_bytes()),
            eof_seen: eof_seen.clone(),
        };
        let temp = tempfile::tempdir().unwrap();
        let mut writer = BundleWriter::create(temp.path().join("bundle")).unwrap();
        writer.write_styles(default_styles()).unwrap();
        let mut emitted_before_eof = false;
        let mut emit = |event: &ImportEvent| {
            if matches!(event, ImportEvent::ChunkReady { .. }) && !eof_seen.load(Ordering::SeqCst) {
                emitted_before_eof = true;
                return Err(HcdError::InvalidBundle(
                    "stop after observing first progressive chunk".to_string(),
                ));
            }
            Ok(())
        };
        let mut options = ImportOptions::new("progressive-test");
        options.chunk_soft_bytes = 512;
        let mut accumulator = ChunkAccumulator::new(
            "progressive-test",
            "word/document.xml",
            "body",
            &options,
            &mut writer,
            &mut emit,
        );
        let result = parse_text_part(
            &mut source,
            &TextPartContext {
                document_id: "progressive-test",
                part: "word/document.xml",
                relationships: &PartRelationships::default(),
                numbering: &NumberingCatalog::default(),
                paragraph_numbering: &BTreeMap::new(),
                theme: &WordTheme::default(),
                table_bands: &BTreeMap::new(),
            },
            &mut accumulator,
            &mut Vec::new(),
        );
        assert!(result.is_err());
        drop(accumulator);
        assert!(emitted_before_eof);
    }

    #[test]
    fn oversized_text_node_and_deep_xml_are_rejected() {
        let oversized = format!(
            "<w:document xmlns:w=\"x\"><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:body></w:document>",
            "x".repeat(MAX_CHUNK_BYTES + 1)
        );
        let oversized_error = parse_for_resource_error(oversized);
        assert!(oversized_error.to_string().contains("NODE_TOO_LARGE"));

        let mut deep = String::from("<root>");
        for _ in 0..MAX_XML_DEPTH {
            deep.push_str("<x>");
        }
        deep.push_str("<x/>");
        for _ in 0..MAX_XML_DEPTH {
            deep.push_str("</x>");
        }
        deep.push_str("</root>");
        let deep_error = parse_for_resource_error(deep);
        assert!(deep_error.to_string().contains("maximum XML depth"));
    }

    #[test]
    fn oversized_vertical_merge_group_is_rejected_before_chunk_publication() {
        let mut xml = String::from("<w:document xmlns:w=\"w\"><w:body><w:tbl>");
        for row in 0..120 {
            let merge = if row == 0 {
                "<w:vMerge w:val=\"restart\"/>"
            } else {
                "<w:vMerge/>"
            };
            xml.push_str("<w:tr><w:tc><w:tcPr>");
            xml.push_str(merge);
            xml.push_str("</w:tcPr><w:p><w:r><w:t>");
            xml.push_str(&"x".repeat(20_000));
            xml.push_str("</w:t></w:r></w:p></w:tc></w:tr>");
        }
        xml.push_str("</w:tbl></w:body></w:document>");

        let error = parse_for_resource_error(xml);

        assert!(error.to_string().contains("vertically merged row group"));
    }

    #[test]
    fn oversized_table_grid_is_rejected_before_html_growth() {
        let mut xml = String::from("<w:document xmlns:w=\"w\"><w:body><w:tbl><w:tblGrid>");
        for _ in 0..=MAX_TABLE_GRID_COLUMNS {
            xml.push_str("<w:gridCol w:w=\"120\"/>");
        }
        xml.push_str("</w:tblGrid></w:tbl></w:body></w:document>");

        let error = parse_for_resource_error(xml);

        assert!(matches!(error, HcdError::ResourceLimit(_)));
        assert!(error.to_string().contains("table grid exceeds"));
    }

    #[test]
    fn oversized_table_band_size_is_rejected() {
        let xml = format!(
            "<w:document xmlns:w=\"w\"><w:body><w:tbl><w:tblPr><w:tblStyleRowBandSize w:val=\"{}\"/></w:tblPr></w:tbl></w:body></w:document>",
            MAX_TABLE_BAND_SIZE + 1
        );

        let error = parse_for_resource_error(xml);

        assert!(matches!(error, HcdError::ResourceLimit(_)));
        assert!(error.to_string().contains("tblStyleRowBandSize"));
    }

    #[test]
    fn oversized_drawing_position_value_is_rejected() {
        let xml = format!(
            "<w:document xmlns:w=\"w\" xmlns:wp=\"wp\"><w:body><w:p><w:r><w:drawing><wp:anchor><wp:positionH relativeFrom=\"page\"><wp:posOffset>{}</wp:posOffset></wp:positionH></wp:anchor></w:drawing></w:r></w:p></w:body></w:document>",
            "9".repeat(129)
        );

        let error = parse_for_resource_error(xml);

        assert!(matches!(error, HcdError::ResourceLimit(_)));
        assert!(error.to_string().contains("position value"));
    }

    fn render_test_part(
        xml: &str,
        theme: &WordTheme,
        chunk_soft_bytes: Option<usize>,
    ) -> (String, ChunkSourceMap) {
        let mut chunks = render_test_parts(xml, theme, chunk_soft_bytes);
        assert_eq!(chunks.len(), 1, "test fixture should emit one chunk");
        chunks.pop().unwrap()
    }

    fn render_test_part_with_table_bands(
        xml: &str,
        theme: &WordTheme,
        table_bands: &TableBandCatalog,
        chunk_soft_bytes: Option<usize>,
    ) -> (String, ChunkSourceMap) {
        let mut chunks =
            render_test_parts_with_table_bands(xml, theme, table_bands, chunk_soft_bytes);
        assert_eq!(chunks.len(), 1, "test fixture should emit one chunk");
        chunks.pop().unwrap()
    }

    fn render_test_parts(
        xml: &str,
        theme: &WordTheme,
        chunk_soft_bytes: Option<usize>,
    ) -> Vec<(String, ChunkSourceMap)> {
        render_test_parts_with_table_bands(xml, theme, &BTreeMap::new(), chunk_soft_bytes)
    }

    fn render_test_parts_with_table_bands(
        xml: &str,
        theme: &WordTheme,
        table_bands: &TableBandCatalog,
        chunk_soft_bytes: Option<usize>,
    ) -> Vec<(String, ChunkSourceMap)> {
        let temp = tempfile::tempdir().unwrap();
        let bundle_path = temp.path().join("bundle");
        let mut writer = BundleWriter::create(&bundle_path).unwrap();
        writer.write_styles(default_styles()).unwrap();
        let mut hrefs = Vec::new();
        let mut options = ImportOptions::new("render-test");
        if let Some(chunk_soft_bytes) = chunk_soft_bytes {
            options.chunk_soft_bytes = chunk_soft_bytes;
        }
        {
            let mut emit = |event: &ImportEvent| {
                if let ImportEvent::ChunkReady { descriptor } = event {
                    hrefs.push((descriptor.html_href.clone(), descriptor.map_href.clone()));
                }
                Ok(())
            };
            let relationships = PartRelationships::default();
            let numbering = NumberingCatalog::default();
            let mut accumulator = ChunkAccumulator::new(
                "render-test",
                "word/document.xml",
                "body",
                &options,
                &mut writer,
                &mut emit,
            );
            parse_text_part(
                &mut Cursor::new(xml.as_bytes()),
                &TextPartContext {
                    document_id: "render-test",
                    part: "word/document.xml",
                    relationships: &relationships,
                    numbering: &numbering,
                    paragraph_numbering: &BTreeMap::new(),
                    theme,
                    table_bands,
                },
                &mut accumulator,
                &mut Vec::new(),
            )
            .unwrap();
            accumulator.flush().unwrap();
        }
        hrefs
            .into_iter()
            .map(|(html_href, map_href)| {
                let html = std::fs::read_to_string(bundle_path.join(html_href)).unwrap();
                let map =
                    serde_json::from_slice(&std::fs::read(bundle_path.join(map_href)).unwrap())
                        .unwrap();
                (html, map)
            })
            .collect()
    }

    fn parse_for_resource_error(xml: String) -> HcdError {
        let temp = tempfile::tempdir().unwrap();
        let mut writer = BundleWriter::create(temp.path().join("bundle")).unwrap();
        writer.write_styles(default_styles()).unwrap();
        let mut emit = |_: &ImportEvent| Ok(());
        let options = ImportOptions::new("resource-test");
        let mut accumulator = ChunkAccumulator::new(
            "resource-test",
            "word/document.xml",
            "body",
            &options,
            &mut writer,
            &mut emit,
        );
        parse_text_part(
            &mut Cursor::new(xml.into_bytes()),
            &TextPartContext {
                document_id: "resource-test",
                part: "word/document.xml",
                relationships: &PartRelationships::default(),
                numbering: &NumberingCatalog::default(),
                paragraph_numbering: &BTreeMap::new(),
                theme: &WordTheme::default(),
                table_bands: &BTreeMap::new(),
            },
            &mut accumulator,
            &mut Vec::new(),
        )
        .unwrap_err()
    }

    struct EofTrackingReader {
        inner: Cursor<Vec<u8>>,
        eof_seen: Arc<AtomicBool>,
    }

    impl Read for EofTrackingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let read = self.inner.read(buffer)?;
            if read == 0 {
                self.eof_seen.store(true, Ordering::SeqCst);
            }
            Ok(read)
        }
    }

    #[test]
    fn namespaced_local_names_are_supported() {
        assert_eq!(local_name(b"w14:paraId"), "paraId");
        assert_eq!(local_name(b"w:t"), "t");
    }
}
