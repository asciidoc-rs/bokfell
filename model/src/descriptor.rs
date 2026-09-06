//! The component version descriptor (`antora.yml`).

use std::collections::BTreeMap;

use serde::Deserialize;

/// Errors from reading a component version descriptor.
#[derive(Debug, thiserror::Error)]
pub enum DescriptorError {
    /// The descriptor file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The descriptor path.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The descriptor is not valid YAML or is missing required keys.
    #[error("invalid antora.yml at {path}: {source}")]
    Invalid {
        /// The descriptor path.
        path: String,
        /// The underlying parse error.
        source: serde_norway::Error,
    },
}

/// A component version descriptor, read from `antora.yml` at a content
/// source root.
///
/// Field meanings follow Antora's
/// [component version descriptor](https://docs.antora.org/antora/latest/component-version-descriptor/).
// Unknown keys are tolerated (Antora's descriptor grows keys like `ext`
// that we have no use for yet).
#[derive(Clone, Debug, Deserialize)]
pub struct ComponentDescriptor {
    /// The component name coordinate (also its URL segment; `ROOT` drops
    /// the segment).
    pub name: String,

    /// The version. `~`/`null` (and, for convenience, an absent key) mean
    /// a *versionless* component version, whose URLs carry no version
    /// segment.
    #[serde(default)]
    pub version: Option<String>,

    /// Display/sort title. Falls back to `name` when absent.
    #[serde(default)]
    pub title: Option<String>,

    /// Presentation-only version label.
    #[serde(default)]
    pub display_version: Option<String>,

    /// Prerelease marker (`true` or an identifier); excluded from "latest"
    /// routing. Parsed but not yet acted on in M1 (single-version builds).
    #[serde(default)]
    pub prerelease: Option<serde_norway::Value>,

    /// Ordered navigation file paths, relative to the content source root
    /// (e.g. `modules/ROOT/nav.adoc`). Order defines menu order.
    #[serde(default)]
    pub nav: Vec<String>,

    /// The component version's start page, as a page resource ID relative
    /// to this component (default `ROOT:index.adoc`).
    #[serde(default)]
    pub start_page: Option<String>,

    /// Component-scoped AsciiDoc configuration.
    #[serde(default)]
    pub asciidoc: AsciiDocConfig,
}

/// The `asciidoc` block of a descriptor or playbook: attributes seeded into
/// every page of the scope.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AsciiDocConfig {
    /// Attribute name → value. A `~`/`null` value means "unset". Values
    /// keep their YAML scalar form and are stringified when seeded.
    #[serde(default)]
    pub attributes: BTreeMap<String, serde_norway::Value>,
}

impl AsciiDocConfig {
    /// The attribute seeds this block contributes: `(name, Some(value))`
    /// to set, `(name, None)` to unset.
    pub fn attribute_seeds(&self) -> Vec<(String, Option<String>)> {
        self.attributes
            .iter()
            .map(|(k, v)| (k.clone(), attribute_value_string(v)))
            .collect()
    }
}

impl ComponentDescriptor {
    /// Reads and parses the `antora.yml` at `path`.
    pub fn load(path: &std::path::Path) -> Result<Self, DescriptorError> {
        let text = std::fs::read_to_string(path).map_err(|source| DescriptorError::Io {
            path: path.display().to_string(),
            source,
        })?;

        serde_norway::from_str(&text).map_err(|source| DescriptorError::Invalid {
            path: path.display().to_string(),
            source,
        })
    }

    /// The display title (explicit `title`, else the component name).
    pub fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    /// The start page resource ID (explicit, else `ROOT:index.adoc`).
    pub fn start_page(&self) -> &str {
        self.start_page.as_deref().unwrap_or("ROOT:index.adoc")
    }
}

/// Stringifies a YAML attribute value for seeding into the parser.
///
/// Returns `None` for `~`/`null` (an unset marker). Booleans map to the
/// AsciiDoc set/empty (`true` → `""`) convention; `false` maps to unset.
pub(crate) fn attribute_value_string(value: &serde_norway::Value) -> Option<String> {
    match value {
        serde_norway::Value::Null => None,
        serde_norway::Value::Bool(false) => None,
        serde_norway::Value::Bool(true) => Some(String::new()),
        serde_norway::Value::String(s) => Some(s.clone()),
        serde_norway::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_descriptor() {
        let d: ComponentDescriptor = serde_norway::from_str("name: html5\nversion: ~\n").unwrap();
        assert_eq!(d.name, "html5");
        assert_eq!(d.version, None);
        assert_eq!(d.title(), "html5");
        assert_eq!(d.start_page(), "ROOT:index.adoc");
        assert!(d.nav.is_empty());
    }

    #[test]
    fn parses_full_descriptor() {
        let d: ComponentDescriptor = serde_norway::from_str(
            "name: html5\n\
             title: AsciiDoc HTML5\n\
             version: '1.4'\n\
             start_page: index.adoc\n\
             nav:\n\
             - modules/ROOT/nav.adoc\n\
             - modules/api/nav.adoc\n\
             asciidoc:\n\
             \x20 attributes:\n\
             \x20   page-pagination: ''\n",
        )
        .unwrap();
        assert_eq!(d.title(), "AsciiDoc HTML5");
        assert_eq!(d.version.as_deref(), Some("1.4"));
        assert_eq!(d.nav.len(), 2);
        assert_eq!(
            attribute_value_string(&d.asciidoc.attributes["page-pagination"]).as_deref(),
            Some("")
        );
    }

    #[test]
    fn attribute_value_conventions() {
        use serde_norway::Value;
        assert_eq!(attribute_value_string(&Value::Null), None);
        assert_eq!(attribute_value_string(&Value::Bool(false)), None);
        assert_eq!(
            attribute_value_string(&Value::Bool(true)).as_deref(),
            Some("")
        );
    }
}
