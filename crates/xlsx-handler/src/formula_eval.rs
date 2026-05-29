/// Formula evaluator for xlsx cells.
/// This is a stub implementation — full formula evaluation is complex
/// and typically requires a spreadsheet engine. For now, we just
/// return formula strings as-is.

/// Evaluate a formula string and return the result.
/// Currently a no-op stub that returns the formula itself.
pub fn evaluate_formula(_formula: &str) -> Option<String> {
    // Stub: just return None to indicate we don't evaluate
    // In the future this could call a proper evaluator
    None
}

/// Check if a formula is a simple arithmetic expression that can be evaluated.
pub fn is_simple_arithmetic(formula: &str) -> bool {
    let stripped = formula.trim_start_matches('=');
    // Check if it only contains digits, operators, and parentheses
    stripped
        .chars()
        .all(|c| c.is_ascii_digit() || "+-*/() .".contains(c))
}
