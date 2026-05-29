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
            Some("str") => Self::InlineString,
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
