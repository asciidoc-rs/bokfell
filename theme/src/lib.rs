//! Theming for the Bokfell documentation site generator.
//!
//! A theme wraps each page's embedded HTML in the site chrome: layout,
//! sidebar navigation, and stylesheet. The default theme is compiled into
//! the binary; a theme directory can override the layout template per file
//! (PLAN.md §5 — no Antora-style zip bundles).

use std::path::Path;

use bokfell_model::{relative_url, NavItem, NavTree};
use minijinja::{context, Environment};

/// The embedded default layout template.
const DEFAULT_LAYOUT: &str = include_str!("../templates/layout.html");

/// The embedded default stylesheet, published as `_/bokfell.css`.
const DEFAULT_CSS: &str = include_str!("../assets/bokfell.css");

/// The URL of the published stylesheet, relative to the site root.
const CSS_URL: &str = "_/bokfell.css";

/// The embedded overlay script (coverage shading and diff highlighting),
/// published as `_/bokfell-overlay.js`.
const OVERLAY_JS: &str = include_str!("../assets/bokfell-overlay.js");

/// The URL of the published overlay script, relative to the site root.
const OVERLAY_JS_URL: &str = "_/bokfell-overlay.js";

/// Errors from theme loading or page composition.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    /// A template failed to load or render.
    #[error("template error: {0}")]
    Template(#[from] minijinja::Error),

    /// A theme override file could not be read.
    #[error("cannot read theme file {path}: {source}")]
    Io {
        /// The file being read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },
}

/// Everything the layout needs to compose one page.
#[derive(Debug)]
pub struct PageContext<'a> {
    /// The site title.
    pub site_title: &'a str,
    /// The page's site-root-relative URL.
    pub url: &'a str,
    /// The page title as rendered inline HTML.
    pub title_html: Option<&'a str>,
    /// The page title as plain text.
    pub title_text: Option<&'a str>,
    /// The page's embedded (body-only) HTML.
    pub contents: &'a str,
    /// The component's navigation tree.
    pub nav: &'a NavTree,
    /// The `href` the site title links to (site-root-relative URL of the
    /// start page redirect).
    pub home_url: &'a str,
    /// This page in every version of its component, highest first. The
    /// selector renders only when there is more than one entry.
    pub versions: &'a [VersionLink],
    /// The page's spec coverage, when coverage data covers it (PLAN.md
    /// §9.2). `None` renders no coverage badge.
    pub coverage: Option<CoverageView>,
    /// What changed on this page relative to its diff base (PLAN.md
    /// §9.1). `None` renders no diff badge.
    pub diff: Option<DiffView>,
    /// The combined JSON payload for the client overlay script: the
    /// block-pairing selector plus whichever of the coverage/diff block
    /// arrays this page carries. Must be safe to embed in a `<script>`
    /// element (serialize `<` escaped). `None` renders no payload or
    /// script.
    pub overlay_json: Option<String>,
}

/// The change summary of one page relative to its diff base (PLAN.md
/// §9.1).
#[derive(Clone, Debug)]
pub struct DiffView {
    /// What the page is compared against (a version label or base ref).
    pub base_label: String,
    /// The page does not exist in the base at all.
    pub new_page: bool,
    /// Count of added blocks.
    pub added: usize,
    /// Count of removed blocks.
    pub removed: usize,
    /// Count of edited blocks.
    pub edited: usize,
    /// Site-root-relative URL of the site's what-changed index.
    pub changes_url: String,
}

/// The spec-coverage presentation of one page (PLAN.md §9.2).
#[derive(Clone, Debug)]
pub struct CoverageView {
    /// Percentage of the page's normative lines that are verified (0–100).
    pub percent: u32,
    /// Count of verified normative lines.
    pub verified: usize,
    /// Count of normative-but-uncovered lines.
    pub uncovered: usize,
    /// Site-root-relative URL of the coverage dashboard page.
    pub dashboard_url: String,
}

/// The badge color band for a coverage percentage
/// (`high` ≥ 90, `mid` ≥ 50, `low` below).
pub fn coverage_level(percent: u32) -> &'static str {
    match percent {
        90.. => "high",
        50.. => "mid",
        _ => "low",
    }
}

/// One entry of the page-version selector.
#[derive(Clone, Debug)]
pub struct VersionLink {
    /// Display label (the display version, version, or `default`).
    pub label: String,
    /// Site-root-relative URL of this page in that version, when the page
    /// exists there.
    pub url: Option<String>,
    /// Whether this entry is the version being viewed.
    pub current: bool,
}

/// A loaded theme.
pub struct Theme {
    env: Environment<'static>,
}

impl Theme {
    /// Loads the built-in default theme.
    pub fn default_theme() -> Result<Self, ThemeError> {
        Self::load(None)
    }

    /// Loads the default theme with per-file overrides from a theme
    /// directory: a `layout.html` there replaces the built-in layout.
    pub fn load(overrides: Option<&Path>) -> Result<Self, ThemeError> {
        let mut env = Environment::new();

        let layout = match overrides.map(|dir| dir.join("layout.html")) {
            Some(path) if path.is_file() => {
                std::fs::read_to_string(&path).map_err(|source| ThemeError::Io {
                    path: path.display().to_string(),
                    source,
                })?
            }
            _ => DEFAULT_LAYOUT.to_string(),
        };
        env.add_template_owned("layout.html", layout)?;

        Ok(Theme { env })
    }

    /// Composes one page into a complete HTML document.
    pub fn compose_page(&self, ctx: &PageContext<'_>) -> Result<String, ThemeError> {
        let template = self.env.get_template("layout.html")?;

        let html = template.render(context! {
            bokfell_version => env!("CARGO_PKG_VERSION"),
            site => context! { title => ctx.site_title },
            page => context! {
                title_html => ctx.title_html,
                title_text => ctx.title_text,
                contents => ctx.contents,
                url => ctx.url,
            },
            nav_html => nav_html(ctx.nav, ctx.url),
            versions_html => versions_html(ctx.versions, ctx.url),
            css_href => escape_html(&relative_url(ctx.url, CSS_URL)),
            home_href => escape_html(&relative_url(ctx.url, ctx.home_url)),
            coverage => ctx.coverage.as_ref().map(|cov| context! {
                percent => cov.percent,
                verified => cov.verified,
                uncovered => cov.uncovered,
                level => coverage_level(cov.percent),
                dashboard_href =>
                    escape_html(&relative_url(ctx.url, &cov.dashboard_url)),
            }),
            diff => ctx.diff.as_ref().map(|diff| context! {
                base_label => diff.base_label.clone(),
                new_page => diff.new_page,
                added => diff.added,
                removed => diff.removed,
                edited => diff.edited,
                changes_href =>
                    escape_html(&relative_url(ctx.url, &diff.changes_url)),
            }),
            overlay_json => ctx.overlay_json.clone(),
            overlay_script_href =>
                escape_html(&relative_url(ctx.url, OVERLAY_JS_URL)),
        })?;

        Ok(html)
    }

    /// The theme's static assets as `(site-root-relative path, bytes)`.
    pub fn assets(&self) -> Vec<(String, Vec<u8>)> {
        vec![
            (CSS_URL.to_string(), DEFAULT_CSS.as_bytes().to_vec()),
            (OVERLAY_JS_URL.to_string(), OVERLAY_JS.as_bytes().to_vec()),
        ]
    }

    /// A minimal redirect page (used for the site root → start page hop).
    pub fn redirect_page(target_href: &str) -> String {
        let escaped = escape_html(target_href);
        format!(
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
             <link rel=\"canonical\" href=\"{escaped}\">\n\
             <meta http-equiv=\"refresh\" content=\"0; url={escaped}\">\n\
             <title>Redirect</title>\n</head>\n\
             <body><a href=\"{escaped}\">Redirecting…</a></body>\n</html>\n"
        )
    }
}

/// Renders the page-version selector: nothing for a single version, else
/// one link (or unlinked label) per version, current marked.
fn versions_html(versions: &[VersionLink], page_url: &str) -> String {
    if versions.len() < 2 {
        return String::new();
    }

    let mut out = String::from("<nav class=\"page-versions\" aria-label=\"Versions\">");
    for link in versions {
        let label = escape_html(&link.label);
        match (&link.url, link.current) {
            (Some(url), false) => {
                out.push_str(&format!(
                    "<a href=\"{}\">{label}</a>",
                    escape_html(&relative_url(page_url, url))
                ));
            }
            (Some(_), true) => {
                out.push_str(&format!("<span class=\"current\">{label}</span>"));
            }
            (None, _) => {
                out.push_str(&format!(
                    "<span class=\"missing\" title=\"This page does not exist in this version\">{label}</span>"
                ));
            }
        }
    }
    out.push_str("</nav>");
    out
}

/// Renders a navigation tree as nested `<ul>` lists with URLs relativized
/// to the current page.
fn nav_html(tree: &NavTree, page_url: &str) -> String {
    let mut out = String::new();
    if !tree.items.is_empty() {
        render_items(&tree.items, page_url, &mut out);
    }
    out
}

fn render_items(items: &[NavItem], page_url: &str, out: &mut String) {
    out.push_str("<ul>\n");
    for item in items {
        out.push_str("<li>");
        match &item.url {
            Some(url) if url.contains("://") => {
                out.push_str(&format!(
                    "<a href=\"{}\">{}</a>",
                    escape_html(url),
                    escape_html(&item.text)
                ));
            }
            Some(url) => {
                let class = if url == page_url {
                    " class=\"current\""
                } else {
                    ""
                };
                out.push_str(&format!(
                    "<a href=\"{}\"{class}>{}</a>",
                    escape_html(&relative_url(page_url, url)),
                    escape_html(&item.text)
                ));
            }
            None => {
                out.push_str(&format!(
                    "<span class=\"nav-heading\">{}</span>",
                    escape_html(&item.text)
                ));
            }
        }
        if !item.children.is_empty() {
            out.push('\n');
            render_items(&item.children, page_url, out);
        }
        out.push_str("</li>\n");
    }
    out.push_str("</ul>\n");
}

/// Escapes `&`, `<`, `>`, and `"` for embedding text in HTML (element
/// content or a double-quoted attribute).
pub fn escape_html(text: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, url: Option<&str>, children: Vec<NavItem>) -> NavItem {
        NavItem {
            html: String::new(),
            text: text.to_string(),
            url: url.map(str::to_string),
            children,
        }
    }

    #[test]
    fn composes_a_page_with_nav() {
        let theme = Theme::default_theme().unwrap();
        let nav = NavTree {
            items: vec![
                item("Start", Some("html5/index.html"), Vec::new()),
                item(
                    "API",
                    None,
                    vec![item("Options", Some("html5/api/options.html"), Vec::new())],
                ),
            ],
        };

        let html = theme
            .compose_page(&PageContext {
                site_title: "AsciiDoc HTML5",
                url: "html5/api/options.html",
                title_html: Some("Options &amp; <code>Options</code>"),
                title_text: Some("Options"),
                contents: "<div class=\"paragraph\"><p>Body.</p></div>",
                nav: &nav,
                home_url: "index.html",
                versions: &[],
                coverage: None,
                diff: None,
                overlay_json: None,
            })
            .unwrap();

        assert!(html.contains("<title>Options :: AsciiDoc HTML5</title>"));
        assert!(html.contains("Options &amp; <code>Options</code>"));
        assert!(html.contains("<p>Body.</p>"));

        // Nav links are relativized to the page and the current page is
        // marked.
        assert!(html.contains("<a href=\"../index.html\">Start</a>"));
        assert!(html.contains("<a href=\"options.html\" class=\"current\">Options</a>"));
        assert!(html.contains("<span class=\"nav-heading\">API</span>"));

        // The stylesheet link climbs to the site root.
        assert!(html.contains("href=\"../../_/bokfell.css\""));

        // No overlays → no badges, payload, or overlay script.
        assert!(!html.contains("page-overlays"));
        assert!(!html.contains("bokfell-overlay-data"));
        assert!(!html.contains("bokfell-overlay.js"));
    }

    #[test]
    fn composes_the_overlay_badges_and_payload() {
        let theme = Theme::default_theme().unwrap();
        let html = theme
            .compose_page(&PageContext {
                site_title: "Demo",
                url: "demo/guide/page.html",
                title_html: None,
                title_text: Some("Page"),
                contents: "<div class=\"paragraph\"><p>Body.</p></div>",
                nav: &NavTree::default(),
                home_url: "index.html",
                versions: &[],
                coverage: Some(CoverageView {
                    percent: 67,
                    verified: 2,
                    uncovered: 1,
                    dashboard_url: "coverage.html".to_string(),
                }),
                diff: Some(DiffView {
                    base_label: "1.3".to_string(),
                    new_page: false,
                    added: 1,
                    removed: 2,
                    edited: 3,
                    changes_url: "whats-changed.html".to_string(),
                }),
                overlay_json: Some(
                    "{\"selector\":\".paragraph\",\"coverage\":[\"verified\"],\"diff\":[null]}"
                        .to_string(),
                ),
            })
            .unwrap();

        // Coverage badge with the mid-band color class and the dashboard
        // link, relativized from the page.
        assert!(html.contains("coverage-badge cov-mid"), "html: {html}");
        assert!(html.contains(">67% verified</button>"));
        assert!(html.contains("<a href=\"../../coverage.html\">all pages</a>"));

        // Diff badge with the base label, counts in the tooltip, and the
        // what-changed link.
        assert!(html.contains(">Changed since 1.3</button>"));
        assert!(html.contains("1 added, 3 edited, 2 removed blocks vs 1.3"));
        assert!(html.contains("<a href=\"../../whats-changed.html\">what changed</a>"));

        // The JSON payload is embedded verbatim and the overlay script is
        // referenced relative to the page.
        assert!(html.contains(
            "<script type=\"application/json\" id=\"bokfell-overlay-data\">\
             {\"selector\":\".paragraph\",\"coverage\":[\"verified\"],\"diff\":[null]}</script>"
        ));
        assert!(html.contains("<script src=\"../../_/bokfell-overlay.js\" defer></script>"));

        // The script ships as a theme asset.
        assert!(theme
            .assets()
            .iter()
            .any(|(url, _)| url == "_/bokfell-overlay.js"));
    }

    #[test]
    fn new_page_diff_badge() {
        let theme = Theme::default_theme().unwrap();
        let html = theme
            .compose_page(&PageContext {
                site_title: "Demo",
                url: "demo/page.html",
                title_html: None,
                title_text: Some("Page"),
                contents: "<div class=\"paragraph\"><p>Body.</p></div>",
                nav: &NavTree::default(),
                home_url: "index.html",
                versions: &[],
                coverage: None,
                diff: Some(DiffView {
                    base_label: "1.0".to_string(),
                    new_page: true,
                    added: 1,
                    removed: 0,
                    edited: 0,
                    changes_url: "whats-changed.html".to_string(),
                }),
                overlay_json: Some(
                    "{\"selector\":\".paragraph\",\"diff\":[\"added\"]}".to_string(),
                ),
            })
            .unwrap();

        assert!(html.contains(">New since 1.0</button>"), "html: {html}");
        assert!(html.contains("does not exist in 1.0"));
        assert!(!html.contains("coverage-badge"));
    }

    #[test]
    fn coverage_levels_band_correctly() {
        assert_eq!(coverage_level(100), "high");
        assert_eq!(coverage_level(90), "high");
        assert_eq!(coverage_level(89), "mid");
        assert_eq!(coverage_level(50), "mid");
        assert_eq!(coverage_level(49), "low");
        assert_eq!(coverage_level(0), "low");
    }

    #[test]
    fn redirect_page_escapes_target() {
        let page = Theme::redirect_page("html5/index.html");
        assert!(page.contains("url=html5/index.html"));
    }
}
