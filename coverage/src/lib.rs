//! The spec-coverage engine of the Bokfell documentation site generator
//! (RFC 0001, `docs/modules/rfcs/pages/0001-spec-coverage.adoc`).
//!
//! A site can *show* which parts of its content are verified against an
//! implementation (PLAN.md §9.2). The engine separates two facts:
//!
//! * **Claims** live in test code — any test function, in any file, in any
//!   crate or repository, claims specific blocks of a spec page by quoting an
//!   excerpt inside a no-op `verifies!` marker ([`scan_test_root`]).
//! * **Classification** lives with the implementation, in a reviewed per-page
//!   sidecar (the coverage map, [`Sidecar`]): what is non-normative,
//!   deliberately out of scope, or planned.
//!
//! The unit of coverage is the block ([`SpecBlock`]); the render
//! pipeline supplies each measured page's blocks, [`resolve`] turns
//! claims and sidecar entries into one [`BlockState`] per block with
//! provenance and diagnostics, and [`report`] rolls the result up (table,
//! JSON, Codecov projection).
//!
//! The interim per-line input format is still read ([`CoverageData`]) so
//! pre-computed data keeps working.

mod claims;
mod legacy;
mod model;
pub mod report;
mod resolve;
mod sidecar;
mod text;

pub use claims::{scan_source, scan_test_root, ScanError, TestRoot};
pub use legacy::{CoverageData, CoverageError, CoverageScope, LineCoverage, LineStatus};
pub use model::{
    BlockCoverage, BlockState, Claim, ClaimSite, ClaimTarget, MeasuredPage, PageCoverage,
    PageRecord, SpecBlock, StateCounts, StructuralKind, Tracking,
};
pub use resolve::{
    claims_by_site, pages_by_component, path_matches, resolve, ComponentRollup, CoverageDatabase,
    Diagnostic, DiagnosticKind,
};
pub use sidecar::{load_spec_map, Sidecar, SidecarEntry, SidecarError, SidecarKind};
pub use text::{excerpt_matches, normalize_whitespace};
