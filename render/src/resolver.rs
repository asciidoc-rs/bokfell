//! Site-wide cross-reference resolution.
//!
//! The parser resolves a single document's references against its own
//! catalog; this module supplies the multi-document half: a snapshot index
//! of every page's referenceable elements, and a [`ReferenceResolver`] that
//! resolves in-page targets like the parser's own `CatalogResolver` and
//! path-bearing targets (`page.adoc#frag`, `module:page.adoc`, …) through
//! the content catalog to relative URLs.

use std::collections::HashMap;

use asciidoc_parser::{
    document::{Document, InterpretedValue, RefEntry},
    parser::{ReferenceResolver, ResolutionContext, ResolvedReference},
};
use bokfell_model::{relative_url, ContentCatalog, Coords, Family, ResourceRef};

/// What the site knows about one rendered page, snapshotted after its
/// deferred parse (the document itself is mutably borrowed during
/// resolution, so resolvers work from these owned copies).
#[derive(Debug, Default)]
pub struct PageInfo {
    /// Site-root-relative URL.
    pub url: String,
    /// The page's title as plain text (for `<title>`, nav labels, and
    /// default xref text).
    pub title_text: Option<String>,
    /// The page's title as rendered inline HTML.
    pub title_html: Option<String>,
    /// The page's navigation label: its `navtitle` attribute when set,
    /// else the plain title (Antora's rule for empty xref text in nav).
    pub nav_text: Option<String>,
    /// Referenceable elements by id.
    pub refs: HashMap<String, RefEntry>,
    /// Reference-text → id (for natural cross-references).
    pub reftext_to_id: HashMap<String, String>,
}

impl PageInfo {
    /// Snapshots a parsed page.
    pub fn from_document(url: String, document: &Document<'_>) -> Self {
        let mut refs = HashMap::new();
        let mut reftext_to_id = HashMap::new();
        for (id, entry) in document.catalog().entries() {
            if let Some(reftext) = &entry.reftext {
                reftext_to_id
                    .entry(reftext.clone())
                    .or_insert_with(|| id.to_string());
            }
            refs.insert(id.to_string(), entry.clone());
        }

        let title_text = document.doctitle_sanitized();
        let nav_text = match document.attribute_value("navtitle") {
            InterpretedValue::Value(v) => Some(v),
            _ => title_text.clone(),
        };

        PageInfo {
            url,
            title_text,
            title_html: document.doctitle().map(str::to_string),
            nav_text,
            refs,
            reftext_to_id,
        }
    }
}

/// The site-wide reference index: one [`PageInfo`] per page, keyed by
/// coordinates.
#[derive(Debug, Default)]
pub struct SiteIndex {
    pages: HashMap<Coords, PageInfo>,
}

impl SiteIndex {
    /// Creates an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a page's snapshot.
    pub fn insert(&mut self, coords: Coords, info: PageInfo) {
        self.pages.insert(coords, info);
    }

    /// Looks up a page snapshot.
    pub fn get(&self, coords: &Coords) -> Option<&PageInfo> {
        self.pages.get(coords)
    }
}

/// The per-page [`ReferenceResolver`]: own-page targets resolve to `#id`
/// fragments (by id, then by reference text, mirroring the parser's
/// `CatalogResolver`); path-bearing targets resolve through the catalog to
/// a URL relative to this page.
pub struct PageResolver<'a> {
    catalog: &'a ContentCatalog,
    site: &'a SiteIndex,
    from: Coords,
    own: &'a PageInfo,
}

impl<'a> PageResolver<'a> {
    /// Builds the resolver for the page at `from`.
    pub fn new(
        catalog: &'a ContentCatalog,
        site: &'a SiteIndex,
        from: Coords,
        own: &'a PageInfo,
    ) -> Self {
        PageResolver {
            catalog,
            site,
            from,
            own,
        }
    }

    fn resolve_path_target(&self, target: &str) -> Option<ResolvedReference> {
        let (path_part, fragment) = match target.split_once('#') {
            Some((p, f)) => (p, Some(f)),
            None => (target, None),
        };

        let reference = ResourceRef::parse(path_part)?;
        let file = self.catalog.resolve(&reference, &self.from, Family::Page)?;
        let target_url = file.url.as_deref()?;
        let target_info = self.site.get(&file.coords);

        let mut href = relative_url(&self.own.url, target_url);
        if let Some(f) = fragment {
            href.push('#');
            href.push_str(f);

            // Prefer the fragment target's own reference text.
            if let Some(entry) = target_info.and_then(|info| info.refs.get(f)) {
                return Some(ResolvedReference::from_entry(href, entry));
            }
        }

        let text = target_info.and_then(|info| info.title_text.clone());
        Some(ResolvedReference::new(href, text))
    }
}

impl ReferenceResolver for PageResolver<'_> {
    fn resolve(&self, context: &ResolutionContext<'_>) -> Option<ResolvedReference> {
        // A target the parser derived a document destination for is a
        // path-bearing reference — ours to resolve through the catalog. A
        // catalog miss returns `None`, falling back to the parser's derived
        // destination plus its unresolved-reference warning.
        if context.derived.is_some() {
            return self.resolve_path_target(context.target);
        }

        // Otherwise mirror the parser's own single-document resolution
        // against this page's snapshot: direct id match first…
        if let Some(entry) = self.own.refs.get(context.target) {
            return Some(ResolvedReference::from_entry(
                format!("#{}", context.target),
                entry,
            ));
        }

        // …then a natural cross-reference by reference text.
        if let Some(id) = self.own.reftext_to_id.get(context.target) {
            return self
                .own
                .refs
                .get(id)
                .map(|entry| ResolvedReference::from_entry(format!("#{id}"), entry));
        }

        None
    }
}
