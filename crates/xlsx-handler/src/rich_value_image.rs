//! Excel 365 in-cell images (rich values).
//!
//! A rich-value image is deliberately separate from SpreadsheetDrawing: the
//! cell holds an error fallback plus a `vm` pointer, while `xl/richData/*`
//! and `xl/metadata.xml` resolve that pointer to a raster media part.

use handler_common::HandlerError;
use oxml::OxmlPackage;

const RICH_DATA_NS: &str = "http://schemas.microsoft.com/office/spreadsheetml/2017/richdata";
const RICH_VALUE_REL_NS: &str =
    "http://schemas.microsoft.com/office/spreadsheetml/2022/richvaluerel";
const OOXML_REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const RICH_VALUE_REL_TYPE: &str =
    "http://schemas.microsoft.com/office/2022/10/relationships/richValueRel";
const RICH_VALUE_REL_CONTENT_TYPE: &str = "application/vnd.ms-excel.richvaluerel+xml";
const XLRICHVALUE_EXT_URI: &str = "{3e2802c4-a4d2-4d8b-9148-e3be6c30e623}";

const RICH_VALUE_REL_PART: &str = "xl/richData/richValueRel.xml";
const RICH_VALUE_PART: &str = "xl/richData/rdrichvalue.xml";
const RICH_VALUE_STRUCTURE_PART: &str = "xl/richData/rdrichvaluestructure.xml";
const RICH_VALUE_TYPES_PART: &str = "xl/richData/rdRichValueTypes.xml";
const METADATA_PART: &str = "xl/metadata.xml";

const RICH_VALUE_TYPES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<rvTypesInfo xmlns="http://schemas.microsoft.com/office/spreadsheetml/2017/richdata2" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" mc:Ignorable="x" xmlns:x="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><global><keyFlags><key name="_Self"><flag name="ExcludeFromFile" value="1"/><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_DisplayString"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Flags"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Format"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_SubLabel"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Attribution"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Icon"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_Display"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_CanonicalPropertyNames"><flag name="ExcludeFromCalcComparison" value="1"/></key><key name="_ClassificationId"><flag name="ExcludeFromCalcComparison" value="1"/></key></keyFlags></global></rvTypesInfo>"#;

/// Metadata exposed for a resolved rich-value image cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub content_type: String,
    pub byte_size: usize,
    pub alt: Option<String>,
}

/// Resolve a `cell@vm` value through all rich-value parts. Foreign or damaged
/// packages intentionally return `None`: callers can retain their normal cell
/// representation instead of failing an otherwise readable workbook.
pub fn read_image_info(package: &OxmlPackage, vm: usize) -> Option<ImageInfo> {
    let metadata_xml = package.read_part_xml(METADATA_PART).ok()?;
    let metadata = roxmltree::Document::parse(&metadata_xml).ok()?;
    let value_metadata = metadata
        .descendants()
        .find(|node| node.has_tag_name("valueMetadata"))?;
    let rc = value_metadata
        .children()
        .filter(|node| node.has_tag_name("bk"))
        .nth(vm.checked_sub(1)?)?
        .descendants()
        .find(|node| node.has_tag_name("rc"))?;
    let type_index = rc.attribute("t")?.parse::<usize>().ok()?;
    let future_index = rc.attribute("v")?.parse::<usize>().ok()?;
    let metadata_type = metadata
        .descendants()
        .filter(|node| node.has_tag_name("metadataType"))
        .nth(type_index.checked_sub(1)?)?;
    if metadata_type.attribute("name")? != "XLRICHVALUE" {
        return None;
    }
    let future = metadata.descendants().find(|node| {
        node.has_tag_name("futureMetadata") && node.attribute("name") == Some("XLRICHVALUE")
    })?;
    let rich_value_index = future
        .children()
        .filter(|node| node.has_tag_name("bk"))
        .nth(future_index)?
        .descendants()
        .find(|node| node.tag_name().name() == "rvb")?
        .attribute("i")?
        .parse::<usize>()
        .ok()?;

    let values_xml = package.read_part_xml(RICH_VALUE_PART).ok()?;
    let values = roxmltree::Document::parse(&values_xml).ok()?;
    let rich_value = values
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("rv"))
        .nth(rich_value_index)?;
    let structure_index = rich_value.attribute("s")?.parse::<usize>().ok()?;
    let structures_xml = package.read_part_xml(RICH_VALUE_STRUCTURE_PART).ok()?;
    let structures = roxmltree::Document::parse(&structures_xml).ok()?;
    let structure = structures
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("s"))
        .nth(structure_index)?;
    if structure.attribute("t") != Some("_localImage") {
        return None;
    }
    let keys: Vec<_> = structure
        .children()
        .filter(|node| node.has_tag_name("k"))
        .filter_map(|node| node.attribute("n"))
        .collect();
    let rich_values: Vec<_> = rich_value
        .children()
        .filter(|node| node.has_tag_name("v"))
        .filter_map(|node| node.text())
        .collect();
    let image_key = keys
        .iter()
        .position(|key| *key == "_rvRel:LocalImageIdentifier")?;
    let rel_index = rich_values.get(image_key)?.parse::<usize>().ok()?;
    let alt = keys
        .iter()
        .position(|key| *key == "Text")
        .and_then(|index| rich_values.get(index))
        .map(|value| (*value).to_string());
    let rich_rel_xml = package.read_part_xml(RICH_VALUE_REL_PART).ok()?;
    let rich_rels = roxmltree::Document::parse(&rich_rel_xml).ok()?;
    let rel_id = rich_rels
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("rel"))
        .nth(rel_index)?
        .attributes()
        .iter()
        .find(|attribute| attribute.name().ends_with("id"))?
        .value();
    let rels_xml = package
        .read_part_xml("xl/richData/_rels/richValueRel.xml.rels")
        .ok()?;
    let rels = roxmltree::Document::parse(&rels_xml).ok()?;
    let target = rels
        .root_element()
        .children()
        .find(|node| node.has_tag_name("Relationship") && node.attribute("Id") == Some(rel_id))?
        .attribute("Target")?;
    let image_part = resolve_relationship_target(RICH_VALUE_REL_PART, target)?;
    let data = package.read_part_bytes(&image_part).ok()?;
    let content_type = content_type_for_part(package, &image_part)?;
    if !content_type
        .get(..6)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
    {
        return None;
    }
    Some(ImageInfo {
        content_type,
        byte_size: data.len(),
        alt,
    })
}

/// OOXML relationship targets are URI-like paths relative to the relationship
/// owner's part.  Do not assume Excel's usual `../media/*` spelling: packages
/// produced by other tools can use `./`, multiple parent segments, or an
/// absolute package target.  Escaping the package root is treated as invalid.
fn resolve_relationship_target(owner_part: &str, target: &str) -> Option<String> {
    let target = target.split('#').next()?;
    if target.is_empty() || target.contains("://") {
        return None;
    }
    let mut components: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        owner_part
            .rsplit_once('/')
            .map(|(dir, _)| dir.split('/').collect())?
    };
    for component in target.trim_start_matches('/').split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value),
        }
    }
    (!components.is_empty()).then(|| components.join("/"))
}

/// Content type belongs to the OOXML package, not the filesystem extension.
/// Third-party workbooks are allowed to use nonstandard media file names, so
/// mirror `ImagePart.ContentType` by consulting `[Content_Types].xml` first.
fn content_type_for_part(package: &OxmlPackage, part: &str) -> Option<String> {
    let content_types = package.read_part_xml("[Content_Types].xml").ok()?;
    content_type_from_manifest(&content_types, part)
}

fn content_type_from_manifest(manifest: &str, part: &str) -> Option<String> {
    let document = roxmltree::Document::parse(manifest).ok()?;
    let part_name = format!("/{}", part.trim_start_matches('/'));
    if let Some(content_type) = document
        .descendants()
        .find(|node| {
            node.has_tag_name("Override") && node.attribute("PartName") == Some(&part_name)
        })
        .and_then(|node| node.attribute("ContentType"))
    {
        return Some(content_type.to_string());
    }

    let extension = std::path::Path::new(part).extension()?.to_str()?;
    document
        .descendants()
        .find(|node| {
            node.has_tag_name("Default")
                && node
                    .attribute("Extension")
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        })
        .and_then(|node| node.attribute("ContentType"))
        .map(str::to_string)
}

/// Materialize an image at `source` as a value-semantic Excel cell image and
/// return the 1-based `vm` index to store on the cell.
pub fn add_image(
    package: &mut OxmlPackage,
    source: &str,
    alt: Option<&str>,
) -> Result<usize, HandlerError> {
    let (extension, content_type) = image_kind(source)?;
    let bytes = std::fs::read(source).map_err(|error| {
        HandlerError::OperationFailed(format!(
            "failed to read in-cell image '{}': {error}",
            source
        ))
    })?;
    let image_path = next_image_part(package, extension);
    package
        .write_part(&image_path, bytes)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;

    ensure_default_content_type(package, extension, content_type)?;
    ensure_rich_data_parts(package)?;

    let rels_path = "xl/richData/_rels/richValueRel.xml.rels";
    let image_rel_id = next_rel_id(package, rels_path);
    append_relationship(
        package,
        rels_path,
        &format!(
            "<Relationship Id=\"{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"../media/{}\"/>",
            image_rel_id,
            image_path.rsplit('/').next().unwrap_or_default()
        ),
    )?;

    let rel_index = append_rich_value_rel(package, &image_rel_id)?;
    let structure_index = ensure_structure(package, alt.is_some())?;
    let rich_value_index = append_rich_value(package, structure_index, rel_index, alt)?;
    ensure_metadata(package, rich_value_index)
}

fn image_kind(source: &str) -> Result<(&'static str, &'static str), HandlerError> {
    let extension = std::path::Path::new(source)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" => Ok(("png", "image/png")),
        "jpg" | "jpeg" => Ok(("jpeg", "image/jpeg")),
        "gif" => Ok(("gif", "image/gif")),
        "bmp" => Ok(("bmp", "image/bmp")),
        "tif" | "tiff" => Ok(("tiff", "image/tiff")),
        "webp" => Ok(("webp", "image/webp")),
        "svg" => Err(HandlerError::InvalidArgument(
            "In-cell images do not support SVG; convert it to PNG or use a floating image"
                .to_string(),
        )),
        _ => Err(HandlerError::InvalidArgument(
            "in-cell image must be a raster file (png, jpg, gif, bmp, tiff, or webp)".to_string(),
        )),
    }
}

fn next_image_part(package: &OxmlPackage, extension: &str) -> String {
    let mut index = 1;
    loop {
        let path = format!("xl/media/image{index}.{extension}");
        if !package.has_part(&path) {
            return path;
        }
        index += 1;
    }
}

fn ensure_rich_data_parts(package: &mut OxmlPackage) -> Result<(), HandlerError> {
    ensure_override(package, RICH_VALUE_REL_PART, RICH_VALUE_REL_CONTENT_TYPE)?;
    ensure_override(
        package,
        RICH_VALUE_PART,
        "application/vnd.ms-excel.rdrichvalue+xml",
    )?;
    ensure_override(
        package,
        RICH_VALUE_STRUCTURE_PART,
        "application/vnd.ms-excel.rdrichvaluestructure+xml",
    )?;
    ensure_override(
        package,
        RICH_VALUE_TYPES_PART,
        "application/vnd.ms-excel.rdrichvaluetypes+xml",
    )?;
    ensure_override(
        package,
        METADATA_PART,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml",
    )?;

    if !package.has_part(RICH_VALUE_REL_PART) {
        write_xml(package, RICH_VALUE_REL_PART, &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><richValueRels xmlns=\"{}\" xmlns:r=\"{}\"/>",
            RICH_VALUE_REL_NS, OOXML_REL_NS
        ))?;
    }
    if !package.has_part(RICH_VALUE_PART) {
        write_xml(package, RICH_VALUE_PART, &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><rvData xmlns=\"{}\" count=\"0\"/>",
            RICH_DATA_NS
        ))?;
    }
    if !package.has_part(RICH_VALUE_STRUCTURE_PART) {
        write_xml(package, RICH_VALUE_STRUCTURE_PART, &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><rvStructures xmlns=\"{}\" count=\"0\"/>",
            RICH_DATA_NS
        ))?;
    }
    if !package.has_part(RICH_VALUE_TYPES_PART) {
        write_xml(package, RICH_VALUE_TYPES_PART, RICH_VALUE_TYPES_XML)?;
    }

    let wb_rels = "xl/_rels/workbook.xml.rels";
    ensure_workbook_relationship(package, wb_rels, RICH_VALUE_REL_PART, RICH_VALUE_REL_TYPE)?;
    ensure_workbook_relationship(
        package,
        wb_rels,
        RICH_VALUE_PART,
        "http://schemas.microsoft.com/office/2017/10/relationships/richValue",
    )?;
    ensure_workbook_relationship(
        package,
        wb_rels,
        RICH_VALUE_STRUCTURE_PART,
        "http://schemas.microsoft.com/office/2017/10/relationships/richValueStructure",
    )?;
    ensure_workbook_relationship(
        package,
        wb_rels,
        RICH_VALUE_TYPES_PART,
        "http://schemas.microsoft.com/office/2017/10/relationships/rdRichValueTypes",
    )?;
    ensure_workbook_relationship(
        package,
        wb_rels,
        METADATA_PART,
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata",
    )?;
    Ok(())
}

fn append_rich_value_rel(
    package: &mut OxmlPackage,
    image_rel_id: &str,
) -> Result<usize, HandlerError> {
    let xml = read_xml(package, RICH_VALUE_REL_PART)?;
    let index = count_elements(&xml, "rel");
    let updated = insert_before_close(
        &xml,
        "richValueRels",
        &format!("<rel r:id=\"{}\"/>", image_rel_id),
    )?;
    write_xml(package, RICH_VALUE_REL_PART, &updated)?;
    Ok(index)
}

fn ensure_structure(package: &mut OxmlPackage, with_alt: bool) -> Result<usize, HandlerError> {
    let xml = read_xml(package, RICH_VALUE_STRUCTURE_PART)?;
    let keys = if with_alt {
        "<k n=\"_rvRel:LocalImageIdentifier\" t=\"i\"/><k n=\"CalcOrigin\" t=\"i\"/><k n=\"Text\" t=\"s\"/>"
    } else {
        "<k n=\"_rvRel:LocalImageIdentifier\" t=\"i\"/><k n=\"CalcOrigin\" t=\"i\"/>"
    };
    let expected = format!("t=\"_localImage\">{keys}");
    if let Some(index) = xml
        .match_indices("<s ")
        .enumerate()
        .find_map(|(index, (offset, _))| {
            xml[offset..].find("</s>").and_then(|end| {
                xml[offset..offset + end]
                    .contains(&expected)
                    .then_some(index)
            })
        })
    {
        return Ok(index);
    }
    let index = count_elements(&xml, "s");
    let updated = insert_before_close(
        &xml,
        "rvStructures",
        &format!("<s t=\"_localImage\">{keys}</s>"),
    )?;
    let updated = set_root_count(updated, "rvStructures", index + 1)?;
    write_xml(package, RICH_VALUE_STRUCTURE_PART, &updated)?;
    Ok(index)
}

fn append_rich_value(
    package: &mut OxmlPackage,
    structure_index: usize,
    rel_index: usize,
    alt: Option<&str>,
) -> Result<usize, HandlerError> {
    let xml = read_xml(package, RICH_VALUE_PART)?;
    let index = count_elements(&xml, "rv");
    let mut values = format!("<v>{rel_index}</v><v>5</v>");
    if let Some(alt) = alt {
        values.push_str(&format!("<v>{}</v>", escape_xml(alt)));
    }
    let updated = insert_before_close(
        &xml,
        "rvData",
        &format!("<rv s=\"{structure_index}\">{values}</rv>"),
    )?;
    let updated = set_root_count(updated, "rvData", index + 1)?;
    write_xml(package, RICH_VALUE_PART, &updated)?;
    Ok(index)
}

/// Append only our metadata records. Existing metadata types and dynamic-array
/// value metadata stay in their original order and retain their indices.
fn ensure_metadata(
    package: &mut OxmlPackage,
    rich_value_index: usize,
) -> Result<usize, HandlerError> {
    if !package.has_part(METADATA_PART) {
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><metadata xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><metadataTypes count=\"1\"><metadataType name=\"XLRICHVALUE\" minSupportedVersion=\"120000\" copy=\"1\" pasteAll=\"1\" pasteValues=\"1\" merge=\"1\" splitFirst=\"1\" rowColumnShift=\"1\" clearFormats=\"1\" clearComments=\"1\" assign=\"1\" coerce=\"1\"/></metadataTypes><futureMetadata name=\"XLRICHVALUE\" count=\"1\"><bk><extLst><ext uri=\"{}\"><xlrd:rvb xmlns:xlrd=\"{}\" i=\"{}\"/></ext></extLst></bk></futureMetadata><valueMetadata count=\"1\"><bk><rc t=\"1\" v=\"0\"/></bk></valueMetadata></metadata>",
            XLRICHVALUE_EXT_URI, RICH_DATA_NS, rich_value_index
        );
        write_xml(package, METADATA_PART, &xml)?;
        return Ok(1);
    }

    let mut xml = read_xml(package, METADATA_PART)?;
    let type_index = ensure_metadata_type(&mut xml)?;
    let future_index = append_future_metadata(&mut xml, rich_value_index)?;
    let vm = append_value_metadata(&mut xml, type_index, future_index)?;
    write_xml(package, METADATA_PART, &xml)?;
    Ok(vm)
}

fn ensure_metadata_type(xml: &mut String) -> Result<usize, HandlerError> {
    let count = count_elements(xml, "metadataType");
    if let Some(index) =
        xml.match_indices("<metadataType ")
            .enumerate()
            .find_map(|(index, (offset, _))| {
                xml[offset..].find('>').and_then(|end| {
                    xml[offset..offset + end]
                        .contains("name=\"XLRICHVALUE\"")
                        .then_some(index + 1)
                })
            })
    {
        return Ok(index);
    }
    let entry = "<metadataType name=\"XLRICHVALUE\" minSupportedVersion=\"120000\" copy=\"1\" pasteAll=\"1\" pasteValues=\"1\" merge=\"1\" splitFirst=\"1\" rowColumnShift=\"1\" clearFormats=\"1\" clearComments=\"1\" assign=\"1\" coerce=\"1\"/>";
    *xml = insert_before_close(xml, "metadataTypes", entry)?;
    *xml = set_root_count(xml.clone(), "metadataTypes", count + 1)?;
    Ok(count + 1)
}

fn append_future_metadata(
    xml: &mut String,
    rich_value_index: usize,
) -> Result<usize, HandlerError> {
    let entry = format!(
        "<bk><extLst><ext uri=\"{}\"><xlrd:rvb xmlns:xlrd=\"{}\" i=\"{}\"/></ext></extLst></bk>",
        XLRICHVALUE_EXT_URI, RICH_DATA_NS, rich_value_index
    );
    if xml.contains("<futureMetadata name=\"XLRICHVALUE\"") {
        let index =
            count_children_in_named_block(xml, "futureMetadata", "name=\"XLRICHVALUE\"", "bk")?;
        *xml = insert_before_named_close(xml, "futureMetadata", "name=\"XLRICHVALUE\"", &entry)?;
        *xml = set_named_count(
            xml.clone(),
            "futureMetadata",
            "name=\"XLRICHVALUE\"",
            index + 1,
        )?;
        return Ok(index);
    }
    let block =
        format!("<futureMetadata name=\"XLRICHVALUE\" count=\"1\">{entry}</futureMetadata>");
    *xml = insert_before_close(xml, "metadata", &block)?;
    Ok(0)
}

fn append_value_metadata(
    xml: &mut String,
    type_index: usize,
    future_index: usize,
) -> Result<usize, HandlerError> {
    let entry = format!("<bk><rc t=\"{type_index}\" v=\"{future_index}\"/></bk>");
    if xml.contains("<valueMetadata") {
        let index = count_children_in_named_block(xml, "valueMetadata", "", "bk")?;
        *xml = insert_before_close(xml, "valueMetadata", &entry)?;
        *xml = set_root_count(xml.clone(), "valueMetadata", index + 1)?;
        return Ok(index + 1);
    }
    let existing = 0;
    let block = format!("<valueMetadata count=\"1\">{entry}</valueMetadata>");
    *xml = insert_before_close(xml, "metadata", &block)?;
    Ok(existing + 1)
}

fn ensure_workbook_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    part: &str,
    relationship_type: &str,
) -> Result<(), HandlerError> {
    let target = part.strip_prefix("xl/").unwrap_or(part);
    let target = target
        .strip_prefix("richData/")
        .map(|path| format!("richData/{path}"))
        .unwrap_or_else(|| target.to_string());
    let existing = package.read_part_xml(rels_path).unwrap_or_default();
    if existing.contains(&format!("Type=\"{relationship_type}\"")) {
        return Ok(());
    }
    let id = next_rel_id(package, rels_path);
    append_relationship(
        package,
        rels_path,
        &format!("<Relationship Id=\"{id}\" Type=\"{relationship_type}\" Target=\"{target}\"/>"),
    )
}

fn ensure_default_content_type(
    package: &mut OxmlPackage,
    extension: &str,
    content_type: &str,
) -> Result<(), HandlerError> {
    let xml = read_xml(package, "[Content_Types].xml")?;
    if xml.contains(&format!("Extension=\"{extension}\"")) {
        return Ok(());
    }
    let updated = insert_after_root_open(
        &xml,
        "Types",
        &format!("<Default Extension=\"{extension}\" ContentType=\"{content_type}\"/>"),
    )?;
    write_xml(package, "[Content_Types].xml", &updated)
}

fn ensure_override(
    package: &mut OxmlPackage,
    part: &str,
    content_type: &str,
) -> Result<(), HandlerError> {
    let xml = read_xml(package, "[Content_Types].xml")?;
    if xml.contains(&format!("PartName=\"/{part}\"")) {
        return Ok(());
    }
    let updated = insert_before_close(
        &xml,
        "Types",
        &format!("<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/>"),
    )?;
    write_xml(package, "[Content_Types].xml", &updated)
}

fn append_relationship(
    package: &mut OxmlPackage,
    rels_path: &str,
    relationship: &str,
) -> Result<(), HandlerError> {
    let xml = package.read_part_xml(rels_path).unwrap_or_else(|_| {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>".to_string()
    });
    let updated = insert_before_close(&xml, "Relationships", relationship)?;
    write_xml(package, rels_path, &updated)
}

fn next_rel_id(package: &OxmlPackage, rels_path: &str) -> String {
    let xml = package.read_part_xml(rels_path).unwrap_or_default();
    let max = xml
        .match_indices("Id=\"rId")
        .filter_map(|(offset, _)| {
            xml[offset + 7..]
                .find('"')
                .and_then(|end| xml[offset + 7..offset + 7 + end].parse::<usize>().ok())
        })
        .max()
        .unwrap_or(0);
    format!("rId{}", max + 1)
}

fn read_xml(package: &OxmlPackage, path: &str) -> Result<String, HandlerError> {
    package
        .read_part_xml(path)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))
}

fn write_xml(package: &mut OxmlPackage, path: &str, xml: &str) -> Result<(), HandlerError> {
    package
        .write_part_xml(path, xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn count_elements(xml: &str, local_name: &str) -> usize {
    let needle = format!("<{local_name}");
    xml.match_indices(&needle)
        .filter(|(offset, _)| {
            xml[*offset + needle.len()..]
                .chars()
                .next()
                .is_some_and(|ch| ch == ' ' || ch == '>' || ch == '/')
        })
        .count()
}

fn insert_before_close(xml: &str, tag: &str, addition: &str) -> Result<String, HandlerError> {
    let close = format!("</{tag}>");
    if let Some(offset) = xml.rfind(&close) {
        let mut out = String::with_capacity(xml.len() + addition.len());
        out.push_str(&xml[..offset]);
        out.push_str(addition);
        out.push_str(&xml[offset..]);
        return Ok(out);
    }
    let self_closing = format!("<{tag}");
    let start = xml
        .find(&self_closing)
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing <{tag}>")))?;
    let end = xml[start..]
        .find("/>")
        .map(|offset| start + offset)
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing closing <{tag}>")))?;
    let mut out = String::with_capacity(xml.len() + addition.len() + tag.len() + 3);
    out.push_str(&xml[..end]);
    out.push('>');
    out.push_str(addition);
    out.push_str(&close);
    out.push_str(&xml[end + 2..]);
    Ok(out)
}

fn insert_after_root_open(xml: &str, tag: &str, addition: &str) -> Result<String, HandlerError> {
    let start = xml
        .find(&format!("<{tag}"))
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing <{tag}>")))?;
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset + 1)
        .ok_or_else(|| HandlerError::OperationFailed(format!("unterminated <{tag}>")))?;
    let mut out = String::with_capacity(xml.len() + addition.len());
    out.push_str(&xml[..end]);
    out.push_str(addition);
    out.push_str(&xml[end..]);
    Ok(out)
}

fn set_root_count(xml: String, tag: &str, value: usize) -> Result<String, HandlerError> {
    set_named_count(xml, tag, "", value)
}

fn set_named_count(
    mut xml: String,
    tag: &str,
    required: &str,
    value: usize,
) -> Result<String, HandlerError> {
    let needle = format!("<{tag}");
    let start = xml
        .match_indices(&needle)
        .find_map(|(offset, _)| {
            xml[offset..]
                .find('>')
                .map(|end| (offset, offset + end))
                .filter(|(start, end)| required.is_empty() || xml[*start..*end].contains(required))
        })
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing <{tag}>")))?;
    let open = &xml[start.0..=start.1];
    let replacement = if let Some(count_at) = open.find("count=\"") {
        let value_start = count_at + 7;
        let value_end = open[value_start..]
            .find('"')
            .map(|end| value_start + end)
            .ok_or_else(|| HandlerError::OperationFailed("invalid count attribute".to_string()))?;
        format!("{}{}{}", &open[..value_start], value, &open[value_end..])
    } else {
        format!("{} count=\"{}\">", &open[..open.len() - 1], value)
    };
    xml.replace_range(start.0..=start.1, &replacement);
    Ok(xml)
}

fn insert_before_named_close(
    xml: &str,
    tag: &str,
    required: &str,
    addition: &str,
) -> Result<String, HandlerError> {
    let start = xml
        .find(&format!("<{tag}"))
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing <{tag}>")))?;
    let open_end = xml[start..]
        .find('>')
        .map(|end| start + end)
        .ok_or_else(|| HandlerError::OperationFailed("unterminated metadata block".to_string()))?;
    if !xml[start..open_end].contains(required) {
        return Err(HandlerError::OperationFailed(format!(
            "missing matching <{tag}>"
        )));
    }
    let close = format!("</{tag}>");
    let close_start = xml[open_end..]
        .find(&close)
        .map(|offset| open_end + offset)
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing </{tag}>")))?;
    let mut out = String::with_capacity(xml.len() + addition.len());
    out.push_str(&xml[..close_start]);
    out.push_str(addition);
    out.push_str(&xml[close_start..]);
    Ok(out)
}

fn count_children_in_named_block(
    xml: &str,
    tag: &str,
    required: &str,
    child: &str,
) -> Result<usize, HandlerError> {
    let start = xml
        .find(&format!("<{tag}"))
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing <{tag}>")))?;
    let end = xml[start..]
        .find(&format!("</{tag}>"))
        .map(|offset| start + offset)
        .ok_or_else(|| HandlerError::OperationFailed(format!("missing </{tag}>")))?;
    if !xml[start..end].contains(required) {
        return Err(HandlerError::OperationFailed(format!(
            "missing matching <{tag}>"
        )));
    }
    Ok(count_elements(&xml[start..end], child))
}

#[cfg(test)]
mod tests {
    use super::{content_type_from_manifest, resolve_relationship_target};

    #[test]
    fn content_type_uses_part_override_before_extension_default() {
        let manifest = r#"<?xml version="1.0" encoding="UTF-8"?>
            <Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
              <Default Extension="bin" ContentType="application/octet-stream"/>
              <Override PartName="/xl/media/image1.bin" ContentType="image/png"/>
            </Types>"#;
        assert_eq!(
            content_type_from_manifest(manifest, "xl/media/image1.bin"),
            Some("image/png".to_string())
        );
        assert_eq!(
            content_type_from_manifest(manifest, "/xl/media/image2.BIN"),
            Some("application/octet-stream".to_string())
        );
    }

    #[test]
    fn relationship_targets_resolve_like_package_parts() {
        assert_eq!(
            resolve_relationship_target("xl/richData/richValueRel.xml", "../media/image1.png"),
            Some("xl/media/image1.png".to_string())
        );
        assert_eq!(
            resolve_relationship_target("xl/richData/richValueRel.xml", "./../media/image1.png"),
            Some("xl/media/image1.png".to_string())
        );
        assert_eq!(
            resolve_relationship_target("xl/richData/richValueRel.xml", "/xl/media/image1.png"),
            Some("xl/media/image1.png".to_string())
        );
        assert_eq!(
            resolve_relationship_target("xl/richData/richValueRel.xml", "../../../image1.png"),
            None
        );
    }
}
