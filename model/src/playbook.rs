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

    /// Runtime settings (cache, fetch policy).
    #[serde(default)]
    pub runtime: RuntimeConfig,

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

/// One content source: either a plain local directory (`path`) or a git
/// repository (`url` plus ref patterns). Exactly one of `path`/`url` must
/// be set.
#[derive(Clone, Debug, Deserialize)]
pub struct SourceConfig {
    /// Path to a content source root (the directory holding `antora.yml`),
    /// relative to the playbook's directory.
    #[serde(default)]
    pub path: Option<PathBuf>,

    /// Git repository URL, or filesystem path of a local clone/worktree
    /// (opened in place; relative paths resolve against the playbook's
    /// directory).
    #[serde(default)]
    pub url: Option<String>,

    /// Branch name patterns (`*` glob, `!` negation, `HEAD` = current
    /// branch). Defaults to `[HEAD]` when `tags` is empty too.
    #[serde(default)]
    pub branches: Vec<String>,

    /// Tag name patterns (`*` glob, `!` negation).
    #[serde(default)]
    pub tags: Vec<String>,

    /// Path of the content root within the repository (empty = repository
    /// root). Only meaningful with `url`.
    #[serde(default)]
    pub start_path: String,

    /// Derive each matched ref's component version from the ref name
    /// instead of `antora.yml` (needed when several refs carry the same
    /// descriptor). Only meaningful with `url`.
    #[serde(default)]
    pub version_from_ref: bool,
}

/// The `output` block.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct OutputConfig {
    /// The output directory, relative to the playbook's directory.
    /// Defaults to `build/site`.
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

/// The `runtime` block.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RuntimeConfig {
    /// Cache directory for cloned repositories and ref exports. Defaults
    /// to the platform cache directory (`~/.cache/bokfell`).
    #[serde(default)]
    pub cache_dir: Option<PathBuf>,

    /// Refresh already-cached remote repositories on every build.
    #[serde(default)]
    pub fetch: bool,
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

    /// Resolves a path from the playbook file's directory.
    pub fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.base_dir.join(path)
        }
    }
}

impl SourceConfig {
    /// Whether this source is a git source (`url`) rather than a plain
    /// directory (`path`).
    pub fn is_git(&self) -> bool {
        self.url.is_some()
    }

    /// Validates that exactly one of `path`/`url` is set.
    pub fn validate(&self) -> Result<(), String> {
        match (&self.path, &self.url) {
            (Some(_), Some(_)) => Err("a content source cannot set both `path` and `url`".into()),
            (None, None) => Err("a content source needs `path` or `url`".into()),
            _ => Ok(()),
        }
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
        assert!(!p.content.sources[0].is_git());
        p.content.sources[0].validate().unwrap();
    }

    #[test]
    fn parses_git_source() {
        let yaml = concat!(
            "site:\n",
            "  title: Docs\n",
            "content:\n",
            "  sources:\n",
            "  - url: https://github.com/asciidoc-rs/asciidoc-html5\n",
            "    branches: [main]\n",
            "    tags: ['asciidoc-html5-v*']\n",
            "    start_path: docs\n",
            "    version_from_ref: true\n",
            "runtime:\n",
            "  fetch: true\n",
        );
        let p: Playbook = serde_norway::from_str(yaml).unwrap();
        let source = &p.content.sources[0];
        assert!(source.is_git());
        source.validate().unwrap();
        assert_eq!(source.branches, ["main"]);
        assert_eq!(source.start_path, "docs");
        assert!(source.version_from_ref);
        assert!(p.runtime.fetch);
    }

    #[test]
    fn rejects_ambiguous_source() {
        let yaml = concat!(
            "site:\n",
            "  title: Docs\n",
            "content:\n",
            "  sources:\n",
            "  - path: docs\n",
            "    url: https://example.org/repo\n",
        );
        let p: Playbook = serde_norway::from_str(yaml).unwrap();
        assert!(p.content.sources[0].validate().is_err());
    }
}
