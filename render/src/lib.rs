//! Catalog-aware AsciiDoc conversion for the Bokfell documentation
//! site generator.
//!
//! This crate will drive the two-phase parse (deferred parse of all
//! pages, then site-wide reference resolution over the content
//! catalog), embedded rendering via `asciidoc-html5`, build-time
//! syntax highlighting, and include resolution through the catalog.
//! See `PLAN.md` §6 and §8 at the workspace root.
