//! Spec-coverage ingestion for the Bokfell documentation site
//! generator.
//!
//! This crate will read per-line spec-coverage data (the generalized
//! `sdd` format), project line statuses onto rendered blocks via
//! source maps, and produce the overlay and rollup model. See
//! `PLAN.md` §9.2 at the workspace root.
