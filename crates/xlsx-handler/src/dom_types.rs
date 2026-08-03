/// Core data types for the xlsx DOM model.
use std::collections::HashMap;

/// Cell reference parsed from a string like "A1", "B12", etc.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CellRef {
    pub col: usize, // 1-based column number (1=A, 2=B, ...)
    pub row: usize, // 1-based row number
}

impl CellRef {
    /// Parse a cell reference string like "A1" into column + row numbers.
    pub fn parse(ref_str: &str) -> Option<Self> {
        let col_part: String = ref_str
            .chars()
            .filter(|c| c.is_ascii_alphabetic())
            .collect();
        let row_part: String = ref_str.chars().filter(|c| c.is_ascii_digit()).collect();

        if col_part.is_empty() || row_part.is_empty() {
            return None;
        }

        let col = col_letters_to_num(&col_part)?;
        let row = row_part.parse::<usize>().ok()?;

        if col == 0 || row == 0 {
            return None;
        }

        Some(Self { col, row })
    }

    /// Format as a cell reference string, e.g. "A1".
    pub fn to_string_ref(&self) -> String {
        format!("{}{}", col_num_to_letters(self.col), self.row)
    }
}

/// Convert column letters to 1-based number: A=1, B=2, Z=26, AA=27.
fn col_letters_to_num(letters: &str) -> Option<usize> {
    let mut num: usize = 0;
    for ch in letters.chars() {
        if !ch.is_ascii_uppercase() {
            return None;
        }
        num = num * 26 + (ch as usize - 'A' as usize + 1);
    }
    Some(num)
}

/// Convert 1-based column number to letters: 1=A, 26=Z, 27=AA.
pub fn col_num_to_letters(num: usize) -> String {
    let mut letters = String::new();
    let mut n = num;
    while n > 0 {
        n -= 1;
        letters.push((b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    letters.chars().rev().collect()
}

/// Cell value type as defined by the x:c/@t attribute.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValueType {
    /// Default: numeric
    Number,
    /// t="s" — shared string reference
    SharedString,
    /// t="str" — inline string (formula result or literal)
    InlineString,
    /// t="b" — boolean
    Boolean,
    /// t="e" — error
    Error,
}

impl CellValueType {
    pub fn from_attr(t: Option<&str>) -> Self {
        match t {
            Some("s") => Self::SharedString,
            // Both `t="str"` (formula result string) and `t="inlineStr"`
            // (literal `<is><t>…</t></is>` content) are inline strings.
            Some("str") | Some("inlineStr") => Self::InlineString,
            Some("b") => Self::Boolean,
            Some("e") => Self::Error,
            None | Some("n") => Self::Number,
            Some(_other) => Self::Number, // fallback for unknown types
        }
    }
}

/// A single cell in a worksheet.
#[derive(Debug, Clone)]
pub struct Cell {
    /// Cell reference (e.g. A1)
    pub ref_str: String,
    /// Parsed column (1-based)
    pub col: usize,
    /// Parsed row (1-based)
    pub row: usize,
    /// Value type
    pub value_type: CellValueType,
    /// Raw value from x:v element
    pub raw_value: Option<String>,
    /// Formula from x:f element
    pub formula: Option<String>,
    /// Resolved display value (after shared string lookup, etc.)
    pub display_value: String,
    /// Style index (x:c/@s)
    pub style_index: Option<usize>,
    /// Value-metadata index (x:c/@vm), used by Excel rich values.
    pub value_metadata_index: Option<usize>,
}

/// A parsed worksheet.
#[derive(Debug, Clone)]
pub struct Worksheet {
    /// Sheet name (from workbook.xml)
    pub name: String,
    /// Sheet index (1-based, from workbook ordering)
    pub index: usize,
    /// Part path within the ZIP (e.g. "xl/worksheets/sheet1.xml")
    pub part_path: String,
    /// Relationship ID (r:id)
    pub rel_id: String,
    /// Cells keyed by (row, col)
    pub cells: HashMap<(usize, usize), Cell>,
    /// Maximum column that has data (1-based)
    pub max_col: usize,
    /// Maximum row that has data (1-based)
    pub max_row: usize,
}

/// Workbook model: sheets + shared strings.
#[derive(Debug, Clone)]
pub struct WorkbookModel {
    /// Ordered list of worksheets
    pub sheets: Vec<Worksheet>,
    /// Shared string table (index -> string)
    pub shared_strings: Vec<String>,
    /// Pivot table definitions found in xl/pivotTables/
    pub pivot_tables: Vec<PivotTableDef>,
    /// ListObjects (Excel Tables) parsed from xl/tables/tableN.xml.
    pub tables: Vec<ListObjectDef>,
}

/// An Excel ListObject ("Table") parsed from xl/tables/tableN.xml.
/// Carries the column-name → absolute-column-index map that the
/// `row[col op val]` predicate resolver uses to match data rows.
#[derive(Debug, Clone)]
pub struct ListObjectDef {
    pub name: String,
    pub display_name: String,
    /// Sheet name this table lives on. Resolved via the worksheet that owns
    /// the table's reference range (we walk each sheet's cells).
    pub sheet_name: String,
    /// Part path of the table definition, e.g. `xl/tables/table1.xml`.
    pub part_path: String,
    /// Reference range as (start_row, start_col, end_row, end_col), 1-based.
    pub range: (usize, usize, usize, usize),
    /// Column display names in left-to-right order.
    pub columns: Vec<String>,
    /// Whether the first row of `range` is a header (default true).
    pub header_row: bool,
    /// Whether the last row of `range` is a totals row (default false).
    pub totals_row: bool,
}

/// A pivot table definition extracted from xl/pivotTables/*.xml.
#[derive(Debug, Clone, Default)]
pub struct PivotTableDef {
    /// Pivot table name from the definition XML
    pub name: String,
    /// Cache ID referencing the pivot cache definition
    pub cache_id: Option<String>,
    /// Source range from the cache definition (e.g. "Sheet1!A1:E100")
    pub source_range: Option<String>,
    /// Number of fields (columns) in the pivot table
    pub field_count: usize,
    /// Part path within the ZIP (e.g. "xl/pivotTables/pivotTable1.xml")
    pub part_path: String,
    /// Location reference (e.g. "Sheet1!A3:D20")
    pub location: Option<String>,
    /// Cache field names (from cache definition). Indexed by pivot field index.
    pub cache_fields: Vec<String>,
    /// Indices of pivot fields used as row fields.
    pub row_fields: Vec<i32>,
    /// Indices of pivot fields used as column fields.
    pub col_fields: Vec<i32>,
    /// Indices of pivot fields used as page/filter fields.
    pub page_fields: Vec<i32>,
    /// Data field specs: (name, function, source_field_index).
    pub data_fields: Vec<(String, String, i32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_ref_parse() {
        let cr = CellRef::parse("A1").unwrap();
        assert_eq!(cr.col, 1);
        assert_eq!(cr.row, 1);

        let cr = CellRef::parse("Z10").unwrap();
        assert_eq!(cr.col, 26);
        assert_eq!(cr.row, 10);

        let cr = CellRef::parse("AA100").unwrap();
        assert_eq!(cr.col, 27);
        assert_eq!(cr.row, 100);
    }

    #[test]
    fn test_col_num_to_letters() {
        assert_eq!(col_num_to_letters(1), "A");
        assert_eq!(col_num_to_letters(26), "Z");
        assert_eq!(col_num_to_letters(27), "AA");
        assert_eq!(col_num_to_letters(52), "AZ");
        assert_eq!(col_num_to_letters(702), "ZZ");
    }

    #[test]
    fn test_cell_ref_roundtrip() {
        for ref_str in &["A1", "B5", "Z26", "AA100", "AZ50"] {
            let cr = CellRef::parse(ref_str).unwrap();
            assert_eq!(cr.to_string_ref(), *ref_str);
        }
    }
}
