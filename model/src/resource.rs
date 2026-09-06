//! Resource families, resource-ID references, and URL helpers.

use std::fmt;

/// The family (kind) of a resource within a module, mirroring Antora's
/// family directories.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Family {
    /// `pages/` — AsciiDoc sources published as site pages.
    Page,
    /// `partials/` — includable AsciiDoc fragments (not published).
    Partial,
    /// `images/` — published under the module's `_images/` segment.
    Image,
    /// `attachments/` — published under the module's `_attachments/`
    /// segment.
    Attachment,
    /// `examples/` — includable code samples (not published).
    Example,
    /// Navigation files registered in `antora.yml` (internal family).
    Nav,
}

impl Family {
    /// The family's source directory name inside a module.
    ///
    /// `Nav` files live at the module base rather than a family directory,
    /// so they have no directory name here.
    pub fn dir_name(self) -> Option<&'static str> {
        match self {
            Family::Page => Some("pages"),
            Family::Partial => Some("partials"),
            Family::Image => Some("images"),
            Family::Attachment => Some("attachments"),
            Family::Example => Some("examples"),
            Family::Nav => None,
        }
    }

    /// The family coordinate used in resource IDs (`partial$`, `image$`, …).
    pub fn coordinate(self) -> &'static str {
        match self {
            Family::Page => "page",
            Family::Partial => "partial",
            Family::Image => "image",
            Family::Attachment => "attachment",
            Family::Example => "example",
            Family::Nav => "nav",
        }
    }

    fn from_coordinate(s: &str) -> Option<Self> {
        match s {
            "page" => Some(Family::Page),
            "partial" => Some(Family::Partial),
            "image" => Some(Family::Image),
            "attachment" => Some(Family::Attachment),
            "example" => Some(Family::Example),
            _ => None,
        }
    }

    /// Whether files of this family are published to the site.
    pub fn is_publishable(self) -> bool {
        matches!(self, Family::Page | Family::Image | Family::Attachment)
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.coordinate())
    }
}

/// A parsed resource-ID reference, before resolution against a catalog.
///
/// Mirrors Antora's resource-ID coordinate syntax
/// `version@component:module:family$path`, where every coordinate except the
/// path may be omitted and omitted coordinates default from the referencing
/// page's own coordinates at resolution time.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceRef {
    /// Explicit version coordinate (`3.2@…`), if given.
    pub version: Option<String>,
    /// Explicit component coordinate (`…component:module:…`), if given.
    pub component: Option<String>,
    /// Explicit module coordinate, if given. An empty module in a
    /// component-qualified reference (`component::page.adoc`) means `ROOT`
    /// and is normalized to `Some("ROOT")` here.
    pub module: Option<String>,
    /// Explicit family coordinate (`partial$…`), if given.
    pub family: Option<Family>,
    /// The family-relative path (`dir/file.adoc`), possibly starting with
    /// `./` for a reference relative to the referencing page's directory.
    pub path: String,
}

impl ResourceRef {
    /// Parses a resource-ID reference.
    ///
    /// Returns `None` for targets that are not resource IDs at all (an
    /// empty target, or a URL with a scheme).
    pub fn parse(target: &str) -> Option<Self> {
        if target.is_empty() || target.contains("://") || target.starts_with("mailto:") {
            return None;
        }

        let mut rest = target;
        let mut version = None;

        // The version coordinate ends with `@` and precedes everything else.
        if let Some(at) = rest.find('@') {
            let (v, r) = rest.split_at(at);

            // Ignore an `@` that appears after a colon — that is not a
            // version coordinate position.
            if !v.contains(':') {
                if !v.is_empty() {
                    version = Some(v.to_string());
                }
                rest = &r[1..];
            }
        }

        // Component and module are colon-delimited prefixes:
        //   0 colons → path only
        //   1 colon  → module:path
        //   2 colons → component:module:path (empty module = ROOT)
        let parts: Vec<&str> = rest.splitn(3, ':').collect();
        let (component, module, path_part) = match parts.as_slice() {
            [p] => (None, None, *p),
            [m, p] => (None, Some(m.to_string()), *p),
            [c, m, p] => {
                let module = if m.is_empty() {
                    "ROOT".to_string()
                } else {
                    m.to_string()
                };
                (Some(c.to_string()), Some(module), *p)
            }
            _ => unreachable!(),
        };

        // A lone module coordinate that is empty (`:path`) is not valid.
        if module.as_deref() == Some("") {
            return None;
        }

        // The family coordinate is a `family$` prefix on the path part.
        let (family, path) = match path_part.split_once('$') {
            Some((f, p)) => (Family::from_coordinate(f)?, p),
            None => return Some(Self::assemble(version, component, module, None, path_part)),
        };

        Some(Self::assemble(
            version,
            component,
            module,
            Some(family),
            path,
        ))
    }

    fn assemble(
        version: Option<String>,
        component: Option<String>,
        module: Option<String>,
        family: Option<Family>,
        path: &str,
    ) -> Self {
        ResourceRef {
            version,
            component,
            module,
            family,
            path: path.to_string(),
        }
    }
}

/// Computes the relative URL from one site-root-relative URL to another.
///
/// Both inputs are site-root-relative paths without a leading slash (the
/// form stored on [`VirtualFile::url`](crate::VirtualFile)). The result is
/// suitable for an `href` in the page published at `from`, so the site works
/// when served from any base path, including `file://`.
pub fn relative_url(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = {
        let mut v: Vec<&str> = from.split('/').collect();
        v.pop();
        v
    };
    let to_parts: Vec<&str> = to.split('/').collect();

    let common = from_dir
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();

    let mut out: Vec<&str> = vec![".."; from_dir.len() - common];
    out.extend(&to_parts[common..]);

    if out.is_empty() {
        // Same file; link to it by name.
        to_parts.last().unwrap_or(&"").to_string()
    } else {
        out.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_path() {
        let r = ResourceRef::parse("page.adoc").unwrap();
        assert_eq!(
            r,
            ResourceRef {
                path: "page.adoc".to_string(),
                ..Default::default()
            }
        );
    }

    #[test]
    fn parses_module_and_path() {
        let r = ResourceRef::parse("api:options.adoc").unwrap();
        assert_eq!(r.module.as_deref(), Some("api"));
        assert_eq!(r.path, "options.adoc");
        assert_eq!(r.component, None);
    }

    #[test]
    fn parses_component_module_path() {
        let r = ResourceRef::parse("html5:api:options.adoc").unwrap();
        assert_eq!(r.component.as_deref(), Some("html5"));
        assert_eq!(r.module.as_deref(), Some("api"));
        assert_eq!(r.path, "options.adoc");
    }

    #[test]
    fn empty_module_in_component_ref_means_root() {
        let r = ResourceRef::parse("html5::index.adoc").unwrap();
        assert_eq!(r.component.as_deref(), Some("html5"));
        assert_eq!(r.module.as_deref(), Some("ROOT"));
        assert_eq!(r.path, "index.adoc");
    }

    #[test]
    fn parses_version_and_family() {
        let r = ResourceRef::parse("3.2@html5:api:partial$snip.adoc").unwrap();
        assert_eq!(r.version.as_deref(), Some("3.2"));
        assert_eq!(r.family, Some(Family::Partial));
        assert_eq!(r.path, "snip.adoc");
    }

    #[test]
    fn parses_family_only() {
        let r = ResourceRef::parse("partial$header.adoc").unwrap();
        assert_eq!(r.family, Some(Family::Partial));
        assert_eq!(r.path, "header.adoc");
        assert_eq!(r.module, None);
    }

    #[test]
    fn rejects_urls_and_unknown_families() {
        assert_eq!(ResourceRef::parse("https://example.org"), None);
        assert_eq!(ResourceRef::parse("bogus$file.adoc"), None);
        assert_eq!(ResourceRef::parse(""), None);
    }

    #[test]
    fn relative_urls() {
        assert_eq!(
            relative_url("html5/index.html", "html5/api/options.html"),
            "api/options.html"
        );
        assert_eq!(
            relative_url("html5/api/options.html", "html5/index.html"),
            "../index.html"
        );
        assert_eq!(
            relative_url("html5/api/options.html", "html5/api/index.html"),
            "index.html"
        );
        assert_eq!(relative_url("index.html", "index.html"), "index.html");
        assert_eq!(
            relative_url("html5/cli/io.html", "html5/_images/pipe.png"),
            "../_images/pipe.png"
        );
    }
}
