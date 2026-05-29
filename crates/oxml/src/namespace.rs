/// OOXML namespace constants used across Word/Excel/PowerPoint.
pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
pub const X: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
pub const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
pub const XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
pub const WPS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingShape";
pub const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
pub const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
pub const CT: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
pub const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub const DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
pub const WP14: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
pub const V: &str = "urn:schemas-microsoft-com:vml";
pub const O: &str = "urn:schemas-microsoft-com:office:office";
pub const M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
pub const W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
pub const W15: &str = "http://schemas.microsoft.com/office/word/2012/wordml";
pub const X14: &str = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main";
pub const X15: &str = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main";

/// Map of common namespace prefixes to their URIs.
pub fn common_namespaces() -> Vec<(String, String)> {
    vec![
        ("w".to_string(), W.to_string()),
        ("r".to_string(), R.to_string()),
        ("a".to_string(), A.to_string()),
        ("p".to_string(), P.to_string()),
        ("x".to_string(), X.to_string()),
        ("wp".to_string(), WP.to_string()),
        ("xdr".to_string(), XDR.to_string()),
        ("wps".to_string(), WPS.to_string()),
        ("mc".to_string(), MC.to_string()),
        ("c".to_string(), C.to_string()),
        ("dgm".to_string(), DGM.to_string()),
    ]
}
