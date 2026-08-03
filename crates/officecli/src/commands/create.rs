use clap::Args;
use handler_common::HandlerError;

/// Create a blank document (docx, xlsx, pptx, pdf)
#[derive(Args)]
pub struct CreateCommand {
    /// File path to create
    pub file: String,

    /// Format: docx, xlsx, pptx
    #[arg(long, visible_alias = "type")]
    pub format: Option<String>,

    /// Overwrite an existing file. Without this flag create refuses to replace data.
    #[arg(long)]
    pub force: bool,

    /// For .docx, emit a raw OOXML scaffold without the Word-style baseline.
    #[arg(long)]
    pub minimal: bool,

    /// Locale tag used to seed DOCX script fonts and RTL defaults (for example zh-CN or ar-SA).
    #[arg(long)]
    pub locale: Option<String>,
}

pub fn handle_create(
    cmd: CreateCommand,
    _format: handler_common::OutputFormat,
) -> Result<String, HandlerError> {
    let mut output_file = cmd.file;
    let ext = cmd.format.unwrap_or_else(|| {
        std::path::Path::new(&output_file)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default()
    });
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    if std::path::Path::new(&output_file).extension().is_none() && !ext.is_empty() {
        output_file.push('.');
        output_file.push_str(ext.trim_start_matches('.'));
    }
    if std::path::Path::new(&output_file).exists() && !cmd.force {
        return Err(HandlerError::InvalidArgument(format!(
            "file already exists: {}. Use --force to overwrite.",
            output_file
        )));
    }

    let result = match ext.as_str() {
        "docx" => create_blank_docx_with_locale(&output_file, cmd.minimal, cmd.locale.as_deref())?,
        "xlsx" => create_blank_xlsx(&output_file)?,
        "pptx" => create_blank_pptx(&output_file)?,
        "pdf" => create_blank_pdf(&output_file)?,
        other => {
            return Err(HandlerError::UnsupportedMode(format!(
                "create {} not supported",
                other
            )))
        }
    };

    Ok(result)
}

pub(crate) fn create_blank_docx(path: &str) -> Result<String, HandlerError> {
    create_blank_docx_with_options(path, false)
}

fn create_blank_docx_with_options(path: &str, minimal: bool) -> Result<String, HandlerError> {
    create_blank_docx_with_locale(path, minimal, None)
}

fn create_blank_docx_with_locale(
    path: &str,
    minimal: bool,
    locale: Option<&str>,
) -> Result<String, HandlerError> {
    use oxml::OxmlPackage;

    // Minimal blank docx: word/document.xml with empty body
    let rtl = locale.map(locale_is_rtl).unwrap_or(false);
    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    <w:p><w:r><w:t/></w:r></w:p>
    {}
  </w:body>
</w:document>"#,
        if rtl {
            "<w:sectPr><w:bidi/></w:sectPr>"
        } else {
            ""
        }
    );

    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
</Types>"#;
    let minimal_content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    // Word permits documents without styles.xml, but Markdown headings and
    // direct `style=HeadingN` edits must not create dangling pStyle IDs. Keep
    // the built-in style IDs Word itself recognizes, with concise definitions
    // sufficient for interoperable OOXML and later style mutation.
    let (latin, east_asia, complex_script) = locale_fonts(locale);
    let r_fonts = format!(
        "<w:rFonts w:ascii=\"{}\" w:hAnsi=\"{}\"{}{} />",
        latin.unwrap_or("Calibri"),
        latin.unwrap_or("Calibri"),
        east_asia
            .map(|font| format!(" w:eastAsia=\"{}\"", font))
            .unwrap_or_default(),
        complex_script
            .map(|font| format!(" w:cs=\"{}\"", font))
            .unwrap_or_default(),
    );
    let paragraph_defaults = if rtl {
        "<w:pPrDefault><w:pPr><w:bidi/></w:pPr></w:pPrDefault>"
    } else {
        ""
    };
    let styles_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults><w:rPrDefault><w:rPr>{}</w:rPr></w:rPrDefault>{}</w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading5"><w:name w:val="heading 5"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading6"><w:name w:val="heading 6"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading7"><w:name w:val="heading 7"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading8"><w:name w:val="heading 8"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading9"><w:name w:val="heading 9"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/></w:style>
</w:styles>"#,
        r_fonts, paragraph_defaults
    );
    let document_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;

    let mut pkg = OxmlPackage::create(path);
    pkg.add_part(
        "[Content_Types].xml",
        if minimal {
            minimal_content_types.as_bytes()
        } else {
            content_types.as_bytes()
        },
    );
    pkg.add_part("_rels/.rels", rels.as_bytes());
    pkg.add_part("word/document.xml", document_xml.as_bytes());
    if !minimal {
        pkg.add_part("word/styles.xml", styles_xml.as_bytes());
        pkg.add_part("word/_rels/document.xml.rels", document_rels.as_bytes());
    }

    pkg.save_as(path)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("Created blank Word document: {}", path))
}

fn locale_fonts(
    locale: Option<&str>,
) -> (
    Option<&'static str>,
    Option<&'static str>,
    Option<&'static str>,
) {
    let Some(locale) = locale else {
        return (None, None, None);
    };
    let normalized = locale.replace('_', "-").to_ascii_lowercase();
    let language = normalized.split('-').next().unwrap_or_default();
    match normalized.as_str() {
        "zh-tw" | "zh-hk" | "zh-mo" | "zh-hant" => {
            (Some("Times New Roman"), Some("新細明體"), None)
        }
        "zh-cn" | "zh-sg" | "zh-hans" => (Some("Times New Roman"), Some("等线"), None),
        _ => match language {
            "zh" => (Some("Times New Roman"), Some("等线"), None),
            "ja" => (Some("Times New Roman"), Some("游明朝"), None),
            "ko" => (Some("Times New Roman"), Some("맑은 고딕"), None),
            "ar" => (Some("Times New Roman"), None, Some("Arabic Typesetting")),
            "he" => (Some("Times New Roman"), None, Some("Times New Roman")),
            "th" => (Some("Times New Roman"), None, Some("Tahoma")),
            "fa" => (Some("Times New Roman"), None, Some("B Nazanin")),
            "ur" => (
                Some("Times New Roman"),
                None,
                Some("Jameel Noori Nastaleeq"),
            ),
            "hi" => (Some("Times New Roman"), None, Some("Mangal")),
            "en" | "fr" | "de" | "es" | "it" | "pt" | "nl" | "ru" | "pl" => {
                (Some("Times New Roman"), None, None)
            }
            _ => (None, None, None),
        },
    }
}

fn locale_is_rtl(locale: &str) -> bool {
    matches!(
        locale
            .replace('_', "-")
            .to_ascii_lowercase()
            .split('-')
            .next(),
        Some(
            "ar" | "he"
                | "iw"
                | "yi"
                | "ji"
                | "ur"
                | "fa"
                | "ps"
                | "sd"
                | "ks"
                | "ug"
                | "ku"
                | "ckb"
                | "dv"
                | "syr"
                | "nqo"
        )
    )
}

pub(crate) fn create_blank_xlsx(path: &str) -> Result<String, HandlerError> {
    use oxml::OxmlPackage;

    let workbook_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"
          xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets>
    <sheet name="Sheet1" sheetId="1" r:id="rId1"/>
  </sheets>
</workbook>"#;

    let sheet_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData></sheetData>
</worksheet>"#;

    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
  <Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
  <Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>
  <Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>
</Types>"#;

    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;

    let wb_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;

    let shared_strings = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="0" uniqueCount="0"/>"#;

    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <fonts count="1"><font><sz val="11"/><name val="Calibri"/></font></fonts>
  <fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>
  <borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>
  <cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>
  <cellXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/></cellXfs>
</styleSheet>"#;

    let mut pkg = OxmlPackage::create(path);
    pkg.add_part("[Content_Types].xml", content_types.as_bytes());
    pkg.add_part("_rels/.rels", rels.as_bytes());
    pkg.add_part("xl/workbook.xml", workbook_xml.as_bytes());
    pkg.add_part("xl/_rels/workbook.xml.rels", wb_rels.as_bytes());
    pkg.add_part("xl/worksheets/sheet1.xml", sheet_xml.as_bytes());
    pkg.add_part("xl/sharedStrings.xml", shared_strings.as_bytes());
    pkg.add_part("xl/styles.xml", styles.as_bytes());

    pkg.save_as(path)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("Created blank Excel workbook: {}", path))
}

pub(crate) fn create_blank_pptx(path: &str) -> Result<String, HandlerError> {
    use oxml::OxmlPackage;

    let presentation_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
               xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
               xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldMasterIdLst/>
  <p:sldIdLst>
    <p:sldId id="256" r:id="rId2"/>
  </p:sldIdLst>
  <p:sldSz cx="9144000" cy="6858000" type="screen4x3"/>
</p:presentation>"#;

    let slide_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld>
    <p:spTree>
      <p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
      <p:grpSpPr/>
    </p:spTree>
  </p:cSld>
</p:sld>"#;

    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
  <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
  <Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
</Types>"#;

    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#;

    let pres_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#;

    // A presentation-level theme keeps the compact Rust blank scaffold usable by
    // the same `/theme` and `defaultFont` commands C# supports on its richer
    // master/layout scaffold. Office accepts this standard relationship directly
    // from presentation.xml; imported decks may instead attach it to a master.
    let theme_xml = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme"><a:themeElements><a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#;

    let mut pkg = OxmlPackage::create(path);
    pkg.add_part("[Content_Types].xml", content_types.as_bytes());
    pkg.add_part("_rels/.rels", rels.as_bytes());
    pkg.add_part("ppt/presentation.xml", presentation_xml.as_bytes());
    pkg.add_part("ppt/_rels/presentation.xml.rels", pres_rels.as_bytes());
    pkg.add_part("ppt/slides/slide1.xml", slide_xml.as_bytes());
    pkg.add_part("ppt/theme/theme1.xml", theme_xml.as_bytes());

    pkg.save_as(path)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;
    Ok(format!("Created blank PowerPoint presentation: {}", path))
}

pub(crate) fn create_blank_pdf(path: &str) -> Result<String, HandlerError> {
    use lopdf::{dictionary, Document, Object, Stream};

    let mut doc = Document::with_version("1.4");

    // Pages root ID
    let pages_id = doc.new_object_id();

    // Page 1 ID
    let page_id = doc.new_object_id();

    // Content stream ID (empty content)
    let content_dict = dictionary! {};
    let content_id = doc.add_object(Object::Stream(Stream::new(content_dict, vec![])));

    // Page dictionary
    let page_dict = dictionary! {
        "Type" => Object::Name(b"Page".to_vec()),
        "Parent" => Object::Reference(pages_id),
        "MediaBox" => Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(595),
            Object::Integer(842),
        ]),
        "Resources" => Object::Dictionary(dictionary! {}),
        "Contents" => Object::Reference(content_id),
    };
    doc.set_object(page_id, Object::Dictionary(page_dict));

    // Pages dictionary
    let pages_dict = dictionary! {
        "Type" => Object::Name(b"Pages".to_vec()),
        "Kids" => Object::Array(vec![Object::Reference(page_id)]),
        "Count" => Object::Integer(1),
    };
    doc.set_object(pages_id, Object::Dictionary(pages_dict));

    // Catalog dictionary
    let catalog_id = doc.new_object_id();
    let catalog_dict = dictionary! {
        "Type" => Object::Name(b"Catalog".to_vec()),
        "Pages" => Object::Reference(pages_id),
    };
    doc.set_object(catalog_id, Object::Dictionary(catalog_dict));

    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(path)
        .map_err(|e| HandlerError::SaveError(e.to_string()))?;

    Ok(format!("Created blank PDF document: {}", path))
}
