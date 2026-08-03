//! Find-and-replace utilities shared across document handlers.
//!
//! Matches the C# FindHelpers shape: a `find` text (case-sensitive or case-insensitive,
//! optional regex) and a `replace` text. The caller is responsible for re-reading the
//! document's TextOffsetMap after each replacement because offsets shift when the
//! text length changes.

use regex::{escape as regex_escape, Regex, RegexBuilder};
use std::collections::HashMap;

/// Result of a find/replace operation.
#[derive(Debug, Clone, Default)]
pub struct FindReplaceResult {
    /// Number of replacements made.
    pub match_count: usize,
    /// Number of replacements actually applied (may be less than match_count if a
    /// limit was set via `max_replacements`).
    pub replaced_count: usize,
}

/// Options for a find/replace pass.
#[derive(Debug, Clone, Default)]
pub struct FindReplaceOptions {
    /// Case-insensitive matching when true.
    pub case_insensitive: bool,
    /// Match whole words only.
    pub whole_word: bool,
    /// Use regex pattern (otherwise literal substring match).
    pub use_regex: bool,
    /// Stop after this many replacements (None = unlimited).
    pub max_replacements: Option<usize>,
}

/// Find all byte offsets where `find` occurs in `text`, honoring case sensitivity.
pub fn find_all_offsets(text: &str, find: &str, opts: &FindReplaceOptions) -> Vec<usize> {
    find_all_spans(text, find, opts)
        .into_iter()
        .map(|(start, _)| start)
        .collect()
}

/// Match the C# CLI's `query --find` vocabulary.
///
/// A normal value is a case-insensitive literal substring. `r"..."` and
/// `r'...'` opt into a case-insensitive regex; the closing quote is found at
/// the final matching quote, matching the upstream command parser. Unlike the
/// older replacement helpers this surfaces an invalid regex so a query cannot
/// silently look like it found no documents.
pub fn matches_text_filter(text: &str, find: &str) -> Result<bool, regex::Error> {
    if let Some(pattern) = parse_regex_prefix(find) {
        let options = FindReplaceOptions {
            case_insensitive: true,
            use_regex: true,
            ..Default::default()
        };
        return Ok(compile_find_regex(pattern, &options)?.is_match(text));
    }

    Ok(text.to_lowercase().contains(&find.to_lowercase()))
}

/// Return the pattern in the C# raw-regex `r"..."` / `r'...'` syntax.
pub fn parse_regex_prefix(find: &str) -> Option<&str> {
    let bytes = find.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'r' || !matches!(bytes[1], b'\'' | b'"') {
        return None;
    }

    let quote = bytes[1];
    let end = bytes.iter().rposition(|byte| *byte == quote)?;
    (end > 1).then(|| &find[2..end])
}

/// Find complete byte spans for every match. Unlike `find_all_offsets`, this
/// preserves the end boundary required by structure-aware editors that split
/// runs before replacing or tracking a match.
pub fn find_all_spans(text: &str, find: &str, opts: &FindReplaceOptions) -> Vec<(usize, usize)> {
    if find.is_empty() {
        return Vec::new();
    }

    if opts.use_regex {
        return compile_find_regex(find, opts)
            .map(|regex| {
                regex
                    .find_iter(text)
                    .map(|m| (m.start(), m.end()))
                    .collect()
            })
            .unwrap_or_default();
    }

    let mut spans = Vec::new();

    if opts.case_insensitive {
        let text_lower = text.to_lowercase();
        let find_lower = find.to_lowercase();
        let mut start = 0;
        while let Some(pos) = text_lower[start..].find(&find_lower) {
            let abs = start + pos;
            if !opts.whole_word || is_word_boundary(text, abs, abs + find.len()) {
                spans.push((abs, abs + find.len()));
            }
            start = abs + find.len();
            if start >= text.len() {
                break;
            }
        }
    } else {
        let mut start = 0;
        while let Some(pos) = text[start..].find(find) {
            let abs = start + pos;
            if !opts.whole_word || is_word_boundary(text, abs, abs + find.len()) {
                spans.push((abs, abs + find.len()));
            }
            start = abs + find.len();
            if start >= text.len() {
                break;
            }
        }
    }
    spans
}

/// Return each non-empty match together with the text that should replace it.
/// Regex replacements expand `$1`, `${name}`, and `$$` against that individual
/// match, which structure-aware handlers need when they emit one OOXML change
/// record per match rather than replacing a whole string at once.
pub fn find_all_replacements(
    text: &str,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
) -> Result<Vec<(usize, usize, String)>, regex::Error> {
    if find.is_empty() {
        return Ok(Vec::new());
    }
    let max = opts.max_replacements.unwrap_or(usize::MAX);
    if opts.use_regex {
        let regex = compile_find_regex(find, opts)?;
        return Ok(regex
            .captures_iter(text)
            .filter_map(|captures| {
                let matched = captures.get(0)?;
                (matched.start() < matched.end()).then(|| {
                    let mut expanded = String::new();
                    expand_replacement(&captures, replace, &mut expanded);
                    (matched.start(), matched.end(), expanded)
                })
            })
            .take(max)
            .collect());
    }
    Ok(find_all_spans(text, find, opts)
        .into_iter()
        .take(max)
        .map(|(start, end)| (start, end, replace.to_string()))
        .collect())
}

/// Compile `find` as a regex with the given options. Returns the compiled regex
/// or an error describing the syntax problem.
pub fn compile_find_regex(find: &str, opts: &FindReplaceOptions) -> Result<Regex, regex::Error> {
    let pattern = if opts.whole_word {
        format!(r"\b(?:{})\b", find)
    } else {
        find.to_string()
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(opts.case_insensitive)
        .build()
}

/// Check if the byte range [start, end) is surrounded by word boundaries
/// (whitespace, punctuation, or string start/end on both sides).
fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
    let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
    before_ok && after_ok
}

/// Perform a simple in-memory find/replace on a String.
/// Returns the modified string and a count of replacements.
pub fn replace_in_string(
    text: &str,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
) -> (String, usize) {
    if find.is_empty() {
        return (text.to_string(), 0);
    }

    // Regex path: match lengths vary, so we walk match-by-match.
    if opts.use_regex {
        return match replace_regex(text, find, replace, opts) {
            Ok(out) => out,
            Err(_) => {
                // Invalid pattern — leave text untouched.
                (text.to_string(), 0)
            }
        };
    }

    let offsets = find_all_offsets(text, find, opts);
    let max = opts.max_replacements.unwrap_or(usize::MAX);
    let applied = offsets.len().min(max);

    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for (i, &off) in offsets.iter().enumerate().take(applied) {
        result.push_str(&text[cursor..off]);
        result.push_str(replace);
        cursor = off + find.len();
        let _ = i;
    }
    result.push_str(&text[cursor..]);

    (result, applied)
}

/// Regex-aware replace. Honors max_replacements; supports `$N` / `${N}` /
/// `$name` capture references via the `regex` crate's substitution grammar.
fn replace_regex(
    text: &str,
    find: &str,
    replace: &str,
    opts: &FindReplaceOptions,
) -> Result<(String, usize), regex::Error> {
    let re = compile_find_regex(find, opts)?;
    let max = opts.max_replacements;
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut count = 0usize;

    for caps in re.captures_iter(text) {
        if let Some(max) = max {
            if count >= max {
                break;
            }
        }
        let m = caps.get(0).expect("capture group 0 always present");
        out.push_str(&text[cursor..m.start()]);
        // Apply $N / ${N} / $name substitution manually so we control count
        // and can support max_replacements cleanly.
        expand_replacement(&caps, replace, &mut out);
        cursor = m.end();
        count += 1;
        // Avoid infinite loop on zero-width matches.
        if m.start() == m.end() {
            if cursor >= text.len() {
                break;
            }
            let bytes = text.as_bytes();
            let mut next = cursor + 1;
            while next < bytes.len() && !text.is_char_boundary(next) {
                next += 1;
            }
            out.push_str(&text[cursor..next]);
            cursor = next;
        }
    }
    out.push_str(&text[cursor..]);
    Ok((out, count))
}

/// Expand `$1`, `${name}`, `$$`, and literal text into `out`.
fn expand_replacement(caps: &regex::Captures, replace: &str, out: &mut String) {
    let bytes = replace.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            let nxt = bytes[i + 1];
            match nxt {
                b'$' => {
                    out.push('$');
                    i += 2;
                    continue;
                }
                b'{' => {
                    // ${name} or ${N}
                    if let Some(close_rel) = replace[i + 2..].find('}') {
                        let name = &replace[i + 2..i + 2 + close_rel];
                        if let Some(m) = caps.name(name) {
                            out.push_str(m.as_str());
                        }
                        i += 2 + close_rel + 1;
                        continue;
                    }
                }
                b'0'..=b'9' => {
                    // $N (single digit by default; greedy digits still ok)
                    let mut j = i + 1;
                    let mut num = 0usize;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        num = num * 10 + (bytes[j] - b'0') as usize;
                        j += 1;
                    }
                    if let Some(m) = caps.get(num) {
                        out.push_str(m.as_str());
                    }
                    i = j;
                    continue;
                }
                _ => {
                    // $name
                    let mut j = i + 1;
                    while j < bytes.len() && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
                        j += 1;
                    }
                    if j > i + 1 {
                        let name = &replace[i + 1..j];
                        if let Some(m) = caps.name(name) {
                            out.push_str(m.as_str());
                        }
                        i = j;
                        continue;
                    }
                }
            }
        }
        // Default: copy one char.
        let ch_start = i;
        i += 1;
        while i < bytes.len() && !replace.is_char_boundary(i) {
            i += 1;
        }
        if i == ch_start {
            i += 1;
        }
        out.push_str(&replace[ch_start..i]);
    }
}

/// Escape a literal string so it can be embedded in a regex pattern.
/// Re-exported for handlers that build combined patterns.
pub fn escape_literal(s: &str) -> String {
    regex_escape(s)
}

/// Map of property keys that should be recognized as find/replace hints by handlers.
/// Mirrors the C# FindHelpers.FindReplace property keys so the same `--prop find=...`
/// invocation works across Word/Excel/PPT.
pub fn find_replace_property_keys() -> Vec<&'static str> {
    vec![
        "find",
        "replace",
        "replaceAll",
        "regex",
        "caseSensitive",
        "wholeWord",
    ]
}

/// Extract find/replace options from a property map.
pub fn extract_find_replace_props(
    properties: &HashMap<String, String>,
) -> Option<(String, String, FindReplaceOptions)> {
    let find = properties.get("find").cloned()?;
    let replace = properties
        .get("replace")
        .or_else(|| properties.get("replaceAll"))
        .cloned()
        .unwrap_or_default();

    let mut opts = FindReplaceOptions::default();
    if let Some(v) = properties.get("regex") {
        opts.use_regex = v == "true" || v == "1";
    }
    if let Some(v) = properties.get("caseSensitive") {
        opts.case_insensitive = !(v == "true" || v == "1");
    }
    if let Some(v) = properties.get("wholeWord") {
        opts.whole_word = v == "true" || v == "1";
    }

    Some((find, replace, opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_all_offsets_case_sensitive() {
        let text = "hello world hello WORLD";
        let offsets = find_all_offsets(text, "hello", &FindReplaceOptions::default());
        assert_eq!(offsets, vec![0, 12]);
    }

    #[test]
    fn test_find_all_offsets_case_insensitive() {
        let text = "hello world hello WORLD";
        let opts = FindReplaceOptions {
            case_insensitive: true,
            ..Default::default()
        };
        let offsets = find_all_offsets(text, "hello", &opts);
        assert_eq!(offsets, vec![0, 12]);
        let offsets = find_all_offsets(text, "world", &opts);
        assert_eq!(offsets, vec![6, 18]);
    }

    #[test]
    fn test_find_all_spans_literal_and_regex() {
        assert_eq!(
            find_all_spans("pre old post old", "old", &FindReplaceOptions::default()),
            vec![(4, 7), (13, 16)]
        );
        let regex = FindReplaceOptions {
            use_regex: true,
            ..Default::default()
        };
        assert_eq!(
            find_all_spans("a1b22", "[0-9]+", &regex),
            vec![(1, 2), (3, 5)]
        );
    }

    #[test]
    fn test_matches_text_filter_uses_csharp_literal_and_regex_vocabulary() {
        assert!(matches_text_filter("Quarterly BULLET review", "bullet").unwrap());
        assert!(matches_text_filter("Quarterly BULLET review", "r\"bull.t\"").unwrap());
        assert!(!matches_text_filter("Quarterly review", "r\"bull.t\"").unwrap());
        assert!(matches_text_filter("text", "r\"[\"").is_err());
    }

    #[test]
    fn test_find_all_replacements_expands_regex_captures() {
        let opts = FindReplaceOptions {
            use_regex: true,
            ..Default::default()
        };
        assert_eq!(
            find_all_replacements("item-42", r"(\w+)-(\d+)", "$2:$1", &opts).unwrap(),
            vec![(0, 7, "42:item".to_string())]
        );
    }

    #[test]
    fn test_replace_basic() {
        let (result, count) = replace_in_string("aXa Xa", "X", "Y", &FindReplaceOptions::default());
        assert_eq!(result, "aYa Ya");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_replace_whole_word() {
        let opts = FindReplaceOptions {
            whole_word: true,
            ..Default::default()
        };
        let (result, count) = replace_in_string("cat category cat", "cat", "dog", &opts);
        assert_eq!(result, "dog category dog");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_replace_max_replacements() {
        let opts = FindReplaceOptions {
            max_replacements: Some(1),
            ..Default::default()
        };
        let (result, count) = replace_in_string("aXbXcX", "X", "Y", &opts);
        assert_eq!(result, "aYbXcX");
        assert_eq!(count, 1);
    }

    #[test]
    fn test_replace_regex_basic() {
        let opts = FindReplaceOptions {
            use_regex: true,
            ..Default::default()
        };
        let (result, count) = replace_in_string("a1b22c333", "[0-9]+", "N", &opts);
        assert_eq!(result, "aNbNcN");
        assert_eq!(count, 3);
    }

    #[test]
    fn test_replace_regex_capture() {
        let opts = FindReplaceOptions {
            use_regex: true,
            ..Default::default()
        };
        let (result, count) = replace_in_string("foo bar", "(\\w+)", "[$1]", &opts);
        assert_eq!(result, "[foo] [bar]");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_replace_regex_case_insensitive() {
        let opts = FindReplaceOptions {
            use_regex: true,
            case_insensitive: true,
            ..Default::default()
        };
        let (result, count) = replace_in_string("HELLO hello HeLLo", "hello", "HI", &opts);
        assert_eq!(result, "HI HI HI");
        assert_eq!(count, 3);
    }

    #[test]
    fn test_replace_regex_whole_word() {
        let opts = FindReplaceOptions {
            use_regex: true,
            whole_word: true,
            ..Default::default()
        };
        let (result, count) = replace_in_string("cat category cat", "cat", "dog", &opts);
        assert_eq!(result, "dog category dog");
        assert_eq!(count, 2);
    }

    #[test]
    fn test_replace_regex_invalid_falls_back_to_original() {
        let opts = FindReplaceOptions {
            use_regex: true,
            ..Default::default()
        };
        let (result, count) = replace_in_string("abc", "[", "X", &opts);
        assert_eq!(result, "abc");
        assert_eq!(count, 0);
    }
}
