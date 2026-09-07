//! Enumerating a page's overlay blocks for the coverage feature.
//!
//! The coverage overlay (PLAN.md §9.2) shades *rendered* blocks by the
//! status of their source lines. The rendered HTML carries no source
//! anchors yet (upstream ask:
//! <https://github.com/asciidoc-rs/asciidoc-html5/issues/339>), so server
//! and client agree on a **document-order walk over the same block set**:
//! this module enumerates it from the AST, and the theme's client script
//! collects the matching container elements — outermost only, exactly
//! mirroring the "don't descend into emitted blocks" rule here. The
//! client verifies the counts match and disables the overlay for the page
//! otherwise, so a mismatch degrades gracefully instead of shading the
//! wrong blocks.

use asciidoc_parser::{
    blocks::{Block, FindBlocks, IsBlock},
    Document, HasSpan,
};
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
/// preprocessed source the parser saw), plus the [`BlockUnit`] the diff
/// engine aligns on (resolved context + span text).
#[derive(Clone, Debug)]
pub struct OverlayBlock {
    /// 1-based start line.
    pub start_line: u32,
    /// Number of source lines the block's span covers (at least 1).
    pub line_count: u32,
    /// The block as a diffable unit.
    pub unit: BlockUnit,
}

/// Enumerates the overlay blocks of a document in document order:
/// sections and the preamble are descended through (they are structure,
/// not content), every overlay-context block is emitted whole, and
/// nothing inside an emitted block is walked — matching the client's
/// outermost-only element collection.
pub fn overlay_blocks<'src>(document: &'src Document<'src>) -> Vec<OverlayBlock> {
    let mut out = Vec::new();
    walk(document.child_blocks(), &mut out);
    out
}

/// Just the diffable units of [`overlay_blocks`], in the same order.
pub fn overlay_units<'src>(document: &'src Document<'src>) -> Vec<BlockUnit> {
    overlay_blocks(document)
        .into_iter()
        .map(|block| block.unit)
        .collect()
}

fn push_block(out: &mut Vec<OverlayBlock>, kind: &str, span: &asciidoc_parser::Span<'_>) {
    out.push(OverlayBlock {
        start_line: span.line() as u32,
        line_count: span.data().lines().count().max(1) as u32,
        unit: BlockUnit {
            kind: kind.to_string(),
            text: span.data().to_string(),
        },
    });
}

fn walk<'src>(blocks: impl Iterator<Item = &'src Block<'src>>, out: &mut Vec<OverlayBlock>) {
    for block in blocks {
        match block {
            Block::Section(section) => walk(section.child_blocks(), out),
            Block::Preamble(preamble) => walk(preamble.child_blocks(), out),
            // Every list kind renders one container element (`.ulist`,
            // `.olist`, `.dlist`, `.colist` — all in the selector table),
            // but the AST context is the generic `list`, so lists are
            // matched structurally. Items are never descended into.
            Block::List(list) => push_block(out, "list", &list.span()),
            other => {
                let context = other.resolved_context();
                if OVERLAY_CONTEXTS
                    .iter()
                    .any(|(token, _)| *token == context.as_ref())
                {
                    push_block(out, context.as_ref(), &other.span());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn selector_is_nonempty_and_comma_joined() {
        let selector = client_selector();
        assert!(selector.contains(".paragraph"));
        assert!(selector.contains("table.tableblock"));
        assert!(selector.split(',').count() == super::OVERLAY_CONTEXTS.len());
    }
}
