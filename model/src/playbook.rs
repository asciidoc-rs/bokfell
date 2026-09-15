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

    /// Spec-coverage settings: what to scan for claims and coverage maps
    /// (RFC 0001 §7).
    #[serde(default)]
    pub coverage: CoverageConfig,

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

    /// Spec-coverage JSON files (the `sdd` tool's Codecov-style output)
    /// describing this source's content, relative to the playbook's
    /// directory (PLAN.md §9.2).
    #[serde(default)]
    pub coverage: Vec<PathBuf>,

    /// The path prefix the coverage files' keys start with. Defaults to
    /// the source's `start_path` (git sources) or `path` (directory
    /// sources) — e.g. keys like `docs/modules/ROOT/pages/x.adoc` carry
    /// the prefix `docs`.
    #[serde(default)]
    pub coverage_prefix: Option<String>,
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

/// The `coverage` block (RFC 0001 §7): the spec pages to measure are the
/// playbook's own content sources; this names the test roots and coverage
/// maps to scan against them.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CoverageConfig {
    /// The repositories to scan for `verifies!` claims and sidecars.
    #[serde(default)]
    pub scan: Vec<ScanConfig>,

    /// The click-through URL template for claims, overriding the GitHub
    /// default: `{repo}` (the `owner/name` slug), `{repo_url}`, `{rev}`,
    /// `{path}`, and `{line}` are substituted.
    #[serde(default)]
    pub link_template: Option<String>,

    /// Where `bokfell coverage scan` writes the (ephemeral) coverage
    /// database, relative to the playbook's directory. Defaults to
    /// `build/coverage.json`.
    #[serde(default)]
    pub database: Option<PathBuf>,
}

/// One `coverage.scan` entry: a repository whose test roots carry claims
/// and whose spec-map directory carries coverage-map sidecars. Exactly
/// one of `repo`/`path` must be set. Claims resolve against the pages of
/// the content sources from the same repository (RFC 0001 §4).
#[derive(Clone, Debug, Deserialize)]
pub struct ScanConfig {
    /// Git repository URL, or filesystem path of a local clone (read at
    /// `ref`, like a content source's `url`).
    #[serde(default)]
    pub repo: Option<String>,

    /// A local directory (a worktree, read as is — uncommitted state
    /// included), relative to the playbook's directory.
    #[serde(default)]
    pub path: Option<PathBuf>,

    /// The ref to read test roots and the spec map at (git `repo` only).
    /// Defaults to `HEAD`.
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,

    /// Test roots to scan for `verifies!` invocations, relative to the
    /// repository root.
    #[serde(default)]
    pub tests: Vec<String>,

    /// The coverage-map root (sidecars mirror spec paths under it),
    /// relative to the repository root.
    #[serde(default)]
    pub spec_map: Option<String>,

    /// Limits the measured pages of this repository to those whose
    /// repository-relative path matches one of these globs (`*` within a
    /// segment, `**` across segments). Empty measures every page.
    #[serde(default)]
    pub pages: Vec<String>,
}

impl ScanConfig {
    /// Validates that exactly one of `repo`/`path` is set.
    pub fn validate(&self) -> Result<(), String> {
        match (&self.repo, &self.path) {
            (Some(_), Some(_)) => {
                Err("a coverage.scan entry cannot set both `repo` and `path`".into())
            }
            (None, None) => Err("a coverage.scan entry needs `repo` or `path`".into()),
            _ => Ok(()),
        }
    }

    /// Whether `repo_path` is measured under this entry's `pages` filter.
    pub fn measures(&self, repo_path: &str) -> bool {
        self.pages.is_empty()
            || self
                .pages
                .iter()
                .any(|glob| path_glob_match(glob, repo_path))
    }
}

/// Matches a `/`-separated path against a glob where `*` matches within
/// one segment and `**` matches across segments.
pub fn path_glob_match(pattern: &str, path: &str) -> bool {
    fn inner(p: &[u8], n: &[u8]) -> bool {
        match p {
            [] => n.is_empty(),
            [b'*', b'*', b'/', rest @ ..] => {
                // `**/` matches zero or more whole segments.
                if inner(rest, n) {
                    return true;
                }
                let mut i = 0;
                while i < n.len() {
                    if n[i] == b'/' && inner(rest, &n[i + 1..]) {
                        return true;
                    }
                    i += 1;
                }
                false
            }
            [b'*', b'*', rest @ ..] => (0..=n.len()).any(|i| inner(rest, &n[i..])),
            [b'*', rest @ ..] => {
                (0..=n.len()).any(|i| !n[..i].contains(&b'/') && inner(rest, &n[i..]))
            }
            [c, rest @ ..] => n.first() == Some(c) && inner(rest, &n[1..]),
        }
    }
    inner(
        pattern.trim_start_matches("./").as_bytes(),
        path.trim_start_matches("./").as_bytes(),
    )
}

impl Playbook {
    /// The resolved coverage database path (default `build/coverage.json`).
    pub fn coverage_database(&self) -> PathBuf {
        let path = self
            .coverage
            .database
            .clone()
            .unwrap_or_else(|| PathBuf::from("build/coverage.json"));
        self.resolve_path(&path)
    }

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

    /// The effective coverage-key prefix (see
    /// [`coverage_prefix`](Self::coverage_prefix)).
    pub fn effective_coverage_prefix(&self) -> String {
        if let Some(prefix) = &self.coverage_prefix {
            return prefix.trim_matches('/').to_string();
        }
        if self.url.is_some() {
            return self.start_path.trim_matches('/').to_string();
        }
        self.path
            .as_deref()
            .map(|p| {
                p.to_string_lossy()
                    .replace('\\', "/")
                    .trim_matches('/')
                    .trim_start_matches("./")
                    .to_string()
            })
            .unwrap_or_default()
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
    fn parses_coverage_scan_entries() {
        let yaml = concat!(
            "site:\n",
            "  title: Docs\n",
            "content:\n",
            "  sources:\n",
            "  - path: docs\n",
            "coverage:\n",
            "  scan:\n",
            "  - repo: https://github.com/asciidoc-rs/asciidoc-html5\n",
            "    tests: [html5/src/tests, cli/src/tests]\n",
            "    spec_map: spec-map\n",
            "  - path: .\n",
            "    ref: main\n",
            "    tests: [coverage/tests]\n",
            "    pages: ['docs/modules/rfcs/**']\n",
            "  link_template: 'https://example.org/{repo}/{rev}/{path}#L{line}'\n",
        );
        let p: Playbook = serde_norway::from_str(yaml).unwrap();
        assert_eq!(p.coverage.scan.len(), 2);
        let git = &p.coverage.scan[0];
        git.validate().unwrap();
        assert_eq!(git.tests, ["html5/src/tests", "cli/src/tests"]);
        assert_eq!(git.spec_map.as_deref(), Some("spec-map"));
        assert!(git.measures("anything/at/all.adoc"));
        let local = &p.coverage.scan[1];
        local.validate().unwrap();
        assert_eq!(local.reference.as_deref(), Some("main"));
        assert!(local.measures("docs/modules/rfcs/pages/0001.adoc"));
        assert!(!local.measures("docs/modules/ROOT/pages/index.adoc"));
        assert!(p.coverage.link_template.is_some());
        assert_eq!(p.coverage_database(), PathBuf::from("build/coverage.json"));

        let both = ScanConfig {
            repo: Some("x".into()),
            path: Some("y".into()),
            reference: None,
            tests: Vec::new(),
            spec_map: None,
            pages: Vec::new(),
        };
        assert!(both.validate().is_err());
    }

    #[test]
    fn path_globs() {
        assert!(path_glob_match("docs/**", "docs/modules/ROOT/pages/x.adoc"));
        assert!(path_glob_match(
            "docs/**/pages/*.adoc",
            "docs/modules/ROOT/pages/x.adoc"
        ));
        assert!(path_glob_match(
            "**/x.adoc",
            "docs/modules/ROOT/pages/x.adoc"
        ));
        assert!(path_glob_match("**/x.adoc", "x.adoc"));
        assert!(!path_glob_match(
            "docs/*/x.adoc",
            "docs/modules/ROOT/x.adoc"
        ));
        assert!(path_glob_match("docs/*/x.adoc", "docs/modules/x.adoc"));
        assert!(!path_glob_match("docs/**", "other/x.adoc"));
        assert!(path_glob_match(
            "docs/modules/rfcs/pages/x.adoc",
            "docs/modules/rfcs/pages/x.adoc"
        ));
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
