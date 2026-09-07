//! Catalog-aware AsciiDoc conversion for the Bokfell documentation site
//! generator.
//!
//! The render pipeline turns a scanned [`ContentCatalog`] into embedded
//! HTML per page plus navigation trees, in three phases (PLAN.md §6, §8):
//!
//! 1. **Deferred parse** of every page with a per-page configured parser (safe
//!    mode, attribute seeds, catalog-backed includes).
//! 2. **Site-wide reference resolution**: each page's references resolve
//!    against a snapshot index of every page ([`Document::resolve_references`]
//!    with a [`PageResolver`]).
//! 3. **Embedded rendering** through `asciidoc-html5` (`convert_document_with`,
//!    body-only output for the theme layer).
//!
//! [`Document::resolve_references`]: asciidoc_parser::Document::resolve_references

mod blockmap;
mod includes;
mod inline_text;
mod navbuild;
mod resolver;

use std::sync::Arc;

use asciidoc_parser::{parser::HtmlInlineRenderer, Parser, SafeMode};
pub use blockmap::client_selector as overlay_client_selector;
use bokfell_coverage::{CoverageScope, PageCoverage};
use bokfell_diff::{diff_blocks, BlockChange};
use bokfell_model::{
    relative_url, Component, ContentCatalog, Coords, Family, NavTree, VirtualFile,
};
pub use resolver::{PageInfo, PageResolver, SiteIndex};

/// Errors from the render pipeline.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// A source file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The file being read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A page's component is not registered in the catalog.
    #[error("page {0} belongs to unknown component")]
    UnknownComponent(String),
}

/// One rendered page.
#[derive(Debug)]
pub struct RenderedPage {
    /// The page's coordinates.
    pub coords: Coords,
    /// Site-root-relative URL.
    pub url: String,
    /// The page title as rendered inline HTML (for the `<h1>`).
    pub title_html: Option<String>,
    /// The page title as plain text (for `<title>` and labels).
    pub title_text: Option<String>,
    /// The embedded (body-only) HTML contents.
    pub contents: String,
    /// Human-readable warnings collected while rendering this page.
    pub warnings: Vec<String>,
    /// Spec coverage of this page, when coverage data was supplied and
    /// covers it (PLAN.md §9.2).
    pub coverage: Option<PageCoverage>,
    /// What changed on this page relative to its diff base, when diffing
    /// was configured and a base exists (PLAN.md §9.1).
    pub diff: Option<PageDiff>,
    /// Per overlay block, the source file and 1-based line the block
    /// starts at — the click-to-source targets for the edit round-trip
    /// (PLAN.md §9.3). `None` for a block whose origin can't be traced
    /// to a file.
    pub block_sources: Vec<Option<(std::path::PathBuf, usize)>>,
    /// Per overlay block, the 1-based *preprocessed* line its span starts
    /// at — the value the rendered container's `data-source-line`
    /// attribute carries, letting the client anchor overlays exactly.
    pub block_lines: Vec<u32>,
}

/// One page's changes relative to its diff base (the previous component
/// version, or the base catalog in PR-preview mode).
#[derive(Clone, Debug)]
pub struct PageDiff {
    /// What the page was compared against (a version label, or the base
    /// ref name in PR mode).
    pub base_label: String,
    /// The page does not exist in the base at all.
    pub new_page: bool,
    /// Count of added blocks.
    pub added: usize,
    /// Count of removed blocks (present in the base only).
    pub removed: usize,
    /// Count of edited blocks.
    pub edited: usize,
    /// Per-block changes in document order (the overlay-block walk of the
    /// *new* document).
    pub blocks: Vec<PageBlockChange>,
}

impl PageDiff {
    /// Whether anything changed relative to the base.
    pub fn is_changed(&self) -> bool {
        self.new_page || self.added + self.removed + self.edited > 0
    }
}

/// One rendered block's change relative to the diff base.
#[derive(Clone, Debug)]
pub enum PageBlockChange {
    /// Identical in the base.
    Unchanged,
    /// Not present in the base.
    Added,
    /// Present in the base with different content.
    Edited {
        /// The block's source with word-level `<del>`/`<ins>` markers
        /// (HTML-escaped).
        diff_html: String,
    },
}

/// What pages are diffed against (PLAN.md §9.1).
pub enum DiffBase {
    /// Each versioned page diffs against the previous version of its
    /// component (Antora version order); the oldest version has no diff.
    PreviousVersion,
    /// Every page diffs against the page with the same coordinates in a
    /// separate base catalog — PR preview mode (base ref vs head).
    Catalog {
        /// A pipeline over the base content (typically the same sources
        /// aggregated at the base ref).
        pipeline: Box<Pipeline>,
        /// The label pages report as their base (e.g. the base ref name).
        label: String,
    },
}

/// The result of rendering a whole site.
#[derive(Debug)]
pub struct RenderedSite {
    /// Every rendered page.
    pub pages: Vec<RenderedPage>,
    /// One navigation tree per component *version*, in catalog order,
    /// keyed by `(component name, version)`.
    pub navs: Vec<((String, Option<String>), NavTree)>,
}

/// The render pipeline over one content catalog.
pub struct Pipeline {
    catalog: Arc<ContentCatalog>,
    site_attrs: Vec<(String, Option<String>)>,
    coverage: Vec<CoverageScope>,
    diff_base: Option<DiffBase>,
}

struct ParsedPage {
    coords: Coords,
    url: String,
    /// The parser's `primary_file_name` for this page — the name the
    /// source map reports for the page's own (top-level) lines. Matches
    /// the canonicalized path `Options::input_file` sets.
    primary_file_name: String,
    /// The options the page was loaded with (also drive its conversion).
    options: asciidoc_html5::Options,
    parser: Parser,
    document: asciidoc_parser::Document<'static>,
    warnings: Vec<String>,
}

/// The name `Options::input_file` will register as the parser's primary
/// file name: the canonicalized path, falling back to an absolutized or
/// verbatim one — mirrored here so source-map origins compare equal.
fn primary_name_for(path: &std::path::Path) -> String {
    path.canonicalize()
        .ok()
        .or_else(|| std::env::current_dir().ok().map(|cwd| cwd.join(path)))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}

impl Pipeline {
    /// Creates a pipeline over `catalog`, seeding every page with the
    /// site-wide attributes. `(name, None)` entries (unset seeds) are
    /// skipped for now.
    pub fn new(catalog: ContentCatalog, site_attrs: Vec<(String, Option<String>)>) -> Self {
        Pipeline {
            catalog: Arc::new(catalog),
            site_attrs,
            coverage: Vec::new(),
            diff_base: None,
        }
    }

    /// Configures page diffing (PLAN.md §9.1). Pages with a base get a
    /// [`PageDiff`] on their [`RenderedPage`].
    pub fn with_diff(mut self, base: DiffBase) -> Self {
        self.diff_base = Some(base);
        self
    }

    /// Supplies spec-coverage data (PLAN.md §9.2), one scope per content
    /// source: a page reads only the scopes that cover its own component
    /// version, first hit wins.
    pub fn with_coverage(mut self, coverage: Vec<CoverageScope>) -> Self {
        self.coverage = coverage;
        self
    }

    /// The catalog the pipeline renders from.
    pub fn catalog(&self) -> &ContentCatalog {
        &self.catalog
    }

    /// Renders every page and builds every component's navigation tree.
    pub fn render_site(&self) -> Result<RenderedSite, RenderError> {
        // Phase 1: deferred parse of every page.
        let mut parsed: Vec<ParsedPage> = Vec::new();
        let page_files: Vec<VirtualFile> = self.catalog.files_of(Family::Page).cloned().collect();
        for file in &page_files {
            parsed.push(self.parse_page(file)?);
        }

        // Phase 2a: snapshot the site-wide reference index.
        let mut index = SiteIndex::new();
        for page in &parsed {
            index.insert(
                page.coords.clone(),
                PageInfo::from_document(page.url.clone(), &page.document),
            );
        }

        // Phase 2b': page diffs against the configured base, computed
        // while every parsed document is still immutable (PLAN.md §9.1).
        let mut diffs: Vec<Option<PageDiff>> = match &self.diff_base {
            None => vec![None; parsed.len()],
            Some(base) => parsed
                .iter()
                .map(|page| self.page_diff(page, &parsed, base))
                .collect(),
        };

        // Phase 2b + 3: resolve each page against the site and render it.
        let mut pages = Vec::new();
        for (page_index, page) in parsed.iter_mut().enumerate() {
            let own = index
                .get(&page.coords)
                .expect("every parsed page was indexed");
            let page_resolver = PageResolver::new(&self.catalog, &index, page.coords.clone(), own);
            let reference_warnings = page.document.resolve_references(
                &page_resolver,
                &HtmlInlineRenderer {},
                &page.parser,
            );
            for warning in reference_warnings {
                page.warnings
                    .push(format!("unresolved reference: {}", warning.target));
            }

            let contents = asciidoc_html5::convert_document_with(&page.document, &page.options);
            let coverage = self.page_coverage(page);
            let overlay = blockmap::overlay_blocks(&page.document);
            let block_sources = self.block_sources(page, &overlay);
            let block_lines: Vec<u32> = overlay.iter().map(|b| b.start_line).collect();

            pages.push(RenderedPage {
                coords: page.coords.clone(),
                url: page.url.clone(),
                title_html: own.title_html.clone(),
                title_text: own.title_text.clone(),
                contents,
                warnings: std::mem::take(&mut page.warnings),
                coverage,
                diff: diffs[page_index].take(),
                block_sources,
                block_lines,
            });
        }

        // Navigation trees, one per component version.
        let mut navs = Vec::new();
        for component in self.catalog.components() {
            let tree = self.build_nav(component, &index)?;
            navs.push((
                (component.desc.name.clone(), component.desc.version.clone()),
                tree,
            ));
        }

        Ok(RenderedSite { pages, navs })
    }

    fn parse_page(&self, file: &VirtualFile) -> Result<ParsedPage, RenderError> {
        let component = self.component_of(&file.coords)?;
        let url = file
            .url
            .clone()
            .expect("pages are publishable and carry a URL");

        let source = std::fs::read_to_string(&file.src_path).map_err(|source| RenderError::Io {
            path: file.src_path.display().to_string(),
            source,
        })?;

        let options = self.build_options(file, component, Some(&url));
        let (document, parser) = asciidoc_html5::load_deferred(&source, &options);

        let warnings = document
            .warnings()
            .map(|w| format!("{w:?}"))
            .collect::<Vec<_>>();

        Ok(ParsedPage {
            coords: file.coords.clone(),
            url,
            primary_file_name: primary_name_for(&file.src_path),
            options,
            parser,
            document,
            warnings,
        })
    }

    /// Builds the load/convert options for one source file: safe mode,
    /// the catalog-backed include handler, attribute overrides
    /// (`Options::attribute` maps to the same API-only modification
    /// context the raw parser used), embedded output, and
    /// `data-source-line` annotations for exact overlay anchoring.
    ///
    /// `page_url` supplies the URL context for the `imagesdir` seed; nav
    /// files pass `None` (their links are resolved through the catalog,
    /// not attribute-relative paths).
    fn build_options(
        &self,
        file: &VirtualFile,
        component: &Component,
        page_url: Option<&str>,
    ) -> asciidoc_html5::Options {
        let include_handler = includes::CatalogIncludeHandler::new(
            self.catalog.clone(),
            file.coords.clone(),
            file.src_path.parent().map(|p| p.to_path_buf()),
        );

        let mut options = asciidoc_html5::Options::new()
            .safe_mode(SafeMode::Safe)
            .input_file(&file.src_path)
            .include_file_handler(include_handler)
            .embedded(true)
            .source_locations(true);

        // Site-wide attributes, then the component's (later wins).
        let component_attrs = component.attribute_seeds();
        for (name, value) in self.site_attrs.iter().chain(component_attrs.iter()) {
            if let Some(value) = value {
                options = options.attribute(name, value);
            }
        }

        // Page context attributes (Antora's page-* family).
        options = options
            .attribute("page-component-name", &component.desc.name)
            .attribute("page-component-title", component.desc.title())
            .attribute("page-module", &file.coords.module);

        // `imagesdir` points at the module's published `_images/` directory,
        // relative to the page, so `image::name.png[]` resolves in place.
        if let Some(url) = page_url {
            if let Some(imagesdir) = imagesdir_for(url, &file.coords) {
                options = options.attribute("imagesdir", imagesdir);
            }
        }

        options
    }

    fn build_nav(&self, component: &Component, index: &SiteIndex) -> Result<NavTree, RenderError> {
        let mut tree = NavTree::default();

        let nav_files: Vec<VirtualFile> = self
            .catalog
            .files_of(Family::Nav)
            .filter(|f| {
                f.coords.component == component.desc.name
                    && f.coords.version == component.desc.version
            })
            .cloned()
            .collect();

        for file in &nav_files {
            let source =
                std::fs::read_to_string(&file.src_path).map_err(|source| RenderError::Io {
                    path: file.src_path.display().to_string(),
                    source,
                })?;

            let options = self.build_options(file, component, None);
            let (document, _parser) = asciidoc_html5::load_deferred(&source, &options);

            // Nav entries resolve as if from a page at the nav's module
            // root.
            let nav_coords = Coords {
                component: file.coords.component.clone(),
                version: file.coords.version.clone(),
                module: file.coords.module.clone(),
                family: Family::Page,
                path: String::new(),
            };

            navbuild::extend_tree(&mut tree, &document, &self.catalog, index, &nav_coords);
        }

        Ok(tree)
    }

    /// Computes a page's coverage: its line data (found under the first
    /// matching prefix) projected onto the page's overlay blocks.
    ///
    /// Coverage lines refer to the page's own file, while block spans are
    /// preprocessed-source lines; a block spliced in by `include::` is
    /// translated out via the source map (its lines belong to another
    /// file's coverage). A block that merely *follows* an include keeps
    /// its translated top-level line.
    fn page_coverage(&self, page: &ParsedPage) -> Option<PageCoverage> {
        let lines = self.coverage.iter().find_map(|scope| {
            if !scope.applies_to(&page.coords.component, page.coords.version.as_deref()) {
                return None;
            }
            scope.data.page_lines(&scope.prefix, &page.coords)
        })?;

        let source_map = page.document.source_map();
        let spans: Vec<(u32, u32)> = blockmap::overlay_blocks(&page.document)
            .iter()
            .map(|block| {
                match source_map.original_file_and_line(block.start_line as usize) {
                    // Top-level content — the map reports the page's own
                    // file (its `primary_file_name`, or `None` when no
                    // name was set): use the translated line.
                    Some(origin)
                        if origin.0.is_none()
                            || origin.0.as_deref() == Some(&page.primary_file_name) =>
                    {
                        (origin.1 as u32, block.line_count)
                    }
                    // Include-origin content (or unmapped): no lines of
                    // this page's coverage apply — line 0 never matches.
                    _ => (0, 0),
                }
            })
            .collect();

        Some(PageCoverage::from_lines(lines, &spans))
    }

    /// Computes one page's diff against the configured base: the base
    /// document's overlay units vs this page's, classified block by
    /// block. `None` when the page has no base (oldest version, or no
    /// diffing possible).
    fn page_diff(
        &self,
        page: &ParsedPage,
        parsed: &[ParsedPage],
        base: &DiffBase,
    ) -> Option<PageDiff> {
        let new_units = blockmap::overlay_units(&page.document);

        let (base_units, base_label) = match base {
            DiffBase::PreviousVersion => {
                let versions = self.catalog.versions_of(&page.coords.component);
                let position = versions
                    .iter()
                    .position(|c| c.desc.version == page.coords.version)?;
                let previous = versions.get(position + 1)?;
                let label = previous
                    .desc
                    .display_version
                    .clone()
                    .or_else(|| previous.desc.version.clone())
                    .unwrap_or_else(|| "default".to_string());

                let base_coords = Coords {
                    version: previous.desc.version.clone(),
                    ..page.coords.clone()
                };
                let base_page = parsed.iter().find(|p| p.coords == base_coords);
                (
                    base_page.map(|p| blockmap::overlay_units(&p.document)),
                    label,
                )
            }
            DiffBase::Catalog { pipeline, label } => {
                // Match by resource coordinates; the version must agree
                // when the base catalog carries several versions.
                let candidates: Vec<&VirtualFile> = pipeline
                    .catalog
                    .files_of(Family::Page)
                    .filter(|f| {
                        f.coords.component == page.coords.component
                            && f.coords.module == page.coords.module
                            && f.coords.path == page.coords.path
                    })
                    .collect();
                let file = candidates
                    .iter()
                    .find(|f| f.coords.version == page.coords.version)
                    .or_else(|| (candidates.len() == 1).then(|| &candidates[0]))
                    .copied();

                let units = file.and_then(|file| {
                    // A base page that fails to parse simply yields no
                    // diff for this page.
                    let base_page = pipeline.parse_page(file).ok()?;
                    Some(blockmap::overlay_units(&base_page.document))
                });
                (units, label.clone())
            }
        };

        match base_units {
            None => {
                // The page is new relative to the base: every block is an
                // addition.
                let added = new_units.len();
                Some(PageDiff {
                    base_label,
                    new_page: true,
                    added,
                    removed: 0,
                    edited: 0,
                    blocks: vec![PageBlockChange::Added; added],
                })
            }
            Some(base_units) => {
                let diff = diff_blocks(&base_units, &new_units);
                let mut blocks = vec![PageBlockChange::Unchanged; new_units.len()];
                for change in &diff.changes {
                    match change {
                        BlockChange::Added { new } => blocks[*new] = PageBlockChange::Added,
                        BlockChange::Edited { new, diff_html, .. } => {
                            blocks[*new] = PageBlockChange::Edited {
                                diff_html: diff_html.clone(),
                            }
                        }
                        _ => {}
                    }
                }
                Some(PageDiff {
                    base_label,
                    new_page: false,
                    added: diff.added,
                    removed: diff.removed,
                    edited: diff.edited,
                    blocks,
                })
            }
        }
    }

    /// Traces every overlay block back to the source file and line it
    /// starts at (PLAN.md §9.3): the page's own file for top-level
    /// blocks, the resolved include target for spliced-in ones.
    fn block_sources(
        &self,
        page: &ParsedPage,
        overlay: &[blockmap::OverlayBlock],
    ) -> Vec<Option<(std::path::PathBuf, usize)>> {
        let source_map = page.document.source_map();
        let root_dir = std::path::Path::new(&page.primary_file_name).parent();

        overlay
            .iter()
            .map(|block| {
                let origin = source_map.original_file_and_line(block.start_line as usize)?;
                match origin.0.as_deref() {
                    None => Some((std::path::PathBuf::from(&page.primary_file_name), origin.1)),
                    Some(name) if name == page.primary_file_name => {
                        Some((std::path::PathBuf::from(&page.primary_file_name), origin.1))
                    }
                    Some(target) => includes::resolve_include_source(
                        &self.catalog,
                        &page.coords,
                        root_dir,
                        target,
                    )
                    .map(|file| (file, origin.1)),
                }
            })
            .collect()
    }

    fn component_of(&self, coords: &Coords) -> Result<&Component, RenderError> {
        // Both coordinates matter: with several versions of one component
        // in the catalog, matching by name alone would hand every page the
        // first-scanned version's descriptor (attributes, title, nav).
        self.catalog
            .components()
            .iter()
            .find(|c| c.desc.name == coords.component && c.desc.version == coords.version)
            .ok_or_else(|| RenderError::UnknownComponent(coords.to_string()))
    }
}

/// The `imagesdir` seed for a page: the relative path from the page's URL
/// to its module's published `_images/` directory.
fn imagesdir_for(page_url: &str, coords: &Coords) -> Option<String> {
    let page_rel_html = coords
        .path
        .strip_suffix(".adoc")
        .map(|stem| format!("{stem}.html"))?;
    let module_base = page_url.strip_suffix(&page_rel_html)?;
    let images_url = format!("{module_base}_images");

    let probe = relative_url(page_url, &format!("{images_url}/_probe"));
    probe
        .strip_suffix("/_probe")
        .map(str::to_string)
        .or(Some(String::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coords(module: &str, path: &str) -> Coords {
        Coords {
            component: "html5".to_string(),
            version: None,
            module: module.to_string(),
            family: Family::Page,
            path: path.to_string(),
        }
    }

    #[test]
    fn imagesdir_relative_to_page_depth() {
        assert_eq!(
            imagesdir_for("html5/index.html", &coords("ROOT", "index.adoc")).as_deref(),
            Some("_images")
        );
        assert_eq!(
            imagesdir_for("html5/api/options.html", &coords("api", "options.adoc")).as_deref(),
            Some("_images")
        );
        assert_eq!(
            imagesdir_for("html5/sub/deep.html", &coords("ROOT", "sub/deep.adoc")).as_deref(),
            Some("../_images")
        );
    }
}
