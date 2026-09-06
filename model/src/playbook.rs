//! The Bokfell playbook (`bokfell.yml`).
//!
//! Bokfell defines its own playbook schema (PLAN.md §12) rather than
//! reading Antora playbooks directly; the core keys deliberately mirror
//! Antora's so an existing playbook translates mechanically.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::descriptor::AsciiDocConfig;

/// Errors from reading a playbook.
#[derive(Debug, thiserror::Error)]
pub enum PlaybookError {
    /// The playbook file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The playbook path.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// The playbook is not valid YAML or is missing required keys.
    #[error("invalid playbook at {path}: {source}")]
    Invalid {
        /// The playbook path.
        path: String,
        /// The underlying parse error.
        source: serde_norway::Error,
    },
}

/// A site playbook: which content goes into the site, and site-level
/// configuration.
#[derive(Clone, Debug, Deserialize)]
pub struct Playbook {
    /// Site-level settings.
    pub site: SiteConfig,

    /// The content sources.
    pub content: ContentConfig,

    /// Site-wide AsciiDoc attribute defaults.
    #[serde(default)]
    pub asciidoc: AsciiDocConfig,

    /// Output settings.
    #[serde(default)]
    pub output: OutputConfig,

    /// The directory the playbook was loaded from; source paths resolve
    /// relative to it. Not part of the file format.
    #[serde(skip)]
    pub base_dir: PathBuf,
}

/// The `site` block.
#[derive(Clone, Debug, Deserialize)]
pub struct SiteConfig {
    /// The site title, shown in page chrome.
    pub title: String,

    /// The site's start page, as a fully qualified page resource ID
    /// (e.g. `html5::index.adoc`). The site root redirects to it.
    #[serde(default)]
    pub start_page: Option<String>,
}

/// The `content` block.
#[derive(Clone, Debug, Deserialize)]
pub struct ContentConfig {
    /// The ordered content sources.
    pub sources: Vec<SourceConfig>,
}

/// One content source. M1 supports local directories; git URLs arrive with
/// milestone M3 (PLAN.md §10).
#[derive(Clone, Debug, Deserialize)]
pub struct SourceConfig {
    /// Path to a content source root (the directory holding `antora.yml`),
    /// relative to the playbook's directory.
    pub path: PathBuf,
}

/// The `output` block.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct OutputConfig {
    /// The output directory, relative to the playbook's directory.
    /// Defaults to `build/site`.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

impl Playbook {
    /// Reads and parses the playbook at `path`, recording its directory as
    /// the base for relative paths.
    pub fn load(path: &Path) -> Result<Self, PlaybookError> {
        let text = std::fs::read_to_string(path).map_err(|source| PlaybookError::Io {
            path: path.display().to_string(),
            source,
        })?;

        let mut playbook: Playbook =
            serde_norway::from_str(&text).map_err(|source| PlaybookError::Invalid {
                path: path.display().to_string(),
                source,
            })?;

        playbook.base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(playbook)
    }

    /// The resolved output directory (default `build/site`), relative paths
    /// anchored at the playbook's directory.
    pub fn output_dir(&self) -> PathBuf {
        let dir = self
            .output
            .dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("build/site"));
        if dir.is_absolute() {
            dir
        } else {
            self.base_dir.join(dir)
        }
    }

    /// The resolved path of each content source root, in order.
    pub fn source_roots(&self) -> Vec<PathBuf> {
        self.content
            .sources
            .iter()
            .map(|s| {
                if s.path.is_absolute() {
                    s.path.clone()
                } else {
                    self.base_dir.join(&s.path)
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_playbook() {
        let p: Playbook = serde_norway::from_str(
            "site:\n\
             \x20 title: AsciiDoc HTML5\n\
             \x20 start_page: html5::index.adoc\n\
             content:\n\
             \x20 sources:\n\
             \x20 - path: docs\n",
        )
        .unwrap();
        assert_eq!(p.site.title, "AsciiDoc HTML5");
        assert_eq!(p.site.start_page.as_deref(), Some("html5::index.adoc"));
        assert_eq!(p.content.sources.len(), 1);
        assert_eq!(p.output.dir, None);
    }
}
