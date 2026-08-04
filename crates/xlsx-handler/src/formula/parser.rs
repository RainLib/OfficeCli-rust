//! Recursive descent formula parser.
//!
//! Precedence (low→high): comparison → concat → add/sub → mul/div → power → unary → postfix(%) → atom.

use super::tokenizer;
use super::types::*;

/// Parse a formula string and evaluate it against a cell resolver.
pub struct FormulaParser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    resolver: &'a dyn CellResolver,
    _same_sheet_depth: usize,
    parse_depth: usize,
}

/// Trait for resolving cell values during formula evaluation.
/// Implemented by the caller to provide access to the workbook's cell data.
pub trait CellResolver {
    /// Resolve a same-sheet cell reference (e.g. "A1") to a FormulaResult.
    fn resolve_cell(&self, cell_ref: &str) -> FormulaResult;

    /// Resolve a cross-sheet cell reference (e.g. "Sheet1!A1") to a FormulaResult.
    fn resolve_sheet_cell(&self, sheet_cell_ref: &str) -> FormulaResult;

    /// Expand a range reference (e.g. "A1:B3" or "Sheet1!A1:B3") to a Vec of (cell_ref, FormulaResult).
    fn expand_range(&self, range_expr: &str) -> Vec<(String, FormulaResult)>;
}

impl<'a> FormulaParser<'a> {
    pub fn new(formula: &str, resolver: &'a dyn CellResolver) -> Result<Self, String> {
        let tokens = tokenizer::tokenize(formula)?;
        Ok(Self {
            tokens,
            pos: 0,
            resolver,
            _same_sheet_depth: 0,
            parse_depth: 0,
        })
    }

    /// Parse and evaluate the formula.
    pub fn evaluate(&mut self) -> Option<FormulaResult> {
        let result = self.evaluate_spill();
        // Top-level arrays collapse to their spill anchor value when a scalar
        // consumer evaluates them.  The full matrix remains available to
        // modern functions while parsing their arguments.
        match result {
            Some(FormulaResult::Array(ref a)) if !a.is_empty() => Some(FormulaResult::Number(a[0])),
            Some(FormulaResult::Matrix(ref rows)) => rows
                .first()
                .and_then(|row| row.first())
                .cloned()
                .or(Some(FormulaResult::Blank)),
            other => other,
        }
    }

    /// Evaluate without collapsing a dynamic array to its anchor. Worksheet
    /// writers use this to persist a complete cached spill range.
    pub fn evaluate_spill(&mut self) -> Option<FormulaResult> {
        let result = self.parse_expression();
        if self.pos != self.tokens.len() {
            return None; // unconsumed tokens
        }
        result
    }

    // ─── Precedence levels ────────────────────────────────────────────

    fn parse_expression(&mut self) -> Option<FormulaResult> {
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Option<FormulaResult> {
        let left = self.parse_concat()?;
        if self.pos < self.tokens.len() && self.tokens[self.pos].tt == TokenType::Compare {
            let op = self.tokens[self.pos].value.clone();
            self.pos += 1;
            let right = self.parse_concat()?;
            if left.is_error() {
                return Some(left);
            }
            if right.is_error() {
                return Some(right);
            }
            let cmp = compare_values(&left, &right);
            let result = match op.as_str() {
                "=" => cmp == 0,
                "<>" => cmp != 0,
                "<" => cmp < 0,
                ">" => cmp > 0,
                "<=" => cmp <= 0,
                ">=" => cmp >= 0,
                _ => return None,
            };
            return Some(FormulaResult::Bool(result));
        }
        Some(left)
    }

    fn parse_concat(&mut self) -> Option<FormulaResult> {
        self.parse_depth += 1;
        if self.parse_depth > 200 {
            self.parse_depth -= 1;
            return Some(FormulaResult::Error("#NUM!".to_string()));
        }
        let left = self.parse_add_sub()?;
        let result = if self.pos < self.tokens.len()
            && self.tokens[self.pos].tt == TokenType::Op
            && self.tokens[self.pos].value == "&"
        {
            let mut result = left;
            while self.pos < self.tokens.len()
                && self.tokens[self.pos].tt == TokenType::Op
                && self.tokens[self.pos].value == "&"
            {
                self.pos += 1;
                let right = self.parse_add_sub()?;
                if result.is_error() {
                    return Some(result);
                }
                if right.is_error() {
                    return Some(right);
                }
                result = FormulaResult::Str(format!("{}{}", result.as_string(), right.as_string()));
            }
            result
        } else {
            left
        };
        self.parse_depth -= 1;
        Some(result)
    }

    fn parse_add_sub(&mut self) -> Option<FormulaResult> {
        let left = self.parse_mul_div()?;
        let mut result = left;
        while self.pos < self.tokens.len()
            && self.tokens[self.pos].tt == TokenType::Op
            && (self.tokens[self.pos].value == "+" || self.tokens[self.pos].value == "-")
        {
            let op = self.tokens[self.pos].value.clone();
            self.pos += 1;
            let right = self.parse_mul_div()?;
            if result.is_error() {
                return Some(result);
            }
            if right.is_error() {
                return Some(right);
            }
            let lv = result.as_number();
            let rv = right.as_number();
            result = FormulaResult::Number(if op == "+" { lv + rv } else { lv - rv });
        }
        Some(result)
    }

    fn parse_mul_div(&mut self) -> Option<FormulaResult> {
        let left = self.parse_power()?;
        let mut result = left;
        while self.pos < self.tokens.len()
            && self.tokens[self.pos].tt == TokenType::Op
            && (self.tokens[self.pos].value == "*" || self.tokens[self.pos].value == "/")
        {
            let op = self.tokens[self.pos].value.clone();
            self.pos += 1;
            let right = self.parse_power()?;
            if result.is_error() {
                return Some(result);
            }
            if right.is_error() {
                return Some(right);
            }
            let lv = result.as_number();
            let rv = right.as_number();
            if op == "/" {
                if rv == 0.0 {
                    return Some(FormulaResult::Error("#DIV/0!".to_string()));
                }
                result = FormulaResult::Number(lv / rv);
            } else {
                result = FormulaResult::Number(lv * rv);
            }
        }
        Some(result)
    }

    fn parse_power(&mut self) -> Option<FormulaResult> {
        let base = self.parse_unary()?;
        let mut result = base;
        while self.pos < self.tokens.len()
            && self.tokens[self.pos].tt == TokenType::Op
            && self.tokens[self.pos].value == "^"
        {
            self.pos += 1;
            let exp = self.parse_unary()?;
            if result.is_error() {
                return Some(result);
            }
            if exp.is_error() {
                return Some(exp);
            }
            result = FormulaResult::Number(result.as_number().powf(exp.as_number()));
        }
        Some(result)
    }

    fn parse_unary(&mut self) -> Option<FormulaResult> {
        if self.pos < self.tokens.len() && self.tokens[self.pos].tt == TokenType::Op {
            if self.tokens[self.pos].value == "-" {
                self.pos += 1;
                let v = self.parse_unary()?;
                return Some(match v {
                    FormulaResult::Number(n) => FormulaResult::Number(-n),
                    FormulaResult::Error(e) => FormulaResult::Error(e),
                    other => FormulaResult::Number(-other.as_number()),
                });
            }
            if self.tokens[self.pos].value == "+" {
                self.pos += 1;
                return self.parse_unary();
            }
        }
        self.parse_postfix()
    }

    fn parse_postfix(&mut self) -> Option<FormulaResult> {
        let v = self.parse_atom()?;
        let mut result = v;
        while self.pos < self.tokens.len()
            && self.tokens[self.pos].tt == TokenType::Op
            && self.tokens[self.pos].value == "%"
        {
            self.pos += 1;
            result = FormulaResult::Number(result.as_number() / 100.0);
        }
        Some(result)
    }

    fn parse_atom(&mut self) -> Option<FormulaResult> {
        if self.pos >= self.tokens.len() {
            return None;
        }
        let tok = &self.tokens[self.pos].clone();
        match tok.tt {
            TokenType::Number => {
                self.pos += 1;
                let v: f64 = tok.value.parse().ok()?;
                Some(FormulaResult::Number(v))
            }
            TokenType::String => {
                self.pos += 1;
                Some(FormulaResult::Str(tok.value.clone()))
            }
            TokenType::Bool => {
                self.pos += 1;
                Some(FormulaResult::Bool(tok.value == "TRUE"))
            }
            TokenType::CellRef => {
                self.pos += 1;
                Some(self.resolver.resolve_cell(&tok.value))
            }
            TokenType::SheetCellRef => {
                self.pos += 1;
                Some(self.resolver.resolve_sheet_cell(&tok.value))
            }
            TokenType::Range => {
                self.pos += 1;
                let cells = self.resolver.expand_range(&tok.value);
                Some(range_cells_to_matrix(cells))
            }
            TokenType::SheetRange => {
                self.pos += 1;
                let cells = self.resolver.expand_range(&tok.value);
                Some(range_cells_to_matrix(cells))
            }
            TokenType::ArrayLit => {
                self.pos += 1;
                Some(parse_array_constant(&tok.value))
            }
            TokenType::Error => {
                self.pos += 1;
                Some(FormulaResult::Error(tok.value.clone()))
            }
            TokenType::LParen => {
                self.pos += 1;
                let inner = self.parse_expression();
                if self.pos < self.tokens.len() && self.tokens[self.pos].tt == TokenType::RParen {
                    self.pos += 1;
                }
                inner
            }
            TokenType::Func => self.parse_function(),
            _ => None,
        }
    }

    fn parse_function(&mut self) -> Option<FormulaResult> {
        let name = self.tokens[self.pos].value.clone();
        self.pos += 1;
        if self.pos >= self.tokens.len() || self.tokens[self.pos].tt != TokenType::LParen {
            return None;
        }
        self.pos += 1; // skip (

        let mut args: Vec<FormulaResult> = Vec::new();
        if self.pos < self.tokens.len() && self.tokens[self.pos].tt != TokenType::RParen {
            loop {
                // Empty arg = 0
                if self.pos < self.tokens.len()
                    && (self.tokens[self.pos].tt == TokenType::Comma
                        || self.tokens[self.pos].tt == TokenType::RParen)
                {
                    args.push(FormulaResult::Number(0.0));
                } else {
                    let expr = self.parse_expression()?;
                    args.push(expr);
                }
                if self.pos >= self.tokens.len() || self.tokens[self.pos].tt != TokenType::Comma {
                    break;
                }
                self.pos += 1; // skip comma
            }
        }
        if self.pos < self.tokens.len() && self.tokens[self.pos].tt == TokenType::RParen {
            self.pos += 1;
        }

        // Dispatch to function implementation
        super::functions::eval_function(&name, &args, self.resolver)
    }
}

// ─── Array constant parsing ──────────────────────────────────────────────

fn parse_array_constant(body: &str) -> FormulaResult {
    let rows: Vec<&str> = body.split(';').collect();
    let max_columns = rows
        .iter()
        .map(|row| row.split(',').count())
        .max()
        .unwrap_or(1);
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        let mut output_row = Vec::with_capacity(max_columns);
        for cell in row.split(',') {
            let s = cell.trim();
            if s.is_empty() {
                output_row.push(FormulaResult::Blank);
            } else if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                output_row.push(FormulaResult::Str(s[1..s.len() - 1].replace("\"\"", "\"")));
            } else if s.eq_ignore_ascii_case("TRUE") {
                output_row.push(FormulaResult::Bool(true));
            } else if s.eq_ignore_ascii_case("FALSE") {
                output_row.push(FormulaResult::Bool(false));
            } else if let Ok(n) = s.parse::<f64>() {
                output_row.push(FormulaResult::Number(n));
            } else {
                output_row.push(FormulaResult::Error("#VALUE!".to_string()));
            }
        }
        output_row.resize(max_columns, FormulaResult::Blank);
        values.push(output_row);
    }
    FormulaResult::Matrix(values)
}

fn range_cells_to_matrix(cells: Vec<(String, FormulaResult)>) -> FormulaResult {
    if cells.is_empty() {
        return FormulaResult::Matrix(Vec::new());
    }

    let refs: Vec<_> = cells
        .iter()
        .filter_map(|(cell_ref, value)| {
            parse_ref(cell_ref).map(|(column, row)| (row, col_to_index(&column), value.clone()))
        })
        .collect();
    let (Some(min_row), Some(max_row), Some(min_col), Some(max_col)) = (
        refs.iter().map(|(row, _, _)| *row).min(),
        refs.iter().map(|(row, _, _)| *row).max(),
        refs.iter().map(|(_, col, _)| *col).min(),
        refs.iter().map(|(_, col, _)| *col).max(),
    ) else {
        return FormulaResult::Matrix(cells.into_iter().map(|(_, value)| vec![value]).collect());
    };
    let mut rows = vec![vec![FormulaResult::Blank; max_col - min_col + 1]; max_row - min_row + 1];
    for (row, col, value) in refs {
        rows[row - min_row][col - min_col] = value;
    }
    FormulaResult::Matrix(rows)
}

// ─── Comparison helper ──────────────────────────────────────────────────

pub fn compare_values(a: &FormulaResult, b: &FormulaResult) -> i32 {
    // Try numeric comparison first
    let an = a.as_number();
    let bn = b.as_number();
    if an < bn {
        return -1;
    }
    if an > bn {
        return 1;
    }
    // Fall back to string comparison
    let as_str = a.as_string();
    let bs_str = b.as_string();
    as_str.cmp(&bs_str) as i32
}
