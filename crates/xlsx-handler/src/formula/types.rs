//! Formula evaluator core types.

/// Result of a formula evaluation. Can be numeric, string, boolean, error, or blank.
#[derive(Debug, Clone, PartialEq)]
pub enum FormulaResult {
    Number(f64),
    Str(String),
    Bool(bool),
    Error(String),
    /// A legacy one-dimensional numeric array.  Keep this variant for the
    /// established aggregate/lookup evaluator while modern spill functions
    /// use `Matrix` to retain both shape and scalar types.
    Array(Vec<f64>),
    /// A rectangular dynamic-array result, in row-major order.
    Matrix(Vec<Vec<FormulaResult>>),
    Blank,
}

impl FormulaResult {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Number(_))
    }
    pub fn is_string(&self) -> bool {
        matches!(self, Self::Str(_))
    }
    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Bool(_))
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_) | Self::Matrix(_))
    }
    pub fn is_blank(&self) -> bool {
        matches!(self, Self::Blank)
    }

    /// Coerce to f64 (Excel coercion rules: blank→0, bool→0/1, numeric string→parsed, else 0).
    pub fn as_number(&self) -> f64 {
        match self {
            Self::Number(v) => *v,
            Self::Bool(v) => {
                if *v {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Str(s) => s.parse::<f64>().unwrap_or(0.0),
            Self::Blank => 0.0,
            Self::Error(_) => 0.0,
            Self::Array(a) => a.first().copied().unwrap_or(0.0),
            Self::Matrix(rows) => rows
                .first()
                .and_then(|row| row.first())
                .map(FormulaResult::as_number)
                .unwrap_or(0.0),
        }
    }

    /// Coerce to String (for concatenation/display).
    pub fn as_string(&self) -> String {
        match self {
            Self::Number(v) => format_number(*v),
            Self::Str(s) => s.clone(),
            Self::Bool(v) => {
                if *v {
                    "TRUE".to_string()
                } else {
                    "FALSE".to_string()
                }
            }
            Self::Error(e) => e.clone(),
            Self::Blank => String::new(),
            Self::Array(a) => a.first().map(|v| format_number(*v)).unwrap_or_default(),
            Self::Matrix(rows) => rows
                .first()
                .and_then(|row| row.first())
                .map(FormulaResult::as_string)
                .unwrap_or_default(),
        }
    }

    /// Convert to a cell value text for writing back into the spreadsheet.
    pub fn to_cell_value_text(&self) -> String {
        match self {
            Self::Number(v) => {
                let v = *v;
                if v.is_nan() || v.is_infinite() {
                    "#NUM!".to_string()
                } else if v == 0.0 {
                    "0".to_string()
                } else {
                    // Round to 15 significant digits
                    let digits = 15 - (v.abs().log10().floor() as i32) - 1;
                    if (0..=15).contains(&digits) {
                        format!("{:.1$}", v, digits as usize)
                    } else {
                        format_number(v)
                    }
                }
            }
            Self::Bool(v) => {
                if *v {
                    "1".to_string()
                } else {
                    "0".to_string()
                }
            }
            Self::Str(s) => s.clone(),
            Self::Error(e) => e.clone(),
            Self::Blank => "0".to_string(),
            Self::Array(a) => a.first().map(|v| format_number(*v)).unwrap_or_default(),
            Self::Matrix(rows) => rows
                .first()
                .and_then(|row| row.first())
                .map(FormulaResult::to_cell_value_text)
                .unwrap_or_default(),
        }
    }

    /// Return a matrix preserving scalar types. Scalar values become 1×1;
    /// legacy numeric arrays become an n×1 column for compatibility.
    pub fn as_matrix(&self) -> Vec<Vec<FormulaResult>> {
        match self {
            Self::Matrix(rows) => rows.clone(),
            Self::Array(values) => values
                .iter()
                .copied()
                .map(|value| vec![Self::Number(value)])
                .collect(),
            value => vec![vec![value.clone()]],
        }
    }

    pub fn matrix(rows: Vec<Vec<FormulaResult>>) -> Self {
        Self::Matrix(rows)
    }
}

/// Format a number for display (matches Excel's invariant culture format).
pub fn format_number(v: f64) -> String {
    if v == v.floor() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    }
}

// ─── Token types for the formula tokenizer ──────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenType {
    Number,
    String,
    CellRef,
    Range,
    Op,
    LParen,
    RParen,
    Comma,
    Func,
    Bool,
    Compare,
    SheetCellRef,
    SheetRange,
    ArrayLit,
    Error,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tt: TokenType,
    pub value: String,
}

impl Token {
    pub fn new(tt: TokenType, value: impl Into<String>) -> Self {
        Self {
            tt,
            value: value.into(),
        }
    }
}

// ─── Reference argument for OFFSET ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RefArg {
    pub sheet: Option<String>,
    pub col: usize, // 1-based
    pub row: usize, // 1-based
    pub width: usize,
    pub height: usize,
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Convert column letters to 1-based index: A=1, B=2, Z=26, AA=27.
pub fn col_to_index(col: &str) -> usize {
    let mut r: usize = 0;
    for c in col.chars() {
        r = r * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1);
    }
    r
}

/// Convert 1-based column index to letters: 1=A, 26=Z, 27=AA.
pub fn index_to_col(i: usize) -> String {
    let mut r = String::new();
    let mut n = i;
    while n > 0 {
        n -= 1;
        r.push((b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    r.chars().rev().collect()
}

/// Parse a cell reference string like "A1" into (col_letters, row_number).
pub fn parse_ref(r: &str) -> Option<(String, usize)> {
    let col_part: String = r.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let row_part: String = r.chars().filter(|c| c.is_ascii_digit()).collect();
    if col_part.is_empty() || row_part.is_empty() {
        return None;
    }
    let row: usize = row_part.parse().ok()?;
    Some((col_part.to_uppercase(), row))
}

/// Check if a string looks like a cell reference (e.g. "A1", "AA100").
pub fn is_cell_ref(s: &str) -> bool {
    let s = s.replace('$', "");
    parse_ref(&s).is_some()
}

/// Strip $ signs from a reference string.
pub fn strip_dollar(s: &str) -> String {
    s.replace('$', "")
}
