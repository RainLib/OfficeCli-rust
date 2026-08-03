//! Excel 365 dynamic-array anchor metadata.
//!
//! A modern spill formula needs more than an array `<f>` element: Excel uses a
//! cell-metadata (`cm`) pointer to an XLDAPR record in `xl/metadata.xml` to
//! distinguish it from a legacy CSE array formula.

use crate::rich_value_image::{
    ensure_override, ensure_workbook_relationship, insert_before_close, insert_before_named_close,
    set_named_count,
};
use handler_common::HandlerError;
use oxml::OxmlPackage;

const METADATA_PART: &str = "xl/metadata.xml";
const METADATA_CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheetMetadata+xml";
const METADATA_REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sheetMetadata";
const XLDAPR_TYPE: &str = "<metadataType name=\"XLDAPR\" minSupportedVersion=\"120000\" copy=\"1\" pasteAll=\"1\" pasteValues=\"1\" merge=\"1\" splitFirst=\"1\" rowColShift=\"1\" clearFormats=\"1\" clearComments=\"1\" assign=\"1\" coerce=\"1\" cellMeta=\"1\"/>";
const XLDAPR_FUTURE: &str = "<bk><extLst><ext uri=\"{bdbb8cdc-fa1e-496e-a857-3c3f30c029c3}\"><xda:dynamicArrayProperties fDynamic=\"1\" fCollapsed=\"0\"/></ext></extLst></bk>";

/// Ensure an XLDAPR record exists and return its one-based `cm` index.
///
/// New workbooks receive the same single-record structure as the C# handler.
/// Existing metadata (such as rich-value image records) is retained and a
/// separate cell-metadata entry is appended when necessary.
pub fn ensure_metadata(package: &mut OxmlPackage) -> Result<usize, HandlerError> {
    ensure_override(package, METADATA_PART, METADATA_CONTENT_TYPE)?;
    ensure_workbook_relationship(
        package,
        "xl/_rels/workbook.xml.rels",
        METADATA_PART,
        METADATA_REL_TYPE,
    )?;

    if !package.has_part(METADATA_PART) {
        return write_new_metadata(package);
    }

    let existing = package
        .read_part_xml(METADATA_PART)
        .map_err(|error| HandlerError::OperationFailed(error.to_string()))?;
    let mut xml = ensure_xda_namespace(existing)?;
    let type_index = ensure_type(&mut xml)?;
    let future_index = ensure_future_metadata(&mut xml)?;
    let cm_index = ensure_cell_metadata(&mut xml, type_index, future_index)?;
    package
        .write_part_xml(METADATA_PART, &xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(cm_index)
}

fn write_new_metadata(package: &mut OxmlPackage) -> Result<usize, HandlerError> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><metadata xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:xda=\"http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray\"><metadataTypes count=\"1\">{XLDAPR_TYPE}</metadataTypes><futureMetadata name=\"XLDAPR\" count=\"1\">{XLDAPR_FUTURE}</futureMetadata><cellMetadata count=\"1\"><bk><rc t=\"1\" v=\"0\"/></bk></cellMetadata></metadata>"
    );
    package
        .write_part_xml(METADATA_PART, &xml)
        .map_err(|error| HandlerError::SaveError(error.to_string()))?;
    Ok(1)
}

fn ensure_xda_namespace(mut xml: String) -> Result<String, HandlerError> {
    if xml.contains("xmlns:xda=") {
        return Ok(xml);
    }
    let start = xml
        .find("<metadata")
        .ok_or_else(|| HandlerError::OperationFailed("missing <metadata>".to_string()))?;
    let end = xml[start..]
        .find('>')
        .map(|offset| start + offset)
        .ok_or_else(|| HandlerError::OperationFailed("unterminated <metadata>".to_string()))?;
    xml.insert_str(
        end,
        " xmlns:xda=\"http://schemas.microsoft.com/office/spreadsheetml/2017/dynamicarray\"",
    );
    Ok(xml)
}

fn ensure_type(xml: &mut String) -> Result<usize, HandlerError> {
    if let Some(index) = metadata_type_index(xml, "XLDAPR") {
        return Ok(index);
    }
    let index = metadata_type_count(xml) + 1;
    if xml.contains("<metadataTypes") {
        *xml = insert_before_close(xml, "metadataTypes", XLDAPR_TYPE)?;
        *xml = set_named_count(xml.clone(), "metadataTypes", "", index)?;
    } else {
        *xml = insert_before_close(
            xml,
            "metadata",
            &format!("<metadataTypes count=\"1\">{XLDAPR_TYPE}</metadataTypes>"),
        )?;
    }
    Ok(index)
}

fn ensure_future_metadata(xml: &mut String) -> Result<usize, HandlerError> {
    if let Some(index) = future_metadata_index(xml, "XLDAPR") {
        return Ok(index);
    }
    let index = future_metadata_count(xml, "XLDAPR");
    let block =
        format!("<futureMetadata name=\"XLDAPR\" count=\"1\">{XLDAPR_FUTURE}</futureMetadata>");
    *xml = insert_before_close(xml, "metadata", &block)?;
    Ok(index)
}

fn ensure_cell_metadata(
    xml: &mut String,
    type_index: usize,
    future_index: usize,
) -> Result<usize, HandlerError> {
    if let Some(index) = matching_cell_metadata_index(xml, type_index, future_index) {
        return Ok(index);
    }
    let next = cell_metadata_count(xml) + 1;
    let entry = format!("<bk><rc t=\"{type_index}\" v=\"{future_index}\"/></bk>");
    if xml.contains("<cellMetadata") {
        *xml = insert_before_named_close(xml, "cellMetadata", "", &entry)?;
        *xml = set_named_count(xml.clone(), "cellMetadata", "", next)?;
    } else {
        *xml = insert_before_close(
            xml,
            "metadata",
            &format!("<cellMetadata count=\"1\">{entry}</cellMetadata>"),
        )?;
    }
    Ok(next)
}

fn metadata_type_count(xml: &str) -> usize {
    roxmltree::Document::parse(xml)
        .map(|document| {
            document
                .descendants()
                .filter(|node| node.has_tag_name("metadataType"))
                .count()
        })
        .unwrap_or(0)
}

fn metadata_type_index(xml: &str, name: &str) -> Option<usize> {
    roxmltree::Document::parse(xml).ok().and_then(|document| {
        document
            .descendants()
            .filter(|node| node.has_tag_name("metadataType"))
            .position(|node| node.attribute("name") == Some(name))
            .map(|index| index + 1)
    })
}

fn future_metadata_count(xml: &str, name: &str) -> usize {
    roxmltree::Document::parse(xml)
        .map(|document| {
            document
                .descendants()
                .filter(|node| {
                    node.has_tag_name("futureMetadata") && node.attribute("name") == Some(name)
                })
                .count()
        })
        .unwrap_or(0)
}

fn future_metadata_index(xml: &str, name: &str) -> Option<usize> {
    roxmltree::Document::parse(xml).ok().and_then(|document| {
        document
            .descendants()
            .find(|node| {
                node.has_tag_name("futureMetadata") && node.attribute("name") == Some(name)
            })
            .map(|_| 0)
    })
}

fn cell_metadata_count(xml: &str) -> usize {
    roxmltree::Document::parse(xml)
        .ok()
        .and_then(|document| {
            document
                .descendants()
                .find(|node| node.has_tag_name("cellMetadata"))
                .map(|node| {
                    node.children()
                        .filter(|child| child.has_tag_name("bk"))
                        .count()
                })
        })
        .unwrap_or(0)
}

fn matching_cell_metadata_index(
    xml: &str,
    type_index: usize,
    future_index: usize,
) -> Option<usize> {
    roxmltree::Document::parse(xml).ok().and_then(|document| {
        document
            .descendants()
            .find(|node| node.has_tag_name("cellMetadata"))?
            .children()
            .filter(|node| node.has_tag_name("bk"))
            .position(|block| {
                block.descendants().any(|node| {
                    node.has_tag_name("rc")
                        && node.attribute("t") == Some(&type_index.to_string())
                        && node.attribute("v") == Some(&future_index.to_string())
                })
            })
            .map(|index| index + 1)
    })
}

#[cfg(test)]
mod tests {
    use super::{ensure_metadata, METADATA_PART};
    use oxml::OxmlPackage;

    #[test]
    fn creates_csharp_compatible_xldapr_metadata() {
        let mut package = OxmlPackage::create("dynamic-array-test.xlsx");
        package.add_part(
            "[Content_Types].xml",
            br#"<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>"#,
        );
        package.add_part(
            "xl/_rels/workbook.xml.rels",
            br#"<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>"#,
        );

        assert_eq!(ensure_metadata(&mut package).unwrap(), 1);
        let metadata = package.read_part_xml(METADATA_PART).unwrap();
        assert!(metadata.contains("name=\"XLDAPR\""));
        assert!(metadata.contains("xda:dynamicArrayProperties"));
        assert!(metadata.contains("<cellMetadata count=\"1\"><bk><rc t=\"1\" v=\"0\"/></bk>"));
        assert_eq!(ensure_metadata(&mut package).unwrap(), 1);
    }
}
