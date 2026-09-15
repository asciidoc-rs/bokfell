//! Excerpt matching (RFC 0001 §4, §10).
//!
//! Excerpts match a block's *source* text after whitespace normalization
//! only — inline markup stays intact, so excerpts are quoted from the
//! `.adoc` file and stay reviewable against it. Matching is exact after
//! normalization: fuzziness would silently heal spec drift.

/// Collapses every run of whitespace to one space and trims the ends —
/// the same collapse the search indexer applies to page text.
pub fn normalize_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Whether `excerpt` (any contiguous substring of one block) occurs in
/// `block_text`, both whitespace-normalized. An empty excerpt never
/// matches.
pub fn excerpt_matches(block_text: &str, excerpt: &str) -> bool {
    let excerpt = normalize_whitespace(excerpt);
    if excerpt.is_empty() {
        return false;
    }
    normalize_whitespace(block_text).contains(&excerpt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_whitespace_only() {
        assert_eq!(
            normalize_whitespace("  To nest an\n  ordered\tlist,  add "),
            "To nest an ordered list, add"
        );
        // Inline markup is untouched.
        assert_eq!(
            normalize_whitespace("a *strong* `code`"),
            "a *strong* `code`"
        );
    }

    #[test]
    fn matches_substrings_after_normalization() {
        let block = "To nest an ordered list, add a marker character for each level\nof nesting.";
        assert!(excerpt_matches(
            block,
            "add a marker character for each level of nesting"
        ));
        assert!(excerpt_matches(
            block,
            "add a marker character\n    for each level\n    of nesting."
        ));
        // Any contiguous substring matches, even one cut mid-word.
        assert!(excerpt_matches(block, "level of nest"));
        assert!(!excerpt_matches(
            block,
            "add a marker character for every level"
        ));
        assert!(!excerpt_matches(block, "add a  marker charactor"));
        assert!(!excerpt_matches(block, "   "));
    }
}
