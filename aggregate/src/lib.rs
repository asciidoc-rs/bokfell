//! Content aggregation for the Bokfell documentation site generator.
//!
//! This crate will map content sources to virtual files: local
//! directories, worktree overlays, and gix-based cached bare clones,
//! with branch/tag pattern matching and per-file origin metadata. See
//! `PLAN.md` §6 at the workspace root.
