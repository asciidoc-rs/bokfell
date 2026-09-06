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
            css_href => escape_html(&relative_url(ctx.url, CSS_URL)),
            home_href => escape_html(&relative_url(ctx.url, ctx.home_url)),
        })?;

        Ok(html)
    }

    /// The theme's static assets as `(site-root-relative path, bytes)`.
    pub fn assets(&self) -> Vec<(String, Vec<u8>)> {
        vec![(CSS_URL.to_string(), DEFAULT_CSS.as_bytes().to_vec())]
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

fn escape_html(text: &str) -> String {
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
    }

    #[test]
    fn redirect_page_escapes_target() {
        let page = Theme::redirect_page("html5/index.html");
        assert!(page.contains("url=html5/index.html"));
    }
}
