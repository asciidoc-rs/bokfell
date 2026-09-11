//! Static search-index generation for the Bokfell documentation site
//! generator.
//!
//! PLAN.md §12 left the index format open (elasticlunr-compatible vs
//! Pagefind-style chunks vs custom); resolved at M7 in favor of a small
//! **custom JSON format**: entries with a title, URL, and extracted body
//! text. The generator's own rendered HTML is well-formed,
//! so text extraction is a small tag stripper rather than a sanitizer
//! dependency, and the client (`_/bokfell-search.js` in the theme) scores
//! entries with plain token matching — no index library on either side.
//!
//! Entries are **section-level**: each page yields a lead entry (the page
//! title and any text before its first section) plus one entry per
//! anchored section heading, whose URL carries the `#fragment` — so a
//! result lands the reader on the matching section, not just the page.

use serde::Serialize;

/// One searchable page or page section.
#[derive(Debug, Serialize)]
pub struct SearchEntry {
    /// The page or section title (plain text).
    pub title: String,
    /// Site-root-relative URL, with a `#fragment` for section entries.
    pub url: String,
    /// The entry's visible text, whitespace-collapsed.
    pub text: String,
    /// For a section entry, the title of the page it belongs to; `None`
    /// (and omitted from the JSON) for a page's lead entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

impl SearchEntry {
    /// Builds a single whole-page entry from a rendered page's embedded
    /// HTML. [`page_entries`] is the section-aware sibling the site
    /// build uses.
    pub fn from_html(title: &str, url: &str, contents: &str) -> Self {
        SearchEntry {
            title: title.to_string(),
            url: url.to_string(),
            text: html_to_text(contents),
            page: None,
        }
    }
}

/// Builds the index entries for one page: a lead entry (page title, text
/// before the first section heading) plus one entry per `<h2>`–`<h6>`
/// heading that carries an `id`, anchored with `#id` and holding the
/// text up to the next such heading. A heading without an `id` cannot be
/// linked, so its text merges into the enclosing entry.
pub fn page_entries(title: &str, url: &str, contents: &str) -> Vec<SearchEntry> {
    split_sections(contents)
        .into_iter()
        .map(|section| match section.heading {
            None => SearchEntry::from_html(title, url, &section.body_html),
            Some((anchor, title_html)) => SearchEntry {
                title: html_to_text(&title_html),
                url: format!("{url}#{anchor}"),
                text: html_to_text(&section.body_html),
                page: Some(title.to_string()),
            },
        })
        .collect()
}

/// One linear slice of a page: the lead (no heading) or an anchored
/// section, holding raw HTML for [`page_entries`] to extract text from.
struct Section {
    /// `(anchor, heading inner HTML)`; `None` for the lead slice.
    heading: Option<(String, String)>,
    body_html: String,
}

/// Splits rendered page HTML linearly at each `<h2>`–`<h6>` heading with
/// an `id`. The split is linear rather than tree-shaped on purpose: a
/// subsection's heading ends its parent's slice, so no text lands in two
/// entries. Raw-text elements are skipped with the same rules as
/// [`html_to_text`], so a `<h2` inside script code never splits.
fn split_sections(html: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut heading: Option<(String, String)> = None;
    let mut chunk_start = 0;
    let mut pos = 0;

    while let Some(lt) = html[pos..].find('<') {
        let tag_start = pos + lt;
        let tag_rest = &html[tag_start + 1..];
        let Some(gt) = tag_rest.find('>') else {
            break;
        };
        let tag = &tag_rest[..gt];
        let after_tag = tag_start + 1 + gt + 1;
        pos = after_tag;

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
            match find_closer(&html[after_tag..], closer) {
                Some(close_at) => {
                    let after = after_tag + close_at;
                    match html[after..].find('>') {
                        Some(close_gt) => pos = after + close_gt + 1,
                        None => pos = html.len(),
                    }
                }
                None => pos = html.len(),
            }
            continue;
        }

        if !is_section_heading(&name) {
            continue;
        }
        let Some(id) = id_attr(tag) else {
            continue;
        };

        // Close the running slice at the heading, then lift the heading's
        // inner HTML out as the new slice's title.
        let closer_owned = format!("</{name}");
        let Some(close_at) = find_closer(&html[after_tag..], &closer_owned) else {
            continue;
        };
        let inner = &html[after_tag..after_tag + close_at];
        let heading_end = match html[after_tag + close_at..].find('>') {
            Some(close_gt) => after_tag + close_at + close_gt + 1,
            None => html.len(),
        };

        sections.push(Section {
            heading: heading.take(),
            body_html: html[chunk_start..tag_start].to_string(),
        });
        heading = Some((id.to_string(), inner.to_string()));
        chunk_start = heading_end;
        pos = heading_end;
    }

    sections.push(Section {
        heading,
        body_html: html[chunk_start..].to_string(),
    });
    sections
}

/// Whether a lowercased tag name is a sectioning heading (`h2`–`h6`;
/// `h1` is the page title, which the layout renders outside the page
/// contents).
fn is_section_heading(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next() == Some('h') && matches!(chars.next(), Some('2'..='6')) && chars.next().is_none()
}

/// Extracts the `id="…"` attribute value from a tag's inside (the text
/// between `<` and `>`), requiring whitespace before `id=` so a
/// `data-id="…"` never matches.
fn id_attr(tag: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(at) = tag[from..].find("id=\"") {
        let abs = from + at;
        if tag[..abs].ends_with(|c: char| c.is_whitespace()) {
            let value = &tag[abs + 4..];
            return value.find('"').map(|quote| &value[..quote]);
        }
        from = abs + 4;
    }
    None
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
    fn splits_a_page_into_lead_and_anchored_section_entries() {
        let entries = page_entries(
            "Widths",
            "html5/widths.html",
            "<div class=\"paragraph\"><p>Preamble text.</p></div>\
             <div class=\"sect1\"><h2 id=\"_first\">First</h2>\
             <div class=\"sectionbody\"><p>Alpha.</p></div></div>\
             <div class=\"sect1\"><h2 id=\"_second\">Second</h2>\
             <div class=\"sectionbody\"><p>Beta.</p></div></div>",
        );
        let got: Vec<(&str, &str, &str, Option<&str>)> = entries
            .iter()
            .map(|e| {
                (
                    e.title.as_str(),
                    e.url.as_str(),
                    e.text.as_str(),
                    e.page.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("Widths", "html5/widths.html", "Preamble text.", None),
                (
                    "First",
                    "html5/widths.html#_first",
                    "Alpha.",
                    Some("Widths")
                ),
                (
                    "Second",
                    "html5/widths.html#_second",
                    "Beta.",
                    Some("Widths")
                ),
            ]
        );
    }

    #[test]
    fn subsection_heading_ends_its_parent_slice() {
        // The split is linear: an h3 closes the h2's entry, so no text
        // appears in two entries.
        let entries = page_entries(
            "P",
            "p.html",
            "<h2 id=\"_outer\">Outer</h2><p>Outer text.</p>\
             <h3 id=\"_inner\">Inner</h3><p>Inner text.</p>",
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].text, "Outer text.");
        assert_eq!(entries[2].url, "p.html#_inner");
        assert_eq!(entries[2].text, "Inner text.");
    }

    #[test]
    fn heading_markup_is_stripped_from_the_entry_title() {
        let entries = page_entries(
            "P",
            "p.html",
            "<h2 id=\"_code\">Using <code>adoc &amp; co</code></h2><p>Body.</p>",
        );
        assert_eq!(entries[1].title, "Using adoc & co");
    }

    #[test]
    fn heading_without_an_id_merges_into_the_enclosing_entry() {
        let entries = page_entries(
            "P",
            "p.html",
            "<p>Lead.</p><h2 class=\"x\">Unanchored</h2><p>More lead.</p>\
             <h2 id=\"_real\">Real</h2><p>Body.</p>",
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "Lead. Unanchored More lead.");
        assert_eq!(entries[1].title, "Real");
    }

    #[test]
    fn data_id_attribute_is_not_an_anchor() {
        let entries = page_entries("P", "p.html", "<h2 data-id=\"_x\">No anchor</h2><p>A.</p>");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "No anchor A.");
    }

    #[test]
    fn heading_inside_script_code_never_splits() {
        let entries = page_entries(
            "P",
            "p.html",
            "<p>A.</p><script>render('<h2 id=\"_fake\">');</script>\
             <h2 id=\"_real\">Real</h2><p>B.</p>",
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "A.");
        assert_eq!(entries[1].url, "p.html#_real");
        assert_eq!(entries[1].text, "B.");
    }

    #[test]
    fn section_entries_serialize_page_and_lead_entries_omit_it() {
        let entries = page_entries("P", "p.html", "<p>Lead.</p><h2 id=\"_s\">S</h2><p>B.</p>");
        let json = index_json(&entries);
        assert!(json.contains("\"page\":\"P\""), "json: {json}");
        assert!(
            !json[..json.find("_s").unwrap()].contains("\"page\""),
            "lead entry must omit page: {json}"
        );
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
