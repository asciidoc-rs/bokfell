//! Enumerating a page's overlay blocks — the unit of coverage, diffing,
//! and click-to-source.
//!
//! The overlays (PLAN.md §9.1–§9.3) shade *rendered* blocks. Server and
//! client agree on a **document-order walk over the same block set**:
//! this module enumerates it from the AST, and the theme's client script
//! anchors each block by its `data-source-line` (asciidoc-html5 0.2.2)
//! or, as a fallback, collects the matching container elements —
//! outermost only, exactly mirroring the "don't descend into emitted
//! blocks" rule here. The client verifies the counts match and disables
//! the overlay for the page otherwise, so a mismatch degrades gracefully
//! instead of shading the wrong blocks.
//!
//! The same walk supplies the spec-coverage engine's blocks (RFC 0001
//! §2): each block carries its enclosing section IDs (for `#anchor`
//! scopes) and the structural heuristic's classification (§5).

use asciidoc_parser::{
    blocks::{Block, FindBlocks, IsBlock},
    Document, HasSpan,
};
use bokfell_coverage::StructuralKind;
use bokfell_diff::BlockUnit;

/// The block contexts the overlay pairs on, with the CSS selector the
/// client uses for each. Kept as one table so the two walks cannot drift
/// independently; the theme embeds [`client_selector`] from it.
const OVERLAY_CONTEXTS: &[(&str, &str)] = &[
    ("admonition", ".admonitionblock"),
    ("audio", ".audioblock"),
    ("colist", ".colist"),
    ("dlist", ".dlist"),
    // A description list styled `horizontal` or `qanda` renders as
    // `.hdlist`/`.qlist` instead of `.dlist` (the AST context is the
    // same); all three selectors keep the client's element count in step.
    ("dlist", ".hdlist"),
    ("dlist", ".qlist"),
    ("example", ".exampleblock"),
    ("image", ".imageblock"),
    ("listing", ".listingblock"),
    ("literal", ".literalblock"),
    ("olist", ".olist"),
    ("open", ".openblock"),
    ("paragraph", ".paragraph"),
    ("quote", ".quoteblock"),
    ("sidebar", ".sidebarblock"),
    ("stem", ".stemblock"),
    ("table", "table.tableblock"),
    ("ulist", ".ulist"),
    ("verse", ".verseblock"),
    ("video", ".videoblock"),
];

/// The combined CSS selector for the client-side walk.
pub fn client_selector() -> String {
    OVERLAY_CONTEXTS
        .iter()
        .map(|(_, sel)| *sel)
        .collect::<Vec<_>>()
        .join(",")
}

/// One overlay block: its source start line and line count (in the
/// preprocessed source the parser saw), the [`BlockUnit`] the diff
/// engine aligns on (resolved context + span text), and what the
/// coverage engine needs to classify and target it.
#[derive(Clone, Debug)]
pub struct OverlayBlock {
    /// 1-based start line.
    pub start_line: u32,
    /// Number of source lines the block's span covers (at least 1).
    pub line_count: u32,
    /// The block as a diffable unit.
    pub unit: BlockUnit,
    /// The IDs of the sections enclosing the block, outermost first.
    pub sections: Vec<String>,
    /// The structural heuristic's coverage classification (RFC 0001 §5).
    pub kind: StructuralKind,
}

/// Enumerates the overlay blocks of a document in document order:
/// sections and the preamble are descended through (they are structure,
/// not content), every overlay-context block is emitted whole, and
/// nothing inside an emitted block is walked — matching the client's
/// outermost-only element collection.
pub fn overlay_blocks<'src>(document: &'src Document<'src>) -> Vec<OverlayBlock> {
    let mut out = Vec::new();
    let mut sections = Vec::new();
    walk(document.child_blocks(), &mut sections, &mut out);
    out
}

/// Just the diffable units of [`overlay_blocks`], in the same order.
pub fn overlay_units<'src>(document: &'src Document<'src>) -> Vec<BlockUnit> {
    overlay_blocks(document)
        .into_iter()
        .map(|block| block.unit)
        .collect()
}

/// Every section ID of a document, in document order.
pub fn section_ids<'src>(document: &'src Document<'src>) -> Vec<String> {
    fn collect<'src>(blocks: impl Iterator<Item = &'src Block<'src>>, out: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Section(section) => {
                    if let Some(id) = section.id() {
                        out.push(id.to_string());
                    }
                    collect(section.child_blocks(), out);
                }
                Block::Preamble(preamble) => collect(preamble.child_blocks(), out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    collect(document.child_blocks(), &mut out);
    out
}

/// The structural default for a block context (RFC 0001 §5): example and
/// listing blocks, images, and media illustrate rather than specify;
/// prose paragraphs, admonitions, tables, lists, and quotations may
/// carry rules.
fn structural_kind(context: &str) -> StructuralKind {
    match context {
        "example" | "listing" | "literal" | "image" | "audio" | "video" | "stem" => {
            StructuralKind::NonNormative
        }
        _ => StructuralKind::Prose,
    }
}

fn push_block(
    out: &mut Vec<OverlayBlock>,
    kind: &str,
    span: &asciidoc_parser::Span<'_>,
    sections: &[String],
) {
    out.push(OverlayBlock {
        start_line: span.line() as u32,
        line_count: span.data().lines().count().max(1) as u32,
        unit: BlockUnit {
            kind: kind.to_string(),
            text: span.data().to_string(),
        },
        sections: sections.to_vec(),
        kind: structural_kind(kind),
    });
}

fn walk<'src>(
    blocks: impl Iterator<Item = &'src Block<'src>>,
    sections: &mut Vec<String>,
    out: &mut Vec<OverlayBlock>,
) {
    for block in blocks {
        match block {
            Block::Section(section) => {
                let pushed = section.id().map(|id| sections.push(id.to_string()));
                walk(section.child_blocks(), sections, out);
                if pushed.is_some() {
                    sections.pop();
                }
            }
            Block::Preamble(preamble) => walk(preamble.child_blocks(), sections, out),
            // Every list kind renders one container element (`.ulist`,
            // `.olist`, `.dlist`, `.colist` — all in the selector table),
            // but the AST context is the generic `list`, so lists are
            // matched structurally. Items are never descended into.
            Block::List(list) => push_block(out, "list", &list.span(), sections),
            other => {
                let context = other.resolved_context();
                if OVERLAY_CONTEXTS
                    .iter()
                    .any(|(token, _)| *token == context.as_ref())
                {
                    push_block(out, context.as_ref(), &other.span(), sections);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-op claim marker (RFC 0001 §4): this crate's tests claim the
    /// RFC blocks they exercise.
    macro_rules! verifies {
        ($($tt:tt)*) => {};
    }

    #[test]
    fn enumerates_content_blocks_outermost_only() {
        let document = asciidoc_html5::load(
            "= Title\n\
             \n\
             Preamble paragraph.\n\
             \n\
             == Section\n\
             \n\
             A paragraph.\n\
             \n\
             ====\n\
             Inside an example block.\n\
             ====\n\
             \n\
             * one\n\
             * two\n\
             \n\
             '''\n\
             \n\
             Last.\n",
        );

        let blocks = overlay_blocks(&document);

        // Preamble paragraph, section paragraph, example block (its inner
        // paragraph is NOT emitted), the ulist, and the last paragraph;
        // the thematic break is not an overlay context.
        assert_eq!(blocks.len(), 5, "blocks: {blocks:?}");
        assert_eq!(blocks[0].start_line, 3);
        assert_eq!(blocks[2].line_count, 3);
        assert_eq!(blocks[3].start_line, 13);
        assert_eq!(blocks[3].line_count, 2);
        assert_eq!(blocks[4].start_line, 18);
    }

    #[test]
    fn records_sections_and_structural_kinds() {
        verifies!(
            "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
            "example and listing blocks, block titles, images, and nav are non-normative by default; prose paragraphs, admonitions, and tables default to `unclassified`"
        );
        verifies!(
            "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
            "Bokfell parses every spec page with source maps already; blocks are what the overlay UI shades, what excerpts resolve to, and what the states attach to."
        );
        let document = asciidoc_html5::load(
            "= Title\n\
             \n\
             Preamble.\n\
             \n\
             == Nesting\n\
             \n\
             A rule.\n\
             \n\
             ----\n\
             listing\n\
             ----\n\
             \n\
             [[custom]]\n\
             === Inner\n\
             \n\
             NOTE: An admonition.\n\
             \n\
             == Notes\n\
             \n\
             |===\n\
             | cell\n\
             |===\n",
        );

        let blocks = overlay_blocks(&document);
        assert_eq!(blocks.len(), 5, "blocks: {blocks:?}");
        assert!(blocks[0].sections.is_empty());
        assert_eq!(blocks[1].sections, ["_nesting"]);
        assert_eq!(blocks[1].kind, StructuralKind::Prose);
        assert_eq!(blocks[2].unit.kind, "listing");
        assert_eq!(blocks[2].kind, StructuralKind::NonNormative);
        assert_eq!(blocks[3].sections, ["_nesting", "custom"]);
        assert_eq!(blocks[3].unit.kind, "admonition");
        assert_eq!(blocks[4].sections, ["_notes"]);
        assert_eq!(blocks[4].unit.kind, "table");
        assert_eq!(blocks[4].kind, StructuralKind::Prose);
        for context in ["example", "image", "audio", "video", "literal"] {
            assert_eq!(
                structural_kind(context),
                StructuralKind::NonNormative,
                "{context}"
            );
        }
        for context in ["paragraph", "admonition", "table", "list", "quote", "open"] {
            assert_eq!(structural_kind(context), StructuralKind::Prose, "{context}");
        }

        assert_eq!(section_ids(&document), ["_nesting", "custom", "_notes"]);
    }

    #[test]
    fn selector_is_nonempty_and_comma_joined() {
        let selector = client_selector();
        assert!(selector.contains(".paragraph"));
        assert!(selector.contains("table.tableblock"));
        assert!(selector.split(',').count() == super::OVERLAY_CONTEXTS.len());
    }
}
