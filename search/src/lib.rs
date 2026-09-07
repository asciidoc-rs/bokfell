//! Static search-index generation for the Bokfell documentation site
//! generator.
//!
//! PLAN.md §12 left the index format open (elasticlunr-compatible vs
//! Pagefind-style chunks vs custom); resolved at M7 in favor of a small
//! **custom JSON format**: one entry per page with its title, URL, and
//! extracted body text. The generator's own rendered HTML is well-formed,
//! so text extraction is a small tag stripper rather than a sanitizer
//! dependency, and the client (`_/bokfell-search.js` in the theme) scores
//! entries with plain token matching — no index library on either side.
//! Section-level entries with anchors are the planned refinement.

use serde::Serialize;

/// One searchable page.
#[derive(Debug, Serialize)]
pub struct SearchEntry {
    /// The page title (plain text).
    pub title: String,
    /// Site-root-relative URL.
    pub url: String,
    /// The page's visible text, whitespace-collapsed.
    pub text: String,
}

impl SearchEntry {
    /// Builds an entry from a rendered page's embedded HTML.
    pub fn from_html(title: &str, url: &str, contents: &str) -> Self {
        SearchEntry {
            title: title.to_string(),
            url: url.to_string(),
            text: html_to_text(contents),
        }
    }
}

/// Serializes the index the client script consumes, `<`-escaped so it
/// could be embedded in HTML too.
pub fn index_json(entries: &[SearchEntry]) -> String {
    serde_json::to_string(entries)
        .expect("entries serialize")
        .replace('<', "\\u003c")
}

/// Extracts visible text from the generator's own rendered HTML:
/// tags dropped, `<script>`/`<style>` *content* dropped too, entities
/// for the five XML escapes decoded, whitespace collapsed.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;

    while let Some(start) = rest.find('<') {
        push_text(&mut out, &rest[..start]);
        let tag_rest = &rest[start + 1..];
        let Some(end) = tag_rest.find('>') else {
            break;
        };
        let tag = &tag_rest[..end];
        rest = &tag_rest[end + 1..];

        // Raw-text elements: their content is not markup (a `<` inside a
        // script is code, not a tag — e.g. `if (a < b)` from a
        // passthrough block), so scan straight for the closing tag
        // instead of tokenizing tag by tag, and drop everything before
        // it. An unterminated element swallows the remainder, matching
        // browser behavior.
        let name = tag
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if name == "script" || name == "style" {
            let closer = if name == "script" {
                "</script"
            } else {
                "</style"
            };
            match find_closer(rest, closer) {
                Some(close_at) => {
                    let after = &rest[close_at..];
                    match after.find('>') {
                        Some(gt) => rest = &after[gt + 1..],
                        None => {
                            rest = "";
                        }
                    }
                }
                None => {
                    rest = "";
                }
            }
        }
    }
    push_text(&mut out, rest);

    out.trim().to_string()
}

/// Finds the raw-text element's real closing tag: `closer` (e.g.
/// `</script`) matched case-insensitively and followed by `>`, `/`, or
/// whitespace — so `</scriptfoo>` never terminates a `<script>` early.
fn find_closer(text: &str, closer: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(closer) {
        let candidate = from + at;
        let after = lower[candidate + closer.len()..].chars().next();
        match after {
            None | Some('>') | Some('/') => return Some(candidate),
            Some(c) if c.is_whitespace() => return Some(candidate),
            _ => from = candidate + closer.len(),
        }
    }
    None
}

/// Appends `text` with entities decoded and whitespace collapsed (one
/// space between words, and a space wherever tags separated text).
fn push_text(out: &mut String, text: &str) {
    let decoded = decode_entities(text);
    for word in decoded.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
}

/// Decodes the five XML escapes plus numeric character references
/// (`&#8217;`, `&#x2019;`) — the only entity forms the renderer emits.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let entity_rest = &rest[amp + 1..];
        let Some(semi) = entity_rest.find(';').filter(|s| *s <= 10) else {
            out.push('&');
            rest = entity_rest;
            continue;
        };
        let name = &entity_rest[..semi];
        let decoded = match name {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "amp" => Some('&'),
            _ => name
                .strip_prefix('#')
                .and_then(|num| {
                    num.strip_prefix('x')
                        .or_else(|| num.strip_prefix('X'))
                        .map_or_else(
                            || num.parse::<u32>().ok(),
                            |hex| u32::from_str_radix(hex, 16).ok(),
                        )
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &entity_rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = entity_rest;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_tags_and_collapses_whitespace() {
        let text = html_to_text(
            "<div class=\"paragraph\">\n<p>One   two\nthree.</p>\n</div>\n\
             <pre>let x = 1;</pre>",
        );
        assert_eq!(text, "One two three. let x = 1;");
    }

    #[test]
    fn drops_script_and_style_content() {
        let text = html_to_text(
            "<p>Before.</p><script>var hidden = true;</script>\
             <style>.x{color:red}</style><p>After.</p>",
        );
        assert_eq!(text, "Before. After.");
    }

    #[test]
    fn script_content_with_comparisons_never_swallows_following_text() {
        // A `<` inside script code is not a tag; the text after the
        // script must survive.
        let text =
            html_to_text("<p>Before.</p><script>if (a < b) run(x > y);</script><p>After.</p>");
        assert_eq!(text, "Before. After.");

        // Case-insensitive closer, attributes on the opener.
        let text =
            html_to_text("<p>A.</p><SCRIPT type=\"text/javascript\">1 < 2</SCRIPT><p>B.</p>");
        assert_eq!(text, "A. B.");

        // An unterminated script swallows the rest (browser behavior),
        // never emitting code as searchable text.
        let text = html_to_text("<p>A.</p><script>1 < 2");
        assert_eq!(text, "A.");

        // A closer-shaped prefix inside the code (or a bogus element
        // name) never terminates the raw text early.
        let text = html_to_text("<p>A.</p><script>var s = \"</scriptx>\";</script><p>B.</p>");
        assert_eq!(text, "A. B.");
        let text = html_to_text("<p>A.</p><script>x</script\t>\n<p>B.</p>");
        assert_eq!(text, "A. B.");
    }

    #[test]
    fn decodes_the_basic_entities() {
        let text = html_to_text("<p>a &lt;b&gt; &amp; &quot;c&quot;</p>");
        assert_eq!(text, "a <b> & \"c\"");
    }

    #[test]
    fn decodes_numeric_references() {
        let text = html_to_text("<p>Asciidoctor&#8217;s em&#x2014;dash &bogus; ok</p>");
        assert_eq!(text, "Asciidoctor\u{2019}s em\u{2014}dash &bogus; ok");
    }

    #[test]
    fn index_json_escapes_angle_brackets() {
        let entries = vec![SearchEntry::from_html(
            "T",
            "a/b.html",
            "<p>x &lt;y&gt;</p>",
        )];
        let json = index_json(&entries);
        assert!(!json.contains('<'), "json: {json}");
        assert!(json.contains("a/b.html"));
    }
}
