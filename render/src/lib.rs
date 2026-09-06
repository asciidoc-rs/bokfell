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

mod includes;
mod inline_text;
mod navbuild;
mod resolver;

use std::sync::Arc;

use asciidoc_parser::{
    parser::{HtmlInlineRenderer, ModificationContext},
    Parser, SafeMode,
};
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
}

/// The result of rendering a whole site.
#[derive(Debug)]
pub struct RenderedSite {
    /// Every rendered page.
    pub pages: Vec<RenderedPage>,
    /// One navigation tree per component, in catalog order.
    pub navs: Vec<(String, NavTree)>,
}

/// The render pipeline over one content catalog.
pub struct Pipeline {
    catalog: Arc<ContentCatalog>,
    site_attrs: Vec<(String, Option<String>)>,
}

struct ParsedPage {
    coords: Coords,
    url: String,
    parser: Parser,
    document: asciidoc_parser::Document<'static>,
    warnings: Vec<String>,
}

impl Pipeline {
    /// Creates a pipeline over `catalog`, seeding every page with the
    /// site-wide attributes. `(name, None)` entries (unset seeds) are
    /// skipped for now.
    pub fn new(catalog: ContentCatalog, site_attrs: Vec<(String, Option<String>)>) -> Self {
        Pipeline {
            catalog: Arc::new(catalog),
            site_attrs,
        }
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

        // Phase 2b + 3: resolve each page against the site and render it.
        let render_options = asciidoc_html5::Options::new().embedded(true);
        let mut pages = Vec::new();
        for page in &mut parsed {
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

            let contents = asciidoc_html5::convert_document_with(&page.document, &render_options);

            pages.push(RenderedPage {
                coords: page.coords.clone(),
                url: page.url.clone(),
                title_html: own.title_html.clone(),
                title_text: own.title_text.clone(),
                contents,
                warnings: std::mem::take(&mut page.warnings),
            });
        }

        // Navigation trees, one per component.
        let mut navs = Vec::new();
        for component in self.catalog.components() {
            let tree = self.build_nav(component, &index)?;
            navs.push((component.desc.name.clone(), tree));
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

        let mut parser = self.build_parser(file, component, Some(&url));
        let document = parser.parse_deferred(&source);

        let warnings = document
            .warnings()
            .map(|w| format!("{w:?}"))
            .collect::<Vec<_>>();

        Ok(ParsedPage {
            coords: file.coords.clone(),
            url,
            parser,
            document,
            warnings,
        })
    }

    /// Builds the configured parser for one source file.
    ///
    /// `page_url` supplies the URL context for the `imagesdir` seed; nav
    /// files pass `None` (their links are resolved through the catalog, not
    /// attribute-relative paths).
    fn build_parser(
        &self,
        file: &VirtualFile,
        component: &Component,
        page_url: Option<&str>,
    ) -> Parser {
        let include_handler = includes::CatalogIncludeHandler::new(
            self.catalog.clone(),
            file.coords.clone(),
            file.src_path.parent().map(|p| p.to_path_buf()),
        );

        let mut parser = Parser::default()
            .with_safe_mode(SafeMode::Safe)
            .with_primary_file_name(file.src_path.display().to_string())
            .with_include_file_handler(include_handler);

        // Site-wide attributes, then the component's (later wins).
        let component_attrs = component.attribute_seeds();
        for (name, value) in self.site_attrs.iter().chain(component_attrs.iter()) {
            if let Some(value) = value {
                parser = parser.with_intrinsic_attribute(name, value, ModificationContext::ApiOnly);
            }
        }

        // Intrinsic page context attributes (Antora's page-* family).
        parser = parser
            .with_intrinsic_attribute(
                "page-component-name",
                &component.desc.name,
                ModificationContext::ApiOnly,
            )
            .with_intrinsic_attribute(
                "page-component-title",
                component.desc.title(),
                ModificationContext::ApiOnly,
            )
            .with_intrinsic_attribute(
                "page-module",
                &file.coords.module,
                ModificationContext::ApiOnly,
            );

        // `imagesdir` points at the module's published `_images/` directory,
        // relative to the page, so `image::name.png[]` resolves in place.
        if let Some(url) = page_url {
            if let Some(imagesdir) = imagesdir_for(url, &file.coords) {
                parser = parser.with_intrinsic_attribute(
                    "imagesdir",
                    imagesdir,
                    ModificationContext::ApiOnly,
                );
            }
        }

        parser
    }

    fn build_nav(&self, component: &Component, index: &SiteIndex) -> Result<NavTree, RenderError> {
        let mut tree = NavTree::default();

        let nav_files: Vec<VirtualFile> = self
            .catalog
            .files_of(Family::Nav)
            .filter(|f| f.coords.component == component.desc.name)
            .cloned()
            .collect();

        for file in &nav_files {
            let source =
                std::fs::read_to_string(&file.src_path).map_err(|source| RenderError::Io {
                    path: file.src_path.display().to_string(),
                    source,
                })?;

            let mut parser = self.build_parser(file, component, None);
            let document = parser.parse_deferred(&source);

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

    fn component_of(&self, coords: &Coords) -> Result<&Component, RenderError> {
        self.catalog
            .components()
            .iter()
            .find(|c| c.desc.name == coords.component)
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
