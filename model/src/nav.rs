//! The navigation model: ordered trees of links, one per component
//! version, built from the `nav.adoc` files registered in `antora.yml`.
//!
//! The *data* lives here; *building* a tree from a parsed nav file is the
//! render stage's job, since it needs the AsciiDoc parser.

/// One navigation entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NavItem {
    /// The entry's inline content as rendered HTML (link or plain text).
    /// For linked entries this is the full `<a …>` element.
    pub html: String,

    /// The plain-text label (link text or the text content).
    pub text: String,

    /// The destination as a site-root-relative URL, when the entry links
    /// to a page in the site.
    pub url: Option<String>,

    /// Child entries.
    pub children: Vec<NavItem>,
}

/// The navigation tree of one component version: the concatenation of its
/// registered nav files' top-level lists, in registration order.
#[derive(Clone, Debug, Default)]
pub struct NavTree {
    /// The top-level entries.
    pub items: Vec<NavItem>,
}

impl NavTree {
    /// Depth-first iteration over all entries.
    pub fn walk(&self) -> Vec<&NavItem> {
        fn push<'a>(item: &'a NavItem, out: &mut Vec<&'a NavItem>) {
            out.push(item);
            for child in &item.children {
                push(child, out);
            }
        }
        let mut out = Vec::new();
        for item in &self.items {
            push(item, &mut out);
        }
        out
    }
}
