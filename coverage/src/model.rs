//! The block-level coverage model (RFC 0001 §2–§3).
//!
//! The unit of coverage is the *block*: every measured page is a sequence
//! of [`SpecBlock`]s (the render pipeline's overlay blocks, so shading and
//! coverage agree), and each block resolves to exactly one [`BlockState`].

use std::path::PathBuf;

use bokfell_model::Coords;
use serde::{Deserialize, Serialize};

/// The state of one block of a measured page (RFC 0001 §3).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockState {
    /// At least one claim resolves to this block.
    Verified,
    /// Not implemented yet; carries a tracking link.
    Planned,
    /// Normative, no claim, no annotation.
    Uncovered,
    /// The heuristics could not decide and no human has.
    Unclassified,
    /// Deliberately not implemented; carries a reason.
    OutOfScope,
    /// Describes rather than specifies.
    NonNormative,
}

impl BlockState {
    /// Every state, in dashboard order (the denominator states first).
    pub const ALL: [BlockState; 6] = [
        BlockState::Verified,
        BlockState::Planned,
        BlockState::Uncovered,
        BlockState::Unclassified,
        BlockState::OutOfScope,
        BlockState::NonNormative,
    ];

    /// The state's token, used as its CSS `data-coverage` value and in
    /// JSON output (`verified`, `planned`, `uncovered`, `unclassified`,
    /// `out-of-scope`, `non-normative`).
    pub fn token(self) -> &'static str {
        match self {
            BlockState::Verified => "verified",
            BlockState::Planned => "planned",
            BlockState::Uncovered => "uncovered",
            BlockState::Unclassified => "unclassified",
            BlockState::OutOfScope => "out-of-scope",
            BlockState::NonNormative => "non-normative",
        }
    }

    /// Whether the state counts in the headline percentage's denominator.
    pub fn in_denominator(self) -> bool {
        matches!(
            self,
            BlockState::Verified
                | BlockState::Planned
                | BlockState::Uncovered
                | BlockState::Unclassified
        )
    }
}

impl std::fmt::Display for BlockState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.token())
    }
}

/// What the structural heuristic says about a block no sidecar entry and
/// no claim touches (RFC 0001 §5, "structural defaults").
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StructuralKind {
    /// Prose that may carry a rule: paragraphs, admonitions, tables,
    /// lists, and the like. `unclassified` on unreviewed pages,
    /// normative (`uncovered` until claimed) on reviewed ones.
    Prose,
    /// Content that illustrates rather than specifies: example and
    /// listing blocks, images, and media. `non-normative` by default.
    NonNormative,
}

/// One block of a measured page, as the resolver sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpecBlock {
    /// The block's resolved context (`paragraph`, `listing`, `table`,
    /// `list`, …).
    pub context: String,
    /// The block's source text — what excerpts match against, after
    /// whitespace normalization.
    pub text: String,
    /// The 1-based line the block starts at in the preprocessed source.
    pub start_line: u32,
    /// The number of preprocessed source lines the block spans.
    pub line_count: u32,
    /// The block's `(start, count)` line span in the page's *own* source
    /// file, when it originates there; `None` for content spliced in by
    /// an include. Drives the Codecov projection.
    pub page_lines: Option<(u32, u32)>,
    /// The IDs of the sections enclosing the block, outermost first.
    pub sections: Vec<String>,
    /// The structural heuristic's classification.
    pub kind: StructuralKind,
}

/// One page the coverage engine measures.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MeasuredPage {
    /// The page's catalog coordinates.
    pub coords: Coords,
    /// The page's site-root-relative URL.
    pub url: String,
    /// The page's path within its repository (`/`-separated, e.g.
    /// `docs/modules/ROOT/pages/index.adoc`) — what claim paths
    /// suffix-match against, and the Codecov export key.
    pub repo_path: String,
    /// The index of the `coverage.scan` entry the page's repository
    /// belongs to: claims resolve within their own scope.
    pub scope: usize,
    /// The page's blocks, in document order.
    pub blocks: Vec<SpecBlock>,
    /// Every section ID on the page.
    pub section_ids: Vec<String>,
}

/// Where a claim was found: the provenance the click-through renders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimSite {
    /// The Rust file, relative to its repository root (`/`-separated).
    pub file: String,
    /// The file's absolute path, when the test root is a local directory
    /// (serve mode's "open test" affordance); `None` for cache-backed
    /// or remote roots, which are never exposed to the editor.
    pub local_path: Option<PathBuf>,
    /// The 1-based line of the `verifies!` invocation.
    pub line: u32,
    /// The enclosing test function's name, when the invocation sits in
    /// one.
    pub test_fn: Option<String>,
    /// The enclosing crate's package name, when a `Cargo.toml` was found.
    pub krate: Option<String>,
    /// The repository the file came from (URL or local path as
    /// configured).
    pub repo: Option<String>,
    /// The repository revision the file was read at.
    pub rev: Option<String>,
    /// The `coverage.scan` entry index the test root belongs to.
    pub scope: usize,
}

/// A claim's page target (RFC 0001 §4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClaimTarget {
    /// A path as written in the scanned repository, suffix-matched
    /// against the pages of the same scope.
    Path(String),
    /// A full Antora-style resource ID (`component:module:page.adoc`) for
    /// a cross-repository claim.
    ResourceId(String),
}

/// One `verifies!` claim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    /// The page the claim targets.
    pub target: ClaimTarget,
    /// The `#anchor` section scope, if any.
    pub anchor: Option<String>,
    /// The excerpt; `None` claims the whole section named by `anchor`.
    pub excerpt: Option<String>,
    /// Where the claim was found.
    pub site: ClaimSite,
}

impl Claim {
    /// The claim as the user wrote its target: `path`, `path#anchor`.
    pub fn target_display(&self) -> String {
        let base = match &self.target {
            ClaimTarget::Path(path) | ClaimTarget::ResourceId(path) => path.as_str(),
        };
        match &self.anchor {
            Some(anchor) => format!("{base}#{anchor}"),
            None => base.to_string(),
        }
    }

    /// A short human label for the claim's provenance:
    /// `crate::test_fn` (or `file:line` when no function encloses it).
    pub fn label(&self) -> String {
        match (&self.site.krate, &self.site.test_fn) {
            (Some(krate), Some(test_fn)) => format!("{krate}::{test_fn}"),
            (None, Some(test_fn)) => test_fn.clone(),
            _ => format!("{}:{}", self.site.file, self.site.line),
        }
    }
}

/// A tracking reference of a `planned` entry: `owner/repo#N` shorthand
/// or a full URL.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tracking {
    /// The reference as written.
    pub raw: String,
    /// The URL it links to.
    pub url: String,
}

impl Tracking {
    /// Parses a tracking reference: a URL is kept as is, and
    /// `owner/repo#N` templates to the GitHub issue URL.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        let url = if raw.contains("://") {
            raw.to_string()
        } else if let Some((repo, number)) = raw.split_once('#') {
            if repo.split('/').count() == 2 && number.chars().all(|c| c.is_ascii_digit()) {
                format!("https://github.com/{repo}/issues/{number}")
            } else {
                raw.to_string()
            }
        } else {
            raw.to_string()
        };
        Tracking {
            raw: raw.to_string(),
            url,
        }
    }
}

/// The resolved coverage of one block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockCoverage {
    /// The block's state.
    pub state: BlockState,
    /// The block's resolved context, for display.
    pub context: String,
    /// The block's line span in the page's own file (see
    /// [`SpecBlock::page_lines`]).
    pub page_lines: Option<(u32, u32)>,
    /// The recorded reason of an `out-of-scope` block.
    pub reason: Option<String>,
    /// The tracking link of a `planned` block.
    pub tracking: Option<Tracking>,
    /// Indexes into the database's claim list of every claim resolving
    /// to this block.
    pub claims: Vec<usize>,
}

/// Per-state block counts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateCounts {
    /// Verified blocks.
    pub verified: usize,
    /// Planned blocks.
    pub planned: usize,
    /// Uncovered blocks.
    pub uncovered: usize,
    /// Unclassified blocks.
    pub unclassified: usize,
    /// Out-of-scope blocks (outside the denominator).
    pub out_of_scope: usize,
    /// Non-normative blocks (outside the denominator).
    pub non_normative: usize,
}

impl StateCounts {
    /// Counts one more block in `state`.
    pub fn add(&mut self, state: BlockState) {
        *self.slot(state) += 1;
    }

    /// Adds every count of `other`.
    pub fn add_counts(&mut self, other: &StateCounts) {
        for state in BlockState::ALL {
            *self.slot(state) += other.get(state);
        }
    }

    /// The count for one state.
    pub fn get(&self, state: BlockState) -> usize {
        match state {
            BlockState::Verified => self.verified,
            BlockState::Planned => self.planned,
            BlockState::Uncovered => self.uncovered,
            BlockState::Unclassified => self.unclassified,
            BlockState::OutOfScope => self.out_of_scope,
            BlockState::NonNormative => self.non_normative,
        }
    }

    fn slot(&mut self, state: BlockState) -> &mut usize {
        match state {
            BlockState::Verified => &mut self.verified,
            BlockState::Planned => &mut self.planned,
            BlockState::Uncovered => &mut self.uncovered,
            BlockState::Unclassified => &mut self.unclassified,
            BlockState::OutOfScope => &mut self.out_of_scope,
            BlockState::NonNormative => &mut self.non_normative,
        }
    }

    /// The headline denominator: `verified + planned + uncovered +
    /// unclassified`.
    pub fn denominator(&self) -> usize {
        self.verified + self.planned + self.uncovered + self.unclassified
    }

    /// The headline metric, `verified / denominator` as a percentage (100
    /// when the denominator is empty).
    pub fn percent_verified(&self) -> u32 {
        (self.verified * 100)
            .checked_div(self.denominator())
            .map_or(100, |percent| percent as u32)
    }
}

/// The resolved coverage of one page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageCoverage {
    /// Per-block coverage, in document (overlay) order.
    pub blocks: Vec<BlockCoverage>,
    /// The page's per-state counts.
    pub counts: StateCounts,
}

impl PageCoverage {
    /// Builds page coverage from resolved blocks, tallying the counts.
    pub fn from_blocks(blocks: Vec<BlockCoverage>) -> Self {
        let mut counts = StateCounts::default();
        for block in &blocks {
            counts.add(block.state);
        }
        PageCoverage { blocks, counts }
    }

    /// The page's headline percentage.
    pub fn percent_verified(&self) -> u32 {
        self.counts.percent_verified()
    }
}

/// One measured page's record in the coverage database.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageRecord {
    /// The page's coordinates.
    pub coords: Coords,
    /// The page's site-root-relative URL.
    pub url: String,
    /// The page's repository-relative path.
    pub repo_path: String,
    /// The page's resolved coverage.
    pub coverage: PageCoverage,
}
