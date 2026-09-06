//! Spec-coverage ingestion for the Bokfell documentation site generator.
//!
//! The first Bokfell differentiator (PLAN.md §9.2): a site can *show*
//! which parts of its content are verified against an implementation.
//! The data comes from the spec-driven-development workflow used across
//! the asciidoc-rs crates: test modules reproduce a page line-for-line
//! inside `verifies!`/`non_normative!` markers, and the `sdd` tool emits
//! Codecov-style per-line coverage JSON keyed by source path:
//!
//! ```json
//! { "coverage": { "docs/modules/ROOT/pages/x.adoc": { "9": 1, "12": 0 } } }
//! ```
//!
//! Semantics per non-blank source line: `1` — **verified** by a test;
//! `0` — normative but **uncovered**; absent — **non-normative** prose
//! (or blank). This crate reads that data, addresses it by page
//! coordinates, projects line statuses onto a page's blocks via the
//! parser's always-on source spans, and rolls the counts up per page.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use bokfell_model::Coords;

/// Errors from loading coverage data.
#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    /// A coverage file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The coverage file.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A coverage file is not valid coverage JSON.
    #[error("invalid coverage JSON in {path}: {message}")]
    Invalid {
        /// The coverage file.
        path: String,
        /// What was wrong.
        message: String,
    },
}

/// The status of one covered line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineStatus {
    /// Normative and verified by a test.
    Verified,
    /// Normative but not yet verified.
    Uncovered,
}

/// The rolled-up status of one rendered block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus {
    /// Every covered line in the block is verified.
    Verified,
    /// The block mixes verified and uncovered lines.
    Partial,
    /// The block has covered lines and none are verified.
    Uncovered,
}

impl BlockStatus {
    /// The status's CSS token (`verified`/`partial`/`uncovered`).
    pub fn css_token(self) -> &'static str {
        match self {
            BlockStatus::Verified => "verified",
            BlockStatus::Partial => "partial",
            BlockStatus::Uncovered => "uncovered",
        }
    }
}

/// Per-line coverage of one source file (line numbers are 1-based).
pub type LineCoverage = BTreeMap<u32, LineStatus>;

/// Site-wide coverage data, keyed by the source paths the producing tool
/// used (repository-root-relative, e.g. `docs/modules/ROOT/pages/x.adoc`).
#[derive(Debug, Default)]
pub struct CoverageData {
    files: HashMap<String, LineCoverage>,
}

impl CoverageData {
    /// Creates empty coverage data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether no coverage was loaded at all.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Loads one coverage JSON file, merging into the data already held
    /// (on overlap, a `Verified` line wins over `Uncovered`, mirroring the
    /// producing tool's own multi-crate merge).
    pub fn load(&mut self, path: &Path) -> Result<(), CoverageError> {
        let text = std::fs::read_to_string(path).map_err(|source| CoverageError::Io {
            path: path.display().to_string(),
            source,
        })?;
        self.load_str(&text)
            .map_err(|message| CoverageError::Invalid {
                path: path.display().to_string(),
                message,
            })
    }

    /// Loads coverage JSON from a string (see [`load`](Self::load)).
    pub fn load_str(&mut self, text: &str) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let coverage = value
            .get("coverage")
            .and_then(|c| c.as_object())
            .ok_or("missing top-level \"coverage\" object")?;

        for (file, lines) in coverage {
            let lines = lines
                .as_object()
                .ok_or_else(|| format!("{file}: expected a line map"))?;
            let entry = self.files.entry(normalize(file)).or_default();
            for (line, status) in lines {
                let line: u32 = line
                    .parse()
                    .map_err(|_| format!("{file}: bad line number {line:?}"))?;
                let status = match status.as_u64() {
                    Some(0) => LineStatus::Uncovered,
                    Some(1) => LineStatus::Verified,
                    // Anything else is malformed: accepting it would
                    // silently inflate (or deflate) percentages.
                    _ => return Err(format!("{file}: bad status for line {line}")),
                };
                match entry.get(&line) {
                    Some(LineStatus::Verified) => {}
                    _ => {
                        entry.insert(line, status);
                    }
                }
            }
        }
        Ok(())
    }

    /// The line coverage recorded for a page, addressed by its coordinates
    /// plus the `prefix` the producing tool's paths start with (typically
    /// the content source's start path, e.g. `docs`).
    pub fn page_lines(&self, prefix: &str, coords: &Coords) -> Option<&LineCoverage> {
        let family_dir = coords.family.dir_name()?;
        let key = if prefix.is_empty() {
            format!("modules/{}/{}/{}", coords.module, family_dir, coords.path)
        } else {
            format!(
                "{}/modules/{}/{}/{}",
                prefix.trim_end_matches('/'),
                coords.module,
                family_dir,
                coords.path
            )
        };
        self.files.get(&key)
    }
}

/// One content source's coverage, scoped to the component versions that
/// source contributed to the catalog: pages of other components (or other
/// versions of the same component) never read it, so identical
/// prefix-plus-path keys in two sources cannot contaminate each other.
#[derive(Debug, Default)]
pub struct CoverageScope {
    /// The source's merged coverage data.
    pub data: CoverageData,
    /// The path prefix the source's coverage keys start with.
    pub prefix: String,
    /// The `(component name, version)` keys this coverage applies to.
    pub components: Vec<(String, Option<String>)>,
}

impl CoverageScope {
    /// Whether this scope covers the given component version.
    pub fn applies_to(&self, component: &str, version: Option<&str>) -> bool {
        self.components
            .iter()
            .any(|(name, v)| name == component && v.as_deref() == version)
    }
}

/// Coverage of one rendered page.
#[derive(Clone, Debug)]
pub struct PageCoverage {
    /// Count of verified lines.
    pub verified: usize,
    /// Count of normative-but-uncovered lines.
    pub uncovered: usize,
    /// Per-block statuses in document (pre-)order; `None` for a block
    /// whose lines carry no coverage (non-normative content).
    pub blocks: Vec<Option<BlockStatus>>,
}

impl PageCoverage {
    /// The percentage of normative lines that are verified (100 when the
    /// page has no normative lines at all).
    pub fn percent_verified(&self) -> u32 {
        let total = self.verified + self.uncovered;
        (self.verified * 100)
            .checked_div(total)
            .map_or(100, |percent| percent as u32)
    }

    /// Builds page coverage from line data plus the page's blocks, given
    /// as `(start_line, line_count)` spans in document order.
    pub fn from_lines(lines: &LineCoverage, block_spans: &[(u32, u32)]) -> Self {
        let verified = lines
            .values()
            .filter(|s| **s == LineStatus::Verified)
            .count();
        let uncovered = lines.len() - verified;

        let blocks = block_spans
            .iter()
            .map(|(start, count)| {
                let end = start + count.max(&1) - 1;
                let mut saw_verified = false;
                let mut saw_uncovered = false;
                for (_, status) in lines.range(*start..=end) {
                    match status {
                        LineStatus::Verified => saw_verified = true,
                        LineStatus::Uncovered => saw_uncovered = true,
                    }
                }
                match (saw_verified, saw_uncovered) {
                    (true, true) => Some(BlockStatus::Partial),
                    (true, false) => Some(BlockStatus::Verified),
                    (false, true) => Some(BlockStatus::Uncovered),
                    (false, false) => None,
                }
            })
            .collect();

        PageCoverage {
            verified,
            uncovered,
            blocks,
        }
    }
}

fn normalize(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use bokfell_model::Family;

    use super::*;

    fn coords(path: &str) -> Coords {
        Coords {
            component: "demo".to_string(),
            version: None,
            module: "ROOT".to_string(),
            family: Family::Page,
            path: path.to_string(),
        }
    }

    #[test]
    fn loads_and_addresses_by_coords() {
        let mut data = CoverageData::new();
        data.load_str(
            r#"{ "coverage": {
                "docs/modules/ROOT/pages/index.adoc": { "3": 1, "5": 0, "9": 1 }
            } }"#,
        )
        .unwrap();

        let lines = data.page_lines("docs", &coords("index.adoc")).unwrap();
        assert_eq!(lines.get(&3), Some(&LineStatus::Verified));
        assert_eq!(lines.get(&5), Some(&LineStatus::Uncovered));
        assert_eq!(lines.get(&4), None);

        assert!(data.page_lines("docs", &coords("other.adoc")).is_none());
        assert!(data.page_lines("other", &coords("index.adoc")).is_none());
    }

    #[test]
    fn merging_prefers_verified() {
        let mut data = CoverageData::new();
        data.load_str(r#"{ "coverage": { "docs/modules/ROOT/pages/a.adoc": { "1": 0 } } }"#)
            .unwrap();
        data.load_str(r#"{ "coverage": { "docs/modules/ROOT/pages/a.adoc": { "1": 1 } } }"#)
            .unwrap();
        data.load_str(r#"{ "coverage": { "docs/modules/ROOT/pages/a.adoc": { "1": 0 } } }"#)
            .unwrap();

        let lines = data.page_lines("docs", &coords("a.adoc")).unwrap();
        assert_eq!(lines.get(&1), Some(&LineStatus::Verified));
    }

    #[test]
    fn block_statuses_from_spans() {
        let mut data = CoverageData::new();
        data.load_str(
            r#"{ "coverage": {
                "docs/modules/ROOT/pages/p.adoc": { "1": 1, "2": 1, "5": 0, "8": 1, "9": 0 }
            } }"#,
        )
        .unwrap();
        let lines = data.page_lines("docs", &coords("p.adoc")).unwrap();

        // Blocks: lines 1-2 (all verified), 4-5 (uncovered), 7-9 (mixed),
        // 11-12 (no coverage → non-normative).
        let coverage = PageCoverage::from_lines(lines, &[(1, 2), (4, 2), (7, 3), (11, 2)]);
        assert_eq!(
            coverage.blocks,
            vec![
                Some(BlockStatus::Verified),
                Some(BlockStatus::Uncovered),
                Some(BlockStatus::Partial),
                None,
            ]
        );
        assert_eq!(coverage.verified, 3);
        assert_eq!(coverage.uncovered, 2);
        assert_eq!(coverage.percent_verified(), 60);
    }

    #[test]
    fn invalid_json_is_reported() {
        let mut data = CoverageData::new();
        assert!(data.load_str("{}").is_err());
        assert!(data.load_str("not json").is_err());

        // Only 0 and 1 are valid statuses; anything else would silently
        // skew percentages if accepted.
        assert!(data
            .load_str(r#"{ "coverage": { "a.adoc": { "1": 2 } } }"#)
            .is_err());
        assert!(data
            .load_str(r#"{ "coverage": { "a.adoc": { "1": -1 } } }"#)
            .is_err());
    }
}
