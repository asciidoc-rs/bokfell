//! AsciiDoc-aware structural diff.
//!
//! This crate will diff two parsed documents at the block level
//! (alignment, added/removed/moved/edited classification, inline
//! word-diff of changed content) and emit annotated HTML. It is
//! deliberately independent of the site generator so it can back
//! other tools. See `PLAN.md` §9.1 at the workspace root.
