//! Formula evaluator module for xlsx documents.
//!
//! Supports 80+ Excel-compatible functions including math, logical, text,
//! lookup, date/time, statistical, conditional, financial, and trig functions.

mod functions;
mod modern;
mod parser;
mod resolver;
pub mod tokenizer;
pub mod types;

pub use modern::{is_dynamic_array_formula, qualify_for_ooxml, unqualify_for_readback};
pub use parser::CellResolver;
pub use resolver::WorkbookResolver;
pub use types::{format_number, FormulaResult};

/// Evaluate a formula string (without leading '=') against a workbook model.
/// Returns None if the formula cannot be evaluated.
pub fn evaluate(formula: &str, model: &crate::dom_types::WorkbookModel) -> Option<FormulaResult> {
    let resolver = WorkbookResolver::new(model);
    Some(evaluate_with_resolver(formula, &resolver))
}

/// Evaluate a formula string with a specific resolver.
pub fn evaluate_with_resolver(formula: &str, resolver: &dyn CellResolver) -> FormulaResult {
    match parser::FormulaParser::new(formula, resolver) {
        Ok(mut p) => p
            .evaluate()
            .unwrap_or(FormulaResult::Error("#VALUE!".to_string())),
        Err(_) => FormulaResult::Error("#NAME?".to_string()),
    }
}

/// Evaluate a formula while retaining any dynamic-array matrix result.
pub fn evaluate_spill(
    formula: &str,
    model: &crate::dom_types::WorkbookModel,
) -> Option<FormulaResult> {
    let resolver = WorkbookResolver::new(model);
    parser::FormulaParser::new(formula, &resolver)
        .ok()?
        .evaluate_spill()
}
