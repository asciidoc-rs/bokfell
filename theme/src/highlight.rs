//! Build-time syntax highlighting for the test sources the coverage
//! overlay inlines (PLAN.md §7: `syntect`, no client-side highlighter).
//!
//! Output is class-based HTML (`<span class="keyword …">`) so the
//! stylesheet's token palette follows the light/dark scheme, and it is
//! split per source line so the overlay can number lines and mark the
//! one carrying a claim.

use std::sync::OnceLock;

use syntect::{
    html::{ClassStyle, ClassedHTMLGenerator},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Highlights Rust source as one HTML fragment per line (no trailing
/// newlines), each fragment self-contained: spans the grammar opened on
/// an earlier line are re-opened on the next. Source text is escaped;
/// the fragments carry only `<span class="…">` markup. Falls back to
/// escaped plain text if highlighting fails.
pub fn highlight_rust_lines(source: &str) -> Vec<String> {
    let syntaxes = syntaxes();
    let Some(syntax) = syntaxes.find_syntax_by_extension("rs") else {
        return plain_lines(source);
    };
    let mut generator =
        ClassedHTMLGenerator::new_with_class_style(syntax, syntaxes, ClassStyle::Spaced);
    for line in LinesWithEndings::from(source) {
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return plain_lines(source);
        }
    }
    let html = generator.finalize();
    let lines = split_lines(&html);

    // The generator's output covers exactly the source's lines; anything
    // else means the split lost track, so fall back rather than mislabel.
    let expected = source.lines().count();
    if lines.len() == expected {
        lines
    } else {
        plain_lines(source)
    }
}

fn plain_lines(source: &str) -> Vec<String> {
    source.lines().map(escape).collect()
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// The scope families the stylesheet's token palette styles. Spans of
/// any other scope (the grammar's `meta.*` structure and `punctuation`
/// wrappers, mostly) carry no styling and are dropped, which shrinks
/// the output several-fold.
const STYLED: [&str; 8] = [
    "comment", "keyword", "storage", "string", "constant", "entity", "support", "variable",
];

/// Rewrites one of the generator's opening tags for the palette: `None`
/// for a span the stylesheet never styles, else the tag with the
/// language suffix (`rust`) dropped from its class list.
fn keep_tag(tag: &str) -> Option<String> {
    let classes = tag
        .strip_prefix("<span class=\"")
        .and_then(|rest| rest.strip_suffix("\">"))?;
    let mut tokens: Vec<&str> = classes.split(' ').collect();
    if tokens.last() == Some(&"rust") {
        tokens.pop();
    }
    let first = tokens.first()?;
    STYLED
        .contains(first)
        .then(|| format!("<span class=\"{}\">", tokens.join(" ")))
}

/// Splits highlighted HTML at newlines into self-contained fragments:
/// every kept span open at a newline is closed there and re-opened
/// (same class) at the start of the next line; unstyled spans are
/// unwrapped. The input holds only `<span class="…">`/`</span>` tags
/// and escaped text.
fn split_lines(html: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    // Every open span, kept (its rewritten tag) or dropped (`None`), so
    // each `</span>` closes the right one.
    let mut open: Vec<Option<String>> = Vec::new();
    let mut rest = html;

    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("</span>") {
            if open.pop().flatten().is_some() {
                current.push_str("</span>");
            }
            rest = stripped;
        } else if rest.starts_with("<span") {
            let end = rest.find('>').map_or(rest.len(), |i| i + 1);
            let kept = keep_tag(&rest[..end]);
            if let Some(tag) = &kept {
                current.push_str(tag);
            }
            open.push(kept);
            rest = &rest[end..];
        } else if let Some(stripped) = rest.strip_prefix('\n') {
            for _ in open.iter().flatten() {
                current.push_str("</span>");
            }
            lines.push(std::mem::take(&mut current));
            for tag in open.iter().flatten() {
                current.push_str(tag);
            }
            rest = stripped;
        } else {
            let next = rest.find(['<', '\n']).unwrap_or(rest.len()).max(1);
            current.push_str(&rest[..next]);
            rest = &rest[next..];
        }
    }

    // A final line without a trailing newline — or, after a trailing
    // newline, only the reopened spans, which hold no text and are
    // dropped.
    if !only_tags(&current) {
        for _ in open.iter().flatten() {
            current.push_str("</span>");
        }
        lines.push(current);
    }
    lines
}

/// Whether a fragment holds nothing but tags (no text at all).
fn only_tags(fragment: &str) -> bool {
    let mut rest = fragment;
    while let Some(start) = rest.find('<') {
        if !rest[..start].is_empty() {
            return false;
        }
        match rest[start..].find('>') {
            Some(end) => rest = &rest[start + end + 1..],
            None => return false,
        }
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_rust_per_line_with_balanced_spans() {
        let lines = highlight_rust_lines(
            "#[test]\nfn t() {\n    verifies!(\"p.adoc\", r#\"multi\nline\"#);\n    let n = 42; // <c>\n}",
        );
        assert_eq!(lines.len(), 6, "{lines:#?}");
        assert!(
            lines[0].contains("class=\"variable annotation\">test<"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains("storage type function\">fn<"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].contains("support macro\">verifies!<"),
            "{}",
            lines[2]
        );
        assert!(lines[4].contains("constant numeric integer decimal\">42<"));

        // Unstyled structure spans are unwrapped, not just relabeled.
        assert!(!lines[1].contains("meta function"), "{}", lines[1]);
        assert!(!lines[2].contains("punctuation"), "{}", lines[2]);
        assert!(lines[4].contains("&lt;c&gt;"), "{}", lines[4]);

        // Every line is self-contained: as many closes as opens.
        for line in &lines {
            assert_eq!(
                line.matches("<span").count(),
                line.matches("</span>").count(),
                "{line}"
            );
            assert!(!line.contains('\n'));
        }

        // The raw string that spans lines 3–4 keeps its class on line 4.
        assert!(
            lines[3].contains("string quoted double raw\""),
            "{}",
            lines[3]
        );
    }

    #[test]
    fn split_reopens_spans_and_keeps_text() {
        let lines = split_lines(
            "<span class=\"meta rust\"><span class=\"string rust\">x\ny</span>\nz</span>",
        );
        assert_eq!(
            lines,
            [
                "<span class=\"string\">x</span>",
                "<span class=\"string\">y</span>",
                "z"
            ]
        );
        assert_eq!(split_lines("only\n"), ["only"]);
        assert_eq!(split_lines(""), Vec::<String>::new());
    }
}
