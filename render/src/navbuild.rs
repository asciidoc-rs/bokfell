//! Building navigation trees from `nav.adoc` files.
//!
//! A nav file is plain AsciiDoc whose top-level unordered lists become the
//! component's menu. Antora converts the file and scrapes the resulting
//! HTML; with a real AST we walk the parsed lists directly: each item's
//! principal text carries the xref (or plain text) that becomes the entry.

use asciidoc_parser::{
    blocks::{Block, FindBlocks, IsBlock, ListItem},
    inlines::{InlineNode, RefVariant},
    Document,
};
use bokfell_model::{ContentCatalog, Coords, Family, NavItem, NavTree, ResourceRef};

use crate::{inline_text::inline_text, resolver::SiteIndex};

/// Appends the top-level lists of one parsed nav file to `tree`.
pub(crate) fn extend_tree<'src>(
    tree: &mut NavTree,
    document: &'src Document<'src>,
    catalog: &ContentCatalog,
    index: &SiteIndex,
    nav_coords: &Coords,
) {
    for block in document.child_blocks() {
        if let Block::List(list) = block {
            for child in list.child_blocks() {
                if let Block::ListItem(item) = child {
                    if let Some(nav_item) = nav_item(item, catalog, index, nav_coords) {
                        tree.items.push(nav_item);
                    }
                }
            }
        }
    }
}

fn nav_item<'src>(
    item: &'src ListItem<'src>,
    catalog: &ContentCatalog,
    index: &SiteIndex,
    nav_coords: &Coords,
) -> Option<NavItem> {
    let mut entry = NavItem::default();

    for child in item.child_blocks() {
        match child {
            // The first simple block is the item's principal text.
            Block::Simple(simple) if entry.html.is_empty() => {
                if let Some(inlines) = simple.inlines() {
                    fill_from_inlines(&mut entry, inlines, catalog, index, nav_coords);
                }
                if let Some(html) = simple.rendered_html_content() {
                    entry.html = html.to_string();
                }
            }

            // Nested lists are the item's children.
            Block::List(list) => {
                for nested in list.child_blocks() {
                    if let Block::ListItem(nested_item) = nested {
                        if let Some(child_entry) = nav_item(nested_item, catalog, index, nav_coords)
                        {
                            entry.children.push(child_entry);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if entry.html.is_empty() && entry.text.is_empty() && entry.children.is_empty() {
        return None;
    }
    Some(entry)
}

/// Fills an entry's text and destination from the principal's inline nodes:
/// the first cross-reference (or link) wins; otherwise the entry is an
/// unlinked category label.
fn fill_from_inlines(
    entry: &mut NavItem,
    inlines: &[InlineNode<'_>],
    catalog: &ContentCatalog,
    index: &SiteIndex,
    nav_coords: &Coords,
) {
    entry.text = inline_text(inlines).trim().to_string();

    for node in inlines {
        if let InlineNode::Ref(reference) = node {
            match reference.variant {
                RefVariant::Xref => {
                    let target = reference.target.as_ref();
                    let path_part = target.split_once('#').map_or(target, |(p, _)| p);
                    let file = ResourceRef::parse(path_part)
                        .and_then(|r| catalog.resolve(&r, nav_coords, Family::Page));
                    if let Some(file) = file {
                        entry.url = file.url.clone();

                        // An xref with no supplied text takes the target
                        // page's title.
                        if reference.children.is_empty() {
                            if let Some(info) = index.get(&file.coords) {
                                if let Some(label) = &info.nav_text {
                                    entry.text = label.clone();
                                }
                            }
                        }
                    }
                }
                RefVariant::Link => {
                    entry.url = Some(reference.target.to_string());
                }
            }
            break;
        }
    }
}
