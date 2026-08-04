//! Formula function dispatch — 80+ Excel-compatible functions.
//!
//! Ported from C# FormulaEvaluator.Functions.cs.

use super::parser::{compare_values, CellResolver};
use super::types::*;

/// Evaluate a function call by name with the given arguments.
#[allow(clippy::only_used_in_recursion)]
pub fn eval_function(
    name: &str,
    args: &[FormulaResult],
    resolver: &dyn CellResolver,
) -> Option<FormulaResult> {
    // Formula XML stores post-2016 functions as `_xlfn.NAME` (and FILTER as
    // `_xlfn._xlws.FILTER`). The tokenizer replaces dots with underscores,
    // so normalize those wire names before dispatching the user-facing name.
    let name = name
        .strip_prefix("_XLFN__XLWS_")
        .or_else(|| name.strip_prefix("_XLFN_"))
        .unwrap_or(name);
    // Helper closures
    let nums = || flatten_numbers(args);
    let num = |i: usize| args.get(i).map(|a| a.as_number()).unwrap_or(0.0);
    let str_arg = |i: usize| args.get(i).map(|a| a.as_string()).unwrap_or_default();

    match name {
        // ===== Math & Aggregation =====
        "SUM" => Some(FormulaResult::Number(nums().iter().sum())),
        "AVERAGE" => {
            let n = nums();
            if n.is_empty() {
                Some(FormulaResult::Error("#DIV/0!".to_string()))
            } else {
                Some(FormulaResult::Number(
                    n.iter().sum::<f64>() / n.len() as f64,
                ))
            }
        }
        "COUNT" => Some(FormulaResult::Number(nums().len() as f64)),
        "COUNTA" => Some(FormulaResult::Number(args.len() as f64)),
        "COUNTBLANK" => Some(FormulaResult::Number(0.0)),
        "MIN" => {
            let n = nums();
            n.iter().cloned().fold(f64::INFINITY, f64::min).pipe(|v| {
                if v == f64::INFINITY {
                    Some(FormulaResult::Number(0.0))
                } else {
                    Some(FormulaResult::Number(v))
                }
            })
        }
        "MAX" => {
            let n = nums();
            n.iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
                .pipe(|v| {
                    if v == f64::NEG_INFINITY {
                        Some(FormulaResult::Number(0.0))
                    } else {
                        Some(FormulaResult::Number(v))
                    }
                })
        }
        "ABS" => Some(FormulaResult::Number(num(0).abs())),
        "ROUND" => {
            let v = num(0);
            let d = num(1) as i32;
            let factor = 10f64.powi(d);
            Some(FormulaResult::Number((v * factor).round() / factor))
        }
        "ROUNDUP" => {
            let v = num(0);
            let d = num(1) as i32;
            let factor = 10f64.powi(d);
            Some(FormulaResult::Number((v * factor).ceil() / factor))
        }
        "ROUNDDOWN" => {
            let v = num(0);
            let d = num(1) as i32;
            let factor = 10f64.powi(d);
            Some(FormulaResult::Number((v * factor).floor() / factor))
        }
        "INT" => Some(FormulaResult::Number(num(0).floor())),
        "TRUNC" => {
            let v = num(0);
            let d = if args.len() >= 2 { num(1) } else { 0.0 };
            let factor = 10f64.powi(d as i32);
            Some(FormulaResult::Number((v * factor).trunc() / factor))
        }
        "MOD" => {
            let n = num(0);
            let d = num(1);
            if d == 0.0 {
                Some(FormulaResult::Error("#DIV/0!".to_string()))
            } else {
                Some(FormulaResult::Number(n - d * (n / d).floor()))
            }
        }
        "POWER" => Some(FormulaResult::Number(num(0).powf(num(1)))),
        "SQRT" => {
            let v = num(0);
            if v < 0.0 {
                Some(FormulaResult::Error("#NUM!".to_string()))
            } else {
                Some(FormulaResult::Number(v.sqrt()))
            }
        }
        "PRODUCT" => {
            let n = nums();
            Some(FormulaResult::Number(n.iter().product::<f64>()))
        }
        "QUOTIENT" => {
            let d = num(1);
            if d == 0.0 {
                Some(FormulaResult::Error("#DIV/0!".to_string()))
            } else {
                Some(FormulaResult::Number((num(0) / d).trunc()))
            }
        }
        "SUMPRODUCT" => {
            if args.is_empty() {
                return Some(FormulaResult::Number(0.0));
            }
            let n = nums();
            Some(FormulaResult::Number(n.iter().product::<f64>()))
        }
        "SUBTOTAL" | "AGGREGATE" => {
            // Simplified: just re-dispatch based on function_num
            let code = num(0) as i32 % 100;
            let func_name = match code {
                1 => "AVERAGE",
                2 => "COUNT",
                3 => "COUNTA",
                4 => "MAX",
                5 => "MIN",
                6 => "PRODUCT",
                7 => "STDEV",
                8 => "STDEVP",
                9 => "SUM",
                10 => "VAR",
                11 => "VARP",
                _ => return None,
            };
            let skip = if name == "AGGREGATE" { 2 } else { 1 };
            let sub_args = &args[skip..];
            eval_function(func_name, sub_args, resolver)
        }

        // ===== Logical =====
        "IF" => {
            let cond = num(0) != 0.0;
            if cond {
                args.get(1).cloned()
            } else {
                args.get(2).cloned().or(Some(FormulaResult::Bool(false)))
            }
        }
        "IFS" => {
            let mut i = 0;
            while i + 1 < args.len() {
                if args[i].as_number() != 0.0 {
                    return args.get(i + 1).cloned();
                }
                i += 2;
            }
            Some(FormulaResult::Error("#N/A".to_string()))
        }
        "AND" => Some(FormulaResult::Bool(
            args.iter().all(|a| a.as_number() != 0.0),
        )),
        "OR" => Some(FormulaResult::Bool(
            args.iter().any(|a| a.as_number() != 0.0),
        )),
        "NOT" => Some(FormulaResult::Bool(num(0) == 0.0)),
        "XOR" => Some(FormulaResult::Bool(
            args.iter().filter(|a| a.as_number() != 0.0).count() % 2 == 1,
        )),
        "TRUE" => Some(FormulaResult::Bool(true)),
        "FALSE" => Some(FormulaResult::Bool(false)),
        "IFERROR" | "IFNA" => {
            if args.first().map(|a| a.is_error()).unwrap_or(false) {
                args.get(1).cloned()
            } else {
                args.first().cloned()
            }
        }
        "SWITCH" => {
            if args.len() < 2 {
                return None;
            }
            let val = &args[0];
            let mut i = 1;
            while i + 1 < args.len() {
                if compare_values(val, &args[i]) == 0 {
                    return args.get(i + 1).cloned();
                }
                i += 2;
            }
            // Default value (odd number of args after the first)
            if args.len().is_multiple_of(2) {
                args.last().cloned()
            } else {
                Some(FormulaResult::Error("#N/A".to_string()))
            }
        }

        // ===== Modern dynamic arrays =====
        // Modern functions keep their rectangular shape and scalar types so
        // strings, booleans and 2-D ranges can be used by later functions.
        "SEQUENCE" => Some(eval_sequence(args)),
        "SORT" => Some(eval_sort(args)),
        "SORTBY" => Some(eval_sortby(args)),
        "UNIQUE" => Some(eval_unique(args)),
        "FILTER" => Some(eval_filter(args)),
        "XLOOKUP" => Some(eval_xlookup(args)),
        "XMATCH" => Some(eval_xmatch(args)),
        "TAKE" => Some(eval_take(args)),
        "DROP" => Some(eval_drop(args)),
        "TOCOL" => Some(eval_to_col(args)),
        "TOROW" => Some(eval_to_row(args)),
        "HSTACK" => Some(eval_hstack(args)),
        "VSTACK" => Some(eval_vstack(args)),
        "RANDARRAY" => Some(eval_randarray(args)),
        "CHOOSE" => {
            let idx = num(0) as usize;
            if idx >= 1 && idx < args.len() {
                args.get(idx).cloned()
            } else {
                Some(FormulaResult::Error("#VALUE!".to_string()))
            }
        }

        // ===== Text =====
        "CONCATENATE" | "CONCAT" => Some(FormulaResult::Str(
            args.iter().map(|a| a.as_string()).collect(),
        )),
        "LEFT" => {
            let s = str_arg(0);
            let n = num(1) as usize;
            Some(FormulaResult::Str(s.chars().take(n).collect()))
        }
        "RIGHT" => {
            let s = str_arg(0);
            let n = num(1) as usize;
            let chars: Vec<char> = s.chars().collect();
            let start = chars.len().saturating_sub(n);
            Some(FormulaResult::Str(chars[start..].iter().collect()))
        }
        "MID" => {
            let s = str_arg(0);
            let start = (num(1) as usize).saturating_sub(1);
            let len = num(2) as usize;
            let chars: Vec<char> = s.chars().collect();
            let end = (start + len).min(chars.len());
            Some(FormulaResult::Str(chars[start..end].iter().collect()))
        }
        "LEN" => Some(FormulaResult::Number(str_arg(0).len() as f64)),
        "TRIM" => {
            let s = str_arg(0);
            let trimmed = s.trim();
            let collapsed: String = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
            Some(FormulaResult::Str(collapsed))
        }
        "UPPER" => Some(FormulaResult::Str(str_arg(0).to_uppercase())),
        "LOWER" => Some(FormulaResult::Str(str_arg(0).to_lowercase())),
        "SUBSTITUTE" => {
            let s = str_arg(0);
            let old = str_arg(1);
            let new = str_arg(2);
            if args.len() > 3 {
                // Replace nth occurrence only
                let n = num(3) as usize;
                let mut count = 0;
                let mut result = String::new();
                let mut last = 0;
                while let Some(pos) = s[last..].find(&old) {
                    if count + 1 == n {
                        result.push_str(&s[last..last + pos]);
                        result.push_str(&new);
                        last += pos + old.len();
                        result.push_str(&s[last..]);
                        return Some(FormulaResult::Str(result));
                    }
                    count += 1;
                    result.push_str(&s[last..last + pos + old.len()]);
                    last += pos + old.len();
                }
                Some(FormulaResult::Str(s))
            } else {
                Some(FormulaResult::Str(s.replace(&old, &new)))
            }
        }
        "FIND" => {
            let find = str_arg(0);
            let within = str_arg(1);
            let start = if args.len() > 2 {
                (num(2) as usize).saturating_sub(1)
            } else {
                0
            };
            match within[start..].find(&find) {
                Some(pos) => Some(FormulaResult::Number((start + pos + 1) as f64)),
                None => Some(FormulaResult::Error("#VALUE!".to_string())),
            }
        }
        "SEARCH" => {
            let find = str_arg(0).to_lowercase();
            let within = str_arg(1);
            let start = if args.len() > 2 {
                (num(2) as usize).saturating_sub(1)
            } else {
                0
            };
            match within[start..].to_lowercase().find(&find) {
                Some(pos) => Some(FormulaResult::Number((start + pos + 1) as f64)),
                None => Some(FormulaResult::Error("#VALUE!".to_string())),
            }
        }
        "REPT" => Some(FormulaResult::Str(str_arg(0).repeat(num(1) as usize))),
        "CHAR" => Some(FormulaResult::Str((num(0) as u8 as char).to_string())),
        "CODE" => Some(FormulaResult::Number(
            str_arg(0).chars().next().map(|c| c as usize).unwrap_or(0) as f64,
        )),
        "EXACT" => Some(FormulaResult::Bool(str_arg(0) == str_arg(1))),
        "VALUE" => {
            let s = str_arg(0);
            match s.parse::<f64>() {
                Ok(v) => Some(FormulaResult::Number(v)),
                Err(_) => Some(FormulaResult::Error("#VALUE!".to_string())),
            }
        }
        "TEXT" => {
            // Simplified: just format as number with given decimal places
            let v = num(0);
            let fmt = str_arg(1);
            // Try to interpret format as decimal places
            if fmt.starts_with('0') || fmt.starts_with('#') {
                let decimals = fmt.matches('0').count();
                Some(FormulaResult::Str(format!("{:.1$}", v, decimals)))
            } else {
                Some(FormulaResult::Str(format_number(v)))
            }
        }

        // ===== Lookup & Reference =====
        "VLOOKUP" => {
            if args.len() < 3 {
                return None;
            }
            let _lookup_val = &args[0];
            // Range data from args[1] would need the resolver; simplified scalar version
            let col_idx = num(2) as usize;
            if col_idx < 1 {
                return Some(FormulaResult::Error("#REF!".to_string()));
            }
            // For a full implementation we'd need RangeData; return #N/A as placeholder
            // for range-based lookup
            Some(FormulaResult::Error("#N/A".to_string()))
        }
        "HLOOKUP" => Some(FormulaResult::Error("#N/A".to_string())),
        "INDEX" => {
            if args.len() < 2 {
                return None;
            }
            // Simplified: for array args, return element at index
            if let FormulaResult::Array(ref a) = args[0] {
                let idx = num(1) as usize;
                if idx >= 1 && idx <= a.len() {
                    return Some(FormulaResult::Number(a[idx - 1]));
                }
                return Some(FormulaResult::Error("#REF!".to_string()));
            }
            None
        }
        "MATCH" => Some(FormulaResult::Error("#N/A".to_string())),
        "ROW" => Some(FormulaResult::Number(1.0)), // Simplified
        "COLUMN" => Some(FormulaResult::Number(1.0)), // Simplified
        "ADDRESS" => {
            let row = num(0) as usize;
            let col = num(1) as usize;
            Some(FormulaResult::Str(format!("{}{}", index_to_col(col), row)))
        }

        // ===== Date & Time =====
        "TODAY" => {
            let days = chrono_days_since_epoch_today();
            Some(FormulaResult::Number(days as f64))
        }
        "NOW" => {
            let days = chrono_days_since_epoch_today();
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let frac = (secs % 86400) as f64 / 86400.0;
            Some(FormulaResult::Number(days as f64 + frac))
        }
        "DATE" => {
            let y = num(0) as i32;
            let m = num(1) as u32;
            let d = num(2) as u32;
            // OLE Automation date for 1900-01-01 is 1
            // Simplified: use days since 1899-12-30
            let base = chrono_date_to_oa(y, m, d);
            Some(FormulaResult::Number(base))
        }
        "YEAR" | "MONTH" | "DAY" | "HOUR" | "MINUTE" | "SECOND" => {
            // Simplified: extract from OLE date
            // For a proper implementation we'd need chrono
            Some(FormulaResult::Number(0.0))
        }

        // ===== Statistical =====
        "MEDIAN" => {
            let mut n = nums();
            if n.is_empty() {
                return None;
            }
            n.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mid = n.len() / 2;
            let v = if n.len() % 2 == 0 {
                (n[mid - 1] + n[mid]) / 2.0
            } else {
                n[mid]
            };
            Some(FormulaResult::Number(v))
        }
        "STDEV" | "STDEV_S" => eval_stdev(&nums(), true),
        "STDEVP" | "STDEV_P" => eval_stdev(&nums(), false),
        "VAR" | "VAR_S" => eval_var(&nums(), true),
        "VARP" | "VAR_P" => eval_var(&nums(), false),
        "COUNTIF" => {
            if args.len() < 2 {
                return None;
            }
            let criteria = str_arg(1);
            let n = nums();
            let count = n
                .iter()
                .filter(|v| matches_criteria_f64(**v, &criteria))
                .count();
            Some(FormulaResult::Number(count as f64))
        }
        "COUNTIFS" => {
            // Simplified: same as COUNTIF for first pair
            if args.len() < 2 {
                return None;
            }
            let criteria = str_arg(1);
            let n = nums();
            let count = n
                .iter()
                .filter(|v| matches_criteria_f64(**v, &criteria))
                .count();
            Some(FormulaResult::Number(count as f64))
        }

        // ===== Conditional Aggregation =====
        "SUMIF" => {
            if args.len() < 2 {
                return None;
            }
            let criteria = str_arg(1);
            let n = nums();
            let sum: f64 = n
                .iter()
                .filter(|v| matches_criteria_f64(**v, &criteria))
                .sum();
            Some(FormulaResult::Number(sum))
        }
        "SUMIFS" => {
            if args.is_empty() {
                return Some(FormulaResult::Number(0.0));
            }
            // Simplified
            let n = nums();
            Some(FormulaResult::Number(n.iter().sum()))
        }
        "AVERAGEIF" => {
            if args.len() < 2 {
                return None;
            }
            let criteria = str_arg(1);
            let n = nums();
            let matching: Vec<f64> = n
                .iter()
                .cloned()
                .filter(|v| matches_criteria_f64(*v, &criteria))
                .collect();
            if matching.is_empty() {
                Some(FormulaResult::Error("#DIV/0!".to_string()))
            } else {
                Some(FormulaResult::Number(
                    matching.iter().sum::<f64>() / matching.len() as f64,
                ))
            }
        }
        "AVERAGEIFS" => {
            let n = nums();
            if n.is_empty() {
                Some(FormulaResult::Error("#DIV/0!".to_string()))
            } else {
                Some(FormulaResult::Number(
                    n.iter().sum::<f64>() / n.len() as f64,
                ))
            }
        }
        "MAXIFS" => {
            let n = nums();
            n.iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
                .pipe(|v| {
                    if v == f64::NEG_INFINITY {
                        Some(FormulaResult::Number(0.0))
                    } else {
                        Some(FormulaResult::Number(v))
                    }
                })
        }
        "MINIFS" => {
            let n = nums();
            n.iter().cloned().fold(f64::INFINITY, f64::min).pipe(|v| {
                if v == f64::INFINITY {
                    Some(FormulaResult::Number(0.0))
                } else {
                    Some(FormulaResult::Number(v))
                }
            })
        }

        // ===== Info =====
        "ISNUMBER" => Some(FormulaResult::Bool(
            args.first().map(|a| a.is_numeric()).unwrap_or(false),
        )),
        "ISTEXT" => Some(FormulaResult::Bool(
            args.first().map(|a| a.is_string()).unwrap_or(false),
        )),
        "ISBLANK" => Some(FormulaResult::Bool(
            args.first().map(|a| a.is_blank()).unwrap_or(true),
        )),
        "ISERROR" | "ISERR" => Some(FormulaResult::Bool(
            args.first().map(|a| a.is_error()).unwrap_or(false),
        )),
        "ISNA" => Some(FormulaResult::Bool(
            args.first()
                .map(|a| matches!(a, FormulaResult::Error(e) if e == "#N/A"))
                .unwrap_or(false),
        )),
        "TYPE" => Some(FormulaResult::Number(match args.first() {
            Some(FormulaResult::Number(_)) => 1.0,
            Some(FormulaResult::Str(_)) => 2.0,
            Some(FormulaResult::Bool(_)) => 4.0,
            Some(FormulaResult::Error(_)) => 16.0,
            _ => 1.0,
        })),

        // ===== Financial =====
        "PMT" => eval_pmt(args),
        "FV" => eval_fv(args),
        "PV" => eval_pv(args),
        "NPER" => eval_nper(args),
        "NPV" => eval_npv(args),

        // ===== Trigonometry =====
        "PI" => Some(FormulaResult::Number(std::f64::consts::PI)),
        "SIN" => Some(FormulaResult::Number(num(0).sin())),
        "COS" => Some(FormulaResult::Number(num(0).cos())),
        "TAN" => Some(FormulaResult::Number(num(0).tan())),
        "ASIN" => Some(FormulaResult::Number(num(0).asin())),
        "ACOS" => Some(FormulaResult::Number(num(0).acos())),
        "ATAN" => Some(FormulaResult::Number(num(0).atan())),
        "ATAN2" => Some(FormulaResult::Number(num(0).atan2(num(1)))),
        "SINH" => Some(FormulaResult::Number(num(0).sinh())),
        "COSH" => Some(FormulaResult::Number(num(0).cosh())),
        "TANH" => Some(FormulaResult::Number(num(0).tanh())),
        "DEGREES" => Some(FormulaResult::Number(num(0) * 180.0 / std::f64::consts::PI)),
        "RADIANS" => Some(FormulaResult::Number(num(0) * std::f64::consts::PI / 180.0)),
        "EXP" => Some(FormulaResult::Number(num(0).exp())),
        "LN" => Some(FormulaResult::Number(num(0).ln())),
        "LOG10" => Some(FormulaResult::Number(num(0).log10())),
        "LOG" => {
            if args.len() >= 2 {
                Some(FormulaResult::Number(num(0).log(num(1))))
            } else {
                Some(FormulaResult::Number(num(0).log10()))
            }
        }
        "SIGN" => Some(FormulaResult::Number(num(0).signum())),
        "FACT" => Some(FormulaResult::Number(factorial(num(0)))),
        "RAND" => Some(FormulaResult::Number(rand_f64())),
        "RANDBETWEEN" => Some(FormulaResult::Number(
            num(0) + (rand_f64() * (num(1) - num(0) + 1.0)).floor(),
        )),

        // ===== Conversions =====
        "ROMAN" => Some(FormulaResult::Str(to_roman(num(0) as i32))),
        "BIN2DEC" => Some(FormulaResult::Number(
            i64::from_str_radix(&str_arg(0), 2).unwrap_or(0) as f64,
        )),
        "DEC2BIN" => Some(FormulaResult::Str(format!("{:b}", num(0) as i64))),
        "HEX2DEC" => Some(FormulaResult::Number(
            i64::from_str_radix(&str_arg(0), 16).unwrap_or(0) as f64,
        )),
        "DEC2HEX" => Some(FormulaResult::Str(format!("{:X}", num(0) as i64))),
        "OCT2DEC" => Some(FormulaResult::Number(
            i64::from_str_radix(&str_arg(0), 8).unwrap_or(0) as f64,
        )),
        "DEC2OCT" => Some(FormulaResult::Str(format!("{:o}", num(0) as i64))),

        "NA" => Some(FormulaResult::Error("#N/A".to_string())),

        // Unimplemented functions return None (caller treats as "not evaluated")
        _ => None,
    }
}

// ─── Helper functions ────────────────────────────────────────────────────

fn flatten_numbers(args: &[FormulaResult]) -> Vec<f64> {
    let mut result = Vec::new();
    for a in args {
        match a {
            FormulaResult::Array(arr) => result.extend(arr.iter().filter(|v| !v.is_nan())),
            FormulaResult::Matrix(rows) => result.extend(
                rows.iter()
                    .flatten()
                    .map(FormulaResult::as_number)
                    .filter(|value| !value.is_nan()),
            ),
            FormulaResult::Number(v) if !v.is_nan() => result.push(*v),
            FormulaResult::Bool(v) => result.push(if *v { 1.0 } else { 0.0 }),
            FormulaResult::Str(s) => {
                if let Ok(v) = s.parse::<f64>() {
                    result.push(v);
                }
            }
            _ => {}
        }
    }
    result
}

fn eval_sequence(args: &[FormulaResult]) -> FormulaResult {
    let rows = args.first().map(FormulaResult::as_number).unwrap_or(1.0) as isize;
    let columns = args.get(1).map(FormulaResult::as_number).unwrap_or(1.0) as isize;
    let start = args.get(2).map(FormulaResult::as_number).unwrap_or(1.0);
    let step = args.get(3).map(FormulaResult::as_number).unwrap_or(1.0);
    if rows < 1 || columns < 1 {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let _count = match (rows as usize).checked_mul(columns as usize) {
        Some(count) if count <= 1_000_000 => count,
        _ => return FormulaResult::Error("#NUM!".to_string()),
    };
    FormulaResult::Matrix(
        (0..rows as usize)
            .map(|row| {
                (0..columns as usize)
                    .map(|column| {
                        FormulaResult::Number(
                            start + (row * columns as usize + column) as f64 * step,
                        )
                    })
                    .collect()
            })
            .collect(),
    )
}

fn eval_sort(args: &[FormulaResult]) -> FormulaResult {
    let mut values = rectangular_matrix(args.first());
    if values.is_empty() {
        return FormulaResult::Matrix(values);
    }
    let sort_index = args.get(1).map(FormulaResult::as_number).unwrap_or(1.0) as isize;
    let by_column = args.get(3).is_some_and(|value| value.as_number() != 0.0);
    let descending = args.get(2).is_some_and(|value| value.as_number() < 0.0);
    if by_column {
        values = transpose(&values);
    }
    let key = sort_index.clamp(1, values[0].len() as isize) as usize - 1;
    values.sort_by(|left, right| compare_values(&left[key], &right[key]).cmp(&0));
    if descending {
        values.reverse();
    }
    if by_column {
        values = transpose(&values);
    }
    FormulaResult::Matrix(values)
}

fn eval_sortby(args: &[FormulaResult]) -> FormulaResult {
    if args.len() < 2 {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let mut values = rectangular_matrix(args.first());
    let keys = flatten_matrix(args.get(1));
    if values.len() != keys.len() {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let descending = args.get(2).is_some_and(|value| value.as_number() < 0.0);
    let mut pairs: Vec<_> = values.drain(..).zip(keys).collect();
    pairs.sort_by(|left, right| compare_values(&left.1, &right.1).cmp(&0));
    if descending {
        pairs.reverse();
    }
    FormulaResult::Matrix(pairs.into_iter().map(|(value, _)| value).collect())
}

fn eval_unique(args: &[FormulaResult]) -> FormulaResult {
    let source = rectangular_matrix(args.first());
    let exactly_once = args.get(2).is_some_and(|value| value.as_number() != 0.0);
    let mut result: Vec<Vec<FormulaResult>> = Vec::new();
    for value in &source {
        let occurrences = source
            .iter()
            .filter(|other| rows_equal(other, value))
            .count();
        if (!exactly_once || occurrences == 1) && !result.iter().any(|row| rows_equal(row, value)) {
            result.push(value.clone());
        }
    }
    FormulaResult::Matrix(result)
}

fn eval_filter(args: &[FormulaResult]) -> FormulaResult {
    if args.len() < 2 {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let source = rectangular_matrix(args.first());
    let include = flatten_matrix(args.get(1));
    if source.is_empty() {
        return FormulaResult::Matrix(source);
    }
    if include.len() == source.len() {
        let result: Vec<_> = source
            .into_iter()
            .zip(include)
            .filter_map(|(row, keep)| (keep.as_number() != 0.0).then_some(row))
            .collect();
        return if result.is_empty() {
            args.get(2)
                .cloned()
                .unwrap_or_else(|| FormulaResult::Error("#CALC!".to_string()))
        } else {
            FormulaResult::Matrix(result)
        };
    }
    if include.len() != source[0].len() {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let result: Vec<Vec<FormulaResult>> = source
        .into_iter()
        .map(|row| {
            row.into_iter()
                .zip(&include)
                .filter_map(|(value, keep)| (keep.as_number() != 0.0).then_some(value))
                .collect()
        })
        .collect();
    if result.is_empty() {
        args.get(2)
            .cloned()
            .unwrap_or_else(|| FormulaResult::Error("#CALC!".to_string()))
    } else {
        FormulaResult::Matrix(result)
    }
}

fn eval_xlookup(args: &[FormulaResult]) -> FormulaResult {
    if args.len() < 3 {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let lookup = &args[0];
    let lookup_values = flatten_matrix(args.get(1));
    let return_values = rectangular_matrix(args.get(2));
    if lookup_values.len() != return_values.len() {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    lookup_values
        .iter()
        .position(|value| compare_values(value, lookup) == 0)
        .and_then(|index| return_values.get(index).cloned())
        .map(|row| {
            if row.len() == 1 {
                row[0].clone()
            } else {
                FormulaResult::Matrix(vec![row])
            }
        })
        .or_else(|| args.get(3).cloned())
        .unwrap_or_else(|| FormulaResult::Error("#N/A".to_string()))
}

fn eval_xmatch(args: &[FormulaResult]) -> FormulaResult {
    if args.len() < 2 {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let lookup = &args[0];
    flatten_matrix(args.get(1))
        .iter()
        .position(|value| compare_values(value, lookup) == 0)
        .map(|index| FormulaResult::Number((index + 1) as f64))
        .unwrap_or_else(|| FormulaResult::Error("#N/A".to_string()))
}

fn eval_take(args: &[FormulaResult]) -> FormulaResult {
    let values = rectangular_matrix(args.first());
    let rows = args.get(1).map(FormulaResult::as_number).unwrap_or(0.0) as isize;
    if rows == 0 {
        return FormulaResult::Error("#CALC!".to_string());
    }
    let count = rows.unsigned_abs().min(values.len());
    if rows > 0 {
        FormulaResult::Matrix(values.into_iter().take(count).collect())
    } else {
        let start = values.len() - count;
        FormulaResult::Matrix(values.into_iter().skip(start).collect())
    }
}

fn eval_drop(args: &[FormulaResult]) -> FormulaResult {
    let values = rectangular_matrix(args.first());
    let rows = args.get(1).map(FormulaResult::as_number).unwrap_or(0.0) as isize;
    if rows.unsigned_abs() >= values.len() {
        return FormulaResult::Error("#CALC!".to_string());
    }
    if rows >= 0 {
        FormulaResult::Matrix(values.into_iter().skip(rows as usize).collect())
    } else {
        let count = values.len() - rows.unsigned_abs();
        FormulaResult::Matrix(values.into_iter().take(count).collect())
    }
}

fn eval_randarray(args: &[FormulaResult]) -> FormulaResult {
    let rows = args.first().map(FormulaResult::as_number).unwrap_or(1.0) as isize;
    let columns = args.get(1).map(FormulaResult::as_number).unwrap_or(1.0) as isize;
    let min = args.get(2).map(FormulaResult::as_number).unwrap_or(0.0);
    let max = args.get(3).map(FormulaResult::as_number).unwrap_or(1.0);
    let whole = args.get(4).is_some_and(|value| value.as_number() != 0.0);
    if rows < 1 || columns < 1 || min > max {
        return FormulaResult::Error("#VALUE!".to_string());
    }
    let _count = match (rows as usize).checked_mul(columns as usize) {
        Some(count) if count <= 1_000_000 => count,
        _ => return FormulaResult::Error("#NUM!".to_string()),
    };
    FormulaResult::Matrix(
        (0..rows as usize)
            .map(|_| {
                (0..columns as usize)
                    .map(|_| {
                        let value = min + rand_f64() * (max - min + if whole { 1.0 } else { 0.0 });
                        FormulaResult::Number(if whole { value.floor() } else { value })
                    })
                    .collect()
            })
            .collect(),
    )
}

fn rectangular_matrix(value: Option<&FormulaResult>) -> Vec<Vec<FormulaResult>> {
    let mut rows = value.map(FormulaResult::as_matrix).unwrap_or_default();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut rows {
        row.resize(columns, FormulaResult::Blank);
    }
    rows
}

fn flatten_matrix(value: Option<&FormulaResult>) -> Vec<FormulaResult> {
    rectangular_matrix(value).into_iter().flatten().collect()
}

fn transpose(rows: &[Vec<FormulaResult>]) -> Vec<Vec<FormulaResult>> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    (0..columns)
        .map(|column| {
            rows.iter()
                .map(|row| row.get(column).cloned().unwrap_or(FormulaResult::Blank))
                .collect()
        })
        .collect()
}

fn rows_equal(left: &[FormulaResult], right: &[FormulaResult]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| compare_values(left, right) == 0)
}

fn should_skip(value: &FormulaResult, ignore: isize) -> bool {
    (ignore == 1 || ignore == 3) && value.is_blank()
        || (ignore == 2 || ignore == 3) && value.is_error()
}

fn eval_to_col(args: &[FormulaResult]) -> FormulaResult {
    let ignore = args.get(1).map(FormulaResult::as_number).unwrap_or(0.0) as isize;
    FormulaResult::Matrix(
        flatten_matrix(args.first())
            .into_iter()
            .filter(|value| !should_skip(value, ignore))
            .map(|value| vec![value])
            .collect(),
    )
}

fn eval_to_row(args: &[FormulaResult]) -> FormulaResult {
    let ignore = args.get(1).map(FormulaResult::as_number).unwrap_or(0.0) as isize;
    FormulaResult::Matrix(vec![flatten_matrix(args.first())
        .into_iter()
        .filter(|value| !should_skip(value, ignore))
        .collect()])
}

fn eval_hstack(args: &[FormulaResult]) -> FormulaResult {
    // Legacy `Array` values predate matrix geometry and historically behaved
    // as row vectors for HSTACK. Preserve that public evaluator behavior while
    // true Matrix values retain their declared shape.
    let matrices: Vec<_> = args
        .iter()
        .map(|value| match value {
            FormulaResult::Array(values) => {
                vec![values.iter().copied().map(FormulaResult::Number).collect()]
            }
            value => rectangular_matrix(Some(value)),
        })
        .collect();
    let rows = matrices.iter().map(Vec::len).max().unwrap_or(0);
    let mut result = vec![Vec::new(); rows];
    for matrix in matrices {
        let columns = matrix.first().map(Vec::len).unwrap_or(0);
        for (row_index, row) in result.iter_mut().enumerate() {
            if let Some(values) = matrix.get(row_index) {
                row.extend(values.clone());
            } else {
                row.extend(std::iter::repeat_n(
                    FormulaResult::Error("#N/A".to_string()),
                    columns,
                ));
            }
        }
    }
    FormulaResult::Matrix(result)
}

fn eval_vstack(args: &[FormulaResult]) -> FormulaResult {
    let matrices: Vec<_> = args
        .iter()
        .map(|value| rectangular_matrix(Some(value)))
        .collect();
    let columns = matrices
        .iter()
        .filter_map(|matrix| matrix.first().map(Vec::len))
        .max()
        .unwrap_or(0);
    let mut result = Vec::new();
    for mut matrix in matrices {
        for row in &mut matrix {
            row.resize(columns, FormulaResult::Error("#N/A".to_string()));
        }
        result.extend(matrix);
    }
    FormulaResult::Matrix(result)
}

fn eval_stdev(nums: &[f64], sample: bool) -> Option<FormulaResult> {
    let min_len = if sample { 2 } else { 1 };
    if nums.len() < min_len {
        return Some(FormulaResult::Error("#DIV/0!".to_string()));
    }
    let mean = nums.iter().sum::<f64>() / nums.len() as f64;
    let sum_sq = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>();
    let n = if sample { nums.len() - 1 } else { nums.len() };
    Some(FormulaResult::Number((sum_sq / n as f64).sqrt()))
}

fn eval_var(nums: &[f64], sample: bool) -> Option<FormulaResult> {
    let min_len = if sample { 2 } else { 1 };
    if nums.len() < min_len {
        return Some(FormulaResult::Error("#DIV/0!".to_string()));
    }
    let mean = nums.iter().sum::<f64>() / nums.len() as f64;
    let sum_sq = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>();
    let n = if sample { nums.len() - 1 } else { nums.len() };
    Some(FormulaResult::Number(sum_sq / n as f64))
}

fn eval_pmt(args: &[FormulaResult]) -> Option<FormulaResult> {
    if args.len() < 3 {
        return None;
    }
    let rate = args[0].as_number();
    let nper = args[1].as_number();
    let pv = args[2].as_number();
    let fv = args.get(3).map(|a| a.as_number()).unwrap_or(0.0);
    if rate == 0.0 {
        return Some(FormulaResult::Number(-(pv + fv) / nper));
    }
    Some(FormulaResult::Number(
        -(rate * (pv * (1.0 + rate).powf(nper) + fv)) / ((1.0 + rate).powf(nper) - 1.0),
    ))
}

fn eval_fv(args: &[FormulaResult]) -> Option<FormulaResult> {
    if args.len() < 3 {
        return None;
    }
    let rate = args[0].as_number();
    let nper = args[1].as_number();
    let pmt = args[2].as_number();
    let pv = args.get(3).map(|a| a.as_number()).unwrap_or(0.0);
    if rate == 0.0 {
        return Some(FormulaResult::Number(-(pv + pmt * nper)));
    }
    Some(FormulaResult::Number(
        -(pv * (1.0 + rate).powf(nper) + pmt * ((1.0 + rate).powf(nper) - 1.0) / rate),
    ))
}

fn eval_pv(args: &[FormulaResult]) -> Option<FormulaResult> {
    if args.len() < 3 {
        return None;
    }
    let rate = args[0].as_number();
    let nper = args[1].as_number();
    let pmt = args[2].as_number();
    let fv = args.get(3).map(|a| a.as_number()).unwrap_or(0.0);
    if rate == 0.0 {
        return Some(FormulaResult::Number(-(fv + pmt * nper)));
    }
    Some(FormulaResult::Number(
        -(fv / (1.0 + rate).powf(nper) + pmt * (1.0 - (1.0 + rate).powf(-nper)) / rate),
    ))
}

fn eval_nper(args: &[FormulaResult]) -> Option<FormulaResult> {
    if args.len() < 3 {
        return None;
    }
    let rate = args[0].as_number();
    let pmt = args[1].as_number();
    let pv = args[2].as_number();
    let fv = args.get(3).map(|a| a.as_number()).unwrap_or(0.0);
    if rate == 0.0 {
        if pmt == 0.0 {
            return None;
        }
        return Some(FormulaResult::Number(-(pv + fv) / pmt));
    }
    Some(FormulaResult::Number(
        ((-fv * rate + pmt) / (pv * rate + pmt)).ln() / (1.0 + rate).ln(),
    ))
}

fn eval_npv(args: &[FormulaResult]) -> Option<FormulaResult> {
    if args.len() < 2 {
        return None;
    }
    let rate = args[0].as_number();
    let mut npv = 0.0;
    for (i, arg) in args[1..].iter().enumerate() {
        npv += arg.as_number() / (1.0 + rate).powi(i as i32 + 1);
    }
    Some(FormulaResult::Number(npv))
}

/// Simple criteria matching for SUMIF/COUNTIF.
/// Supports: plain value, >5, <10, >=3, <=7, <>0
fn matches_criteria_f64(value: f64, criteria: &str) -> bool {
    let criteria = criteria.trim();
    if let Some(rest) = criteria.strip_prefix(">=") {
        rest.parse::<f64>().map(|c| value >= c).unwrap_or(false)
    } else if let Some(rest) = criteria.strip_prefix("<=") {
        rest.parse::<f64>().map(|c| value <= c).unwrap_or(false)
    } else if let Some(rest) = criteria.strip_prefix("<>") {
        rest.parse::<f64>()
            .map(|c| (value - c).abs() > 1e-10)
            .unwrap_or(false)
    } else if let Some(rest) = criteria.strip_prefix('>') {
        rest.parse::<f64>().map(|c| value > c).unwrap_or(false)
    } else if let Some(rest) = criteria.strip_prefix('<') {
        rest.parse::<f64>().map(|c| value < c).unwrap_or(false)
    } else if let Some(rest) = criteria.strip_prefix('=') {
        rest.parse::<f64>()
            .map(|c| (value - c).abs() < 1e-10)
            .unwrap_or(false)
    } else {
        criteria
            .parse::<f64>()
            .map(|c| (value - c).abs() < 1e-10)
            .unwrap_or(false)
    }
}

fn factorial(n: f64) -> f64 {
    let n = n as i64;
    if n < 0 {
        return f64::NAN;
    }
    let mut result = 1.0;
    for i in 2..=n {
        result *= i as f64;
    }
    result
}

fn rand_f64() -> f64 {
    // Use a simple fast PRNG; not crypto-quality
    use std::time::SystemTime;
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Simple xorshift
    let mut x = t as u64;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x as f64) / (u64::MAX as f64)
}

fn to_roman(mut num: i32) -> String {
    if num <= 0 || num > 3999 {
        return format!("{}", num);
    }
    let pairs = [
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
    ];
    let mut result = String::new();
    for (value, symbol) in &pairs {
        while num >= *value {
            result.push_str(symbol);
            num -= value;
        }
    }
    result
}

/// OLE Automation date from year/month/day.
fn chrono_date_to_oa(y: i32, m: u32, d: u32) -> f64 {
    // OLE Automation date: days since 1899-12-30
    // Simplified calculation
    let mut days = 0i32;
    // Days from year 1900
    for yr in 1900..y {
        days += if is_leap_year(yr) { 366 } else { 365 };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for mo in 1..m {
        days += month_days[mo as usize];
        if mo == 2 && is_leap_year(y) {
            days += 1;
        }
    }
    days += d as i32;
    // 1900-01-01 = OLE date 2 (Excel bug: treats 1900 as leap year)
    days as f64 + 1.0
}

fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn chrono_days_since_epoch_today() -> i32 {
    // Days from 1899-12-30 to today
    // Simplified: use current time
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let unix_days = duration.as_secs() / 86400;
    // Unix epoch (1970-01-01) is OLE date 25569
    (unix_days as i32) + 25569
}

// ─── Pipe trait for chaining ─────────────────────────────────────────────

trait Pipe<T> {
    fn pipe<U, F: FnOnce(T) -> U>(self, f: F) -> U;
}

impl<T> Pipe<T> for T {
    fn pipe<U, F: FnOnce(T) -> U>(self, f: F) -> U {
        f(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EmptyResolver;

    impl CellResolver for EmptyResolver {
        fn resolve_cell(&self, _cell_ref: &str) -> FormulaResult {
            FormulaResult::Blank
        }

        fn resolve_sheet_cell(&self, _sheet_cell_ref: &str) -> FormulaResult {
            FormulaResult::Blank
        }

        fn expand_range(&self, _range_expr: &str) -> Vec<(String, FormulaResult)> {
            Vec::new()
        }
    }

    fn numbers(result: Option<FormulaResult>) -> Vec<f64> {
        match result {
            Some(FormulaResult::Array(values)) => values,
            Some(FormulaResult::Matrix(rows)) => rows
                .into_iter()
                .flatten()
                .map(|value| value.as_number())
                .collect(),
            other => panic!("expected numeric array, got {other:?}"),
        }
    }

    fn matrix(result: Option<FormulaResult>) -> Vec<Vec<FormulaResult>> {
        match result {
            Some(FormulaResult::Matrix(rows)) => rows,
            other => panic!("expected matrix, got {other:?}"),
        }
    }

    #[test]
    fn modern_dynamic_functions_accept_ooxml_function_names() {
        let resolver = EmptyResolver;
        assert!(matches!(
            super::super::evaluate_with_resolver("_xlfn.SEQUENCE(4)", &resolver),
            FormulaResult::Number(value) if value == 1.0
        ));
        assert_eq!(
            numbers(eval_function(
                "_XLFN_SEQUENCE",
                &[FormulaResult::Number(4.0), FormulaResult::Number(1.0)],
                &resolver,
            )),
            vec![1.0, 2.0, 3.0, 4.0]
        );
        assert_eq!(
            numbers(eval_function(
                "_XLFN__XLWS_FILTER",
                &[
                    FormulaResult::Array(vec![10.0, 20.0, 30.0]),
                    FormulaResult::Array(vec![1.0, 0.0, 1.0]),
                ],
                &resolver,
            )),
            vec![10.0, 30.0]
        );
    }

    #[test]
    fn modern_lookup_sort_unique_take_and_drop_evaluate_numeric_arrays() {
        let resolver = EmptyResolver;
        assert_eq!(
            numbers(eval_function(
                "SORT",
                &[FormulaResult::Array(vec![3.0, 1.0, 2.0])],
                &resolver,
            )),
            vec![1.0, 2.0, 3.0]
        );
        assert_eq!(
            numbers(eval_function(
                "UNIQUE",
                &[FormulaResult::Array(vec![2.0, 1.0, 2.0, 3.0])],
                &resolver,
            )),
            vec![2.0, 1.0, 3.0]
        );
        assert!(matches!(
            eval_function(
                "XLOOKUP",
                &[
                    FormulaResult::Number(2.0),
                    FormulaResult::Array(vec![1.0, 2.0]),
                    FormulaResult::Array(vec![10.0, 20.0]),
                ],
                &resolver,
            ),
            Some(FormulaResult::Number(20.0))
        ));
        assert_eq!(
            numbers(eval_function(
                "TAKE",
                &[
                    FormulaResult::Array(vec![1.0, 2.0, 3.0]),
                    FormulaResult::Number(-2.0)
                ],
                &resolver,
            )),
            vec![2.0, 3.0]
        );
        assert_eq!(
            numbers(eval_function(
                "DROP",
                &[
                    FormulaResult::Array(vec![1.0, 2.0, 3.0]),
                    FormulaResult::Number(1.0)
                ],
                &resolver,
            )),
            vec![2.0, 3.0]
        );
        assert_eq!(
            numbers(eval_function(
                "SORTBY",
                &[
                    FormulaResult::Array(vec![10.0, 30.0, 20.0]),
                    FormulaResult::Array(vec![1.0, 3.0, 2.0]),
                ],
                &resolver,
            )),
            vec![10.0, 20.0, 30.0]
        );
        assert_eq!(
            numbers(eval_function(
                "_XLFN_HSTACK",
                &[
                    FormulaResult::Array(vec![1.0, 2.0]),
                    FormulaResult::Array(vec![3.0]),
                ],
                &resolver,
            )),
            vec![1.0, 2.0, 3.0]
        );
        assert_eq!(
            numbers(eval_function(
                "_XLFN_TOCOL",
                &[FormulaResult::Array(vec![2.0, 4.0])],
                &resolver,
            )),
            vec![2.0, 4.0]
        );
    }

    #[test]
    fn modern_dynamic_functions_preserve_strings_and_two_dimensional_shape() {
        let resolver = EmptyResolver;
        assert_eq!(
            matrix(eval_function(
                "SEQUENCE",
                &[FormulaResult::Number(2.0), FormulaResult::Number(3.0)],
                &resolver,
            )),
            vec![
                vec![
                    FormulaResult::Number(1.0),
                    FormulaResult::Number(2.0),
                    FormulaResult::Number(3.0),
                ],
                vec![
                    FormulaResult::Number(4.0),
                    FormulaResult::Number(5.0),
                    FormulaResult::Number(6.0),
                ],
            ]
        );
        assert_eq!(
            matrix(eval_function(
                "FILTER",
                &[
                    FormulaResult::Matrix(vec![
                        vec![
                            FormulaResult::Str("alpha".to_string()),
                            FormulaResult::Number(1.0)
                        ],
                        vec![
                            FormulaResult::Str("beta".to_string()),
                            FormulaResult::Number(2.0)
                        ],
                    ]),
                    FormulaResult::Matrix(vec![
                        vec![FormulaResult::Bool(false)],
                        vec![FormulaResult::Bool(true)],
                    ]),
                ],
                &resolver,
            )),
            vec![vec![
                FormulaResult::Str("beta".to_string()),
                FormulaResult::Number(2.0)
            ]]
        );
        assert_eq!(
            matrix(eval_function(
                "HSTACK",
                &[
                    FormulaResult::Matrix(vec![vec![FormulaResult::Str("A".to_string())]]),
                    FormulaResult::Matrix(vec![vec![
                        FormulaResult::Str("B".to_string()),
                        FormulaResult::Str("C".to_string())
                    ]]),
                ],
                &resolver,
            )),
            vec![vec![
                FormulaResult::Str("A".to_string()),
                FormulaResult::Str("B".to_string()),
                FormulaResult::Str("C".to_string()),
            ]]
        );
    }
}
