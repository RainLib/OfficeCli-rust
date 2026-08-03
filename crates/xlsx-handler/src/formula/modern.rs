//! OOXML serialization rules for post-2016 Excel functions.
//!
//! Excel displays functions such as `SEQUENCE` without a prefix, but stores
//! them as `_xlfn.SEQUENCE` (and `FILTER` as `_xlfn._xlws.FILTER`) in OOXML.
//! Keeping this conversion at the package boundary makes CLI readback stable
//! while allowing the generated workbook to open correctly in Excel.

const XLFN_FUNCTIONS: &[&str] = &[
    "SEQUENCE",
    "SORT",
    "SORTBY",
    "UNIQUE",
    "XLOOKUP",
    "XMATCH",
    "LET",
    "LAMBDA",
    "REDUCE",
    "ISOMITTED",
    "MAP",
    "BYROW",
    "BYCOL",
    "SCAN",
    "MAKEARRAY",
    "IFS",
    "SWITCH",
    "MAXIFS",
    "MINIFS",
    "CONCAT",
    "TEXTJOIN",
    "STOCKHISTORY",
    "TEXTBEFORE",
    "TEXTAFTER",
    "TEXTSPLIT",
    "REGEXTEST",
    "REGEXEXTRACT",
    "REGEXREPLACE",
    "ISFORMULA",
    "SHEET",
    "SHEETS",
    "TAKE",
    "DROP",
    "CHOOSECOLS",
    "CHOOSEROWS",
    "ARRAYTOTEXT",
    "VALUETOTEXT",
    "TOCOL",
    "TOROW",
    "WRAPCOLS",
    "WRAPROWS",
    "EXPAND",
    "HSTACK",
    "VSTACK",
    "ANCHORARRAY",
    "RANDARRAY",
    "IMCOSH",
    "IMCOT",
    "IMCSC",
    "IMCSCH",
    "IMSEC",
    "IMSECH",
    "IMSINH",
    "IMTAN",
    "BITAND",
    "BITOR",
    "BITXOR",
    "BITLSHIFT",
    "BITRSHIFT",
    "PDURATION",
    "RRI",
    "NORM.DIST",
    "NORM.S.DIST",
    "NORM.INV",
    "NORM.S.INV",
    "GAMMA",
    "GAMMALN.PRECISE",
    "GAMMA.DIST",
    "GAMMA.INV",
    "CHISQ.DIST",
    "CHISQ.DIST.RT",
    "CHISQ.INV",
    "CHISQ.INV.RT",
    "POISSON.DIST",
    "EXPON.DIST",
    "CONFIDENCE.NORM",
    "ERF.PRECISE",
    "ERFC.PRECISE",
    "GAUSS",
    "PHI",
    "BETA.DIST",
    "BETA.INV",
    "T.DIST",
    "T.DIST.2T",
    "T.DIST.RT",
    "T.INV",
    "T.INV.2T",
    "F.DIST",
    "F.DIST.RT",
    "F.INV",
    "F.INV.RT",
    "BINOM.DIST",
    "BINOM.INV",
    "NEGBINOM.DIST",
    "WEIBULL.DIST",
    "LOGNORM.DIST",
    "LOGNORM.INV",
    "HYPGEOM.DIST",
    "T.TEST",
    "CHISQ.TEST",
    "F.TEST",
    "Z.TEST",
    "SKEW.P",
    "COVARIANCE.P",
    "COVARIANCE.S",
    "QUARTILE.INC",
    "QUARTILE.EXC",
    "PERCENTILE.EXC",
    "FORECAST.LINEAR",
    "PERMUTATIONA",
];

const XLWS_FUNCTIONS: &[&str] = &["FILTER"];

const DYNAMIC_ARRAY_FUNCTIONS: &[&str] = &[
    "FILTER",
    "SORT",
    "SORTBY",
    "UNIQUE",
    "SEQUENCE",
    "RANDARRAY",
    "XLOOKUP",
    "XMATCH",
    "LET",
    "LAMBDA",
    "MAP",
    "BYROW",
    "BYCOL",
    "SCAN",
    "MAKEARRAY",
    "TAKE",
    "DROP",
    "CHOOSECOLS",
    "CHOOSEROWS",
    "TOCOL",
    "TOROW",
    "WRAPCOLS",
    "WRAPROWS",
    "EXPAND",
    "HSTACK",
    "VSTACK",
    "TRANSPOSE",
    "LINEST",
    "LOGEST",
    "TREND",
    "GROWTH",
    "TEXTSPLIT",
    "ANCHORARRAY",
];

/// Whether Excel requires the formula cell to carry array/spill metadata.
pub fn is_dynamic_array_formula(formula: &str) -> bool {
    // Writers call this after qualification, while callers of the public API
    // may pass a user-facing formula.  Normalize first so both forms detect
    // the same dynamic function.
    let formula = unqualify_for_readback(formula);
    let chars: Vec<char> = formula.trim_start_matches('=').chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let mut ignored = String::new();
            copy_string(&chars, &mut i, &mut ignored);
            continue;
        }
        if is_ident_start(chars[i]) && (i == 0 || !is_ident_previous(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_formula_ident_char(chars[i]) {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            let mut after = i;
            while after < chars.len() && chars[after] == ' ' {
                after += 1;
            }
            if after < chars.len()
                && chars[after] == '('
                && contains_ignore_case(DYNAMIC_ARRAY_FUNCTIONS, &name)
            {
                return true;
            }
            continue;
        }
        i += 1;
    }
    false
}

/// Convert a user-facing formula to Excel's OOXML representation.
pub fn qualify_for_ooxml(formula: &str) -> Result<String, String> {
    let formula = formula.trim_start_matches('=');
    if formula.trim().is_empty() {
        return Err("Formula cannot be empty or whitespace".to_string());
    }
    if let Some(element) = invalid_array_element(formula) {
        return Err(format!(
            "Invalid array constant: '{element}'. Inline arrays may contain only literal values"
        ));
    }

    let lambda_params = collect_lambda_params(formula);
    let mut output = String::with_capacity(formula.len() + 16);
    let chars: Vec<char> = formula.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            copy_string(&chars, &mut i, &mut output);
            continue;
        }
        if is_ident_start(chars[i]) && (i == 0 || !is_ident_previous(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_formula_ident_char(chars[i]) {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            if lambda_params
                .iter()
                .any(|param| param.eq_ignore_ascii_case(&name))
            {
                output.push_str("_xlpm.");
                output.push_str(&name);
                continue;
            }
            let mut after = i;
            while after < chars.len() && chars[after] == ' ' {
                after += 1;
            }
            if after < chars.len() && chars[after] == '(' {
                if contains_ignore_case(XLWS_FUNCTIONS, &name) {
                    output.push_str("_xlfn._xlws.");
                } else if contains_ignore_case(XLFN_FUNCTIONS, &name) {
                    output.push_str("_xlfn.");
                }
            }
            output.push_str(&name);
            continue;
        }
        output.push(chars[i]);
        i += 1;
    }
    Ok(output)
}

/// Convert OOXML-only namespace prefixes back to the CLI's canonical syntax.
pub fn unqualify_for_readback(formula: &str) -> String {
    formula
        .replace("_xlfn._xlws.", "")
        .replace("_xlfn.", "")
        .replace("_xlpm.", "")
}

fn contains_ignore_case(values: &[&str], value: &str) -> bool {
    values
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(value))
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_previous(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'
}

fn is_formula_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'
}

fn is_parameter_ident(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn copy_string(chars: &[char], index: &mut usize, output: &mut String) {
    output.push(chars[*index]);
    *index += 1;
    while *index < chars.len() {
        output.push(chars[*index]);
        if chars[*index] == '"' {
            if *index + 1 < chars.len() && chars[*index + 1] == '"' {
                output.push('"');
                *index += 2;
                continue;
            }
            *index += 1;
            break;
        }
        *index += 1;
    }
}

fn collect_lambda_params(formula: &str) -> Vec<String> {
    let chars: Vec<char> = formula.chars().collect();
    let mut params = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let mut ignored = String::new();
            copy_string(&chars, &mut i, &mut ignored);
            continue;
        }
        if is_ident_start(chars[i]) && (i == 0 || !is_ident_previous(chars[i - 1])) {
            let start = i;
            while i < chars.len() && is_parameter_ident(chars[i]) {
                i += 1;
            }
            let name: String = chars[start..i].iter().collect();
            let mut open = i;
            while open < chars.len() && chars[open] == ' ' {
                open += 1;
            }
            let is_let = name.eq_ignore_ascii_case("LET");
            let is_lambda = name.eq_ignore_ascii_case("LAMBDA");
            if (is_let || is_lambda) && open < chars.len() && chars[open] == '(' {
                let args = top_level_args(&chars, open);
                for (index, arg) in args.iter().enumerate().take(args.len().saturating_sub(1)) {
                    if (is_lambda || index % 2 == 0) && is_plain_parameter(arg) {
                        params.push(arg.trim().to_string());
                    }
                }
            }
            continue;
        }
        i += 1;
    }
    params
}

fn top_level_args(chars: &[char], open: usize) -> Vec<String> {
    let mut args = Vec::new();
    let mut start = open + 1;
    let mut depth = 0;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                let mut ignored = String::new();
                copy_string(chars, &mut i, &mut ignored);
                continue;
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    args.push(chars[start..i].iter().collect());
                    break;
                }
            }
            ',' if depth == 1 => {
                args.push(chars[start..i].iter().collect());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    args
}

fn is_plain_parameter(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && is_ident_start(value.chars().next().unwrap_or_default())
        && value.chars().all(is_parameter_ident)
}

fn invalid_array_element(formula: &str) -> Option<String> {
    let chars: Vec<char> = formula.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let mut ignored = String::new();
            copy_string(&chars, &mut i, &mut ignored);
            continue;
        }
        if chars[i] != '{' {
            i += 1;
            continue;
        }
        let mut element = String::new();
        let mut parens = 0;
        i += 1;
        while i < chars.len() && chars[i] != '}' {
            match chars[i] {
                '"' => copy_string(&chars, &mut i, &mut element),
                '{' => return Some("{ }".to_string()),
                '(' => {
                    parens += 1;
                    element.push('(');
                    i += 1;
                }
                ')' => {
                    parens -= 1;
                    element.push(')');
                    i += 1;
                }
                ',' | ';' if parens == 0 => {
                    if !is_array_literal(&element) {
                        return Some(element.trim().to_string());
                    }
                    element.clear();
                    i += 1;
                }
                ch => {
                    element.push(ch);
                    i += 1;
                }
            }
        }
        if i == chars.len() || !is_array_literal(&element) {
            return Some(element.trim().to_string());
        }
        i += 1;
    }
    None
}

fn is_array_literal(value: &str) -> bool {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return true;
    }
    value.eq_ignore_ascii_case("TRUE")
        || value.eq_ignore_ascii_case("FALSE")
        || value.starts_with('#')
        || value.parse::<f64>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::{is_dynamic_array_formula, qualify_for_ooxml, unqualify_for_readback};

    #[test]
    fn qualifies_dynamic_functions_without_touching_literals_or_existing_prefixes() {
        assert_eq!(
            qualify_for_ooxml("SEQUENCE(3)&\" SORT(2) \"+_xlfn.UNIQUE(A1:A3)").unwrap(),
            "_xlfn.SEQUENCE(3)&\" SORT(2) \"+_xlfn.UNIQUE(A1:A3)"
        );
        assert_eq!(
            qualify_for_ooxml("FILTER(A1:A3,B1:B3=1)").unwrap(),
            "_xlfn._xlws.FILTER(A1:A3,B1:B3=1)"
        );
    }

    #[test]
    fn qualifies_lambda_parameters_and_restores_readback() {
        let qualified = qualify_for_ooxml("LET(x,1,x+SEQUENCE(2))").unwrap();
        assert_eq!(qualified, "_xlfn.LET(_xlpm.x,1,_xlpm.x+_xlfn.SEQUENCE(2))");
        assert_eq!(unqualify_for_readback(&qualified), "LET(x,1,x+SEQUENCE(2))");
    }

    #[test]
    fn rejects_non_literal_inline_array_values() {
        assert!(qualify_for_ooxml("SUM({A1,2})").is_err());
        assert!(qualify_for_ooxml("SUM({1,TRUE,\"x\"})").is_ok());
    }

    #[test]
    fn recognizes_dynamic_calls_but_not_quoted_text() {
        assert!(is_dynamic_array_formula("SORT(A1:A3)"));
        assert!(is_dynamic_array_formula("_xlfn.SEQUENCE(2)"));
        assert!(!is_dynamic_array_formula("\"SORT(A1:A3)\""));
    }
}
