//! The interim per-line coverage input (PLAN.md §9.2's first data
//! contract), kept for pre-computed data.
//!
//! The `sdd` tools emit Codecov-style per-line JSON keyed by source path:
//!
//! ```json
//! { "coverage": { "docs/modules/ROOT/pages/x.adoc": { "9": 1, "12": 0 } } }
//! ```
//!
//! Semantics per non-blank source line: `1` — verified by a test; `0` —
//! normative but uncovered; absent — non-normative prose (or blank). The
//! reader addresses that data by page coordinates and projects it onto
//! the block model ([`PageCoverage::from_legacy_lines`]).

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use bokfell_model::Coords;

use crate::model::{BlockCoverage, BlockState, PageCoverage, SpecBlock};

/// Errors from loading per-line coverage data.
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

/// Per-line coverage of one source file (line numbers are 1-based).
pub type LineCoverage = BTreeMap<u32, LineStatus>;

/// Site-wide per-line coverage data, keyed by the source paths the
/// producing tool used (repository-root-relative, e.g.
/// `docs/modules/ROOT/pages/x.adoc`).
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

/// One content source's per-line coverage, scoped to the component
/// versions that source contributed to the catalog: pages of other
/// components (or other versions of the same component) never read it,
/// so identical prefix-plus-path keys in two sources cannot contaminate
/// each other.
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

impl PageCoverage {
    /// Projects per-line data onto a page's blocks: a block whose page
    /// lines are all verified is `verified`, one with any uncovered line
    /// is `uncovered`, and one whose lines carry no coverage at all is
    /// `non-normative`. (The interim format cannot express the other
    /// states.)
    pub fn from_legacy_lines(lines: &LineCoverage, blocks: &[SpecBlock]) -> Self {
        let resolved = blocks
            .iter()
            .map(|block| {
                let mut saw_verified = false;
                let mut saw_uncovered = false;
                if let Some((start, count)) = block.page_lines {
                    let end = start + count.max(1) - 1;
                    for (_, status) in lines.range(start..=end) {
                        match status {
                            LineStatus::Verified => saw_verified = true,
                            LineStatus::Uncovered => saw_uncovered = true,
                        }
                    }
                }
                let state = match (saw_verified, saw_uncovered) {
                    (true, false) => BlockState::Verified,
                    (_, true) => BlockState::Uncovered,
                    (false, false) => BlockState::NonNormative,
                };
                BlockCoverage {
                    state,
                    context: block.context.clone(),
                    page_lines: block.page_lines,
                    reason: None,
                    tracking: None,
                    claims: Vec::new(),
                }
            })
            .collect();
        PageCoverage::from_blocks(resolved)
    }
}

fn normalize(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use bokfell_model::Family;

    use super::*;
    use crate::model::StructuralKind;

    fn coords(path: &str) -> Coords {
        Coords {
            component: "demo".to_string(),
            version: None,
            module: "ROOT".to_string(),
            family: Family::Page,
            path: path.to_string(),
        }
    }

    fn block(page_lines: Option<(u32, u32)>) -> SpecBlock {
        SpecBlock {
            context: "paragraph".to_string(),
            text: String::new(),
            start_line: page_lines.map_or(0, |(s, _)| s),
            line_count: page_lines.map_or(1, |(_, c)| c),
            page_lines,
            sections: Vec::new(),
            kind: StructuralKind::Prose,
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
    fn block_states_from_lines() {
        let mut data = CoverageData::new();
        data.load_str(
            r#"{ "coverage": {
                "docs/modules/ROOT/pages/p.adoc": { "1": 1, "2": 1, "5": 0, "8": 1, "9": 0 }
            } }"#,
        )
        .unwrap();
        let lines = data.page_lines("docs", &coords("p.adoc")).unwrap();

        // Blocks: lines 1-2 (all verified), 4-5 (uncovered), 7-9 (mixed →
        // uncovered), 11-12 (no coverage → non-normative), and an
        // include-origin block (no page lines → non-normative).
        let blocks = [
            block(Some((1, 2))),
            block(Some((4, 2))),
            block(Some((7, 3))),
            block(Some((11, 2))),
            block(None),
        ];
        let coverage = PageCoverage::from_legacy_lines(lines, &blocks);
        let states: Vec<BlockState> = coverage.blocks.iter().map(|b| b.state).collect();
        assert_eq!(
            states,
            vec![
                BlockState::Verified,
                BlockState::Uncovered,
                BlockState::Uncovered,
                BlockState::NonNormative,
                BlockState::NonNormative,
            ]
        );
        assert_eq!(coverage.counts.verified, 1);
        assert_eq!(coverage.counts.uncovered, 2);
        assert_eq!(coverage.counts.non_normative, 2);
        assert_eq!(coverage.percent_verified(), 33);
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
