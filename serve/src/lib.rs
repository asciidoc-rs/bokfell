//! Development server for the Bokfell documentation site generator.
//!
//! This crate will hold the file watcher, change classification and
//! incremental rebuild orchestration, the HTTP + WebSocket livereload
//! server, and the edit API (click-to-source, write-back). See
//! `PLAN.md` §9.3 at the workspace root.
