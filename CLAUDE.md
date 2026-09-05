# CLAUDE.md

Guidance for working in this repository.

## What this is

**Bokfell** (Old Norse *bókfell*, "book-skin" — the prepared vellum a
manuscript is written on) is a documentation site generator for AsciiDoc: a
single static binary that aggregates content from one or more git
repositories, organizes it into versioned components (Antora's content
model), and publishes a cross-linked static site — plus three capabilities
Antora doesn't have: version/PR diffs, spec-coverage overlays, and live
preview/editing.

**Read [`PLAN.md`](PLAN.md) before any architectural work.** It is the
founding plan: goals and non-goals, the adopted Antora content model, the
lessons taken from Zola, the verified `asciidoc-parser`/`asciidoc-html5`
integration seams, the designs for the three differentiators, and the
milestone order.

A Cargo workspace; directory → package:

- `cli/` — `bokfell`, the **binary** (`build`, `serve`, `diff`, `coverage`,
  `init`).
- `model/` — `bokfell-model`: playbook + `antora.yml` parsing, the
  component/version/module/family model, resource IDs, the content catalog,
  the nav model.
- `aggregate/` — `bokfell-aggregate`: content sources → virtual files (local
  dirs, worktree overlays, cached bare git clones).
- `render/` — `bokfell-render`: catalog-aware conversion via
  `asciidoc-html5` (two-phase parse, site-wide xref resolution, embedded
  rendering, highlighting).
- `theme/` — `bokfell-theme`: templating, built-in default theme, theme-dir
  overrides.
- `diff/` — `bokfell-diff`: AsciiDoc-aware structural diff (kept
  generator-independent).
- `coverage/` — `bokfell-coverage`: spec-coverage ingestion and overlay
  model.
- `serve/` — `bokfell-serve`: dev server (watcher, incremental rebuilds,
  livereload, edit API).
- `search/` — `bokfell-search`: static search-index generation.

Keep `bokfell-diff` free of site-generator dependencies — it is meant to be
usable standalone.

## ⚠️ License boundaries — read before borrowing ANY code

This project is `MIT OR Apache-2.0`. Zola is a primary architectural
reference, and **Zola relicensed from MIT to EUPL-1.2 (a copyleft license)
effective v0.22.0, released January 2026.** That has hard consequences:

- **NEVER copy code from Zola's master branch or any tag ≥ v0.22.0.**
  EUPL-1.2 code cannot be incorporated into this permissively licensed
  project — not as snippets, not adapted, not vendored.
- **Tags ≤ v0.21.0 remain MIT.** Code may be borrowed from those tags
  *only*, with attribution (file header + a NOTICE entry; create `NOTICE`
  on first use). Note that v0.21's serve stack is the older two-port
  hyper + `ws` design — for serve/livereload, learn the architecture from
  master and **reimplement**; lift code only from ≤ v0.21.0 when it
  genuinely matches.
- **`giallo` (Zola's syntax highlighter) is EUPL-1.2 too** (verified on
  crates.io, 2026-09). Do not add it as a dependency and do not vendor it.
  Use `syntect` for build-time highlighting.
- Not affected: **Tera is still MIT** (verified through 2.3.0, 2026-08),
  **minijinja is Apache-2.0**, and the `livereload.js` client Zola embeds is
  MIT (Andrey Tarantsov). **Antora is MPL-2.0** — we adopt its content-model
  *design* and repository file formats, never its code.

When adding any dependency or borrowing any code, check the `license` field
of the exact version on crates.io first. When in doubt, don't copy —
reimplement from the described behavior.

## Conventions

- **Commits:** use [Conventional Commits](https://www.conventionalcommits.org)
  (`feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`, `ci:`,
  `update:`). Keep the subject imperative and scoped, start the description
  with a capital letter, and omit the trailing period — CI enforces this on
  PR titles (see `.commitlintrc.no-scope.yml`). For example,
  `feat(model): Parse the component version descriptor`.
- **Comments:** put a blank line before a code comment (unless it is the
  first line of its block), so the comment visually attaches to the code it
  precedes.
- **Edition:** Rust 2021. **MSRV:** 1.88.0 (matching `asciidoc-parser` and
  `asciidoc-html5`; verified in CI). **License:** `MIT OR Apache-2.0`.
- **Antora compatibility:** repository-side formats (`antora.yml`,
  `nav.adoc`, the module/family layout, resource IDs, URL and version
  semantics) stay Antora-compatible; the playbook is Bokfell's own schema
  with a documented mapping from Antora's keys (PLAN.md §12).
- **Technology decisions** live in PLAN.md §7's table; change them there
  (with evidence) before changing them in code.

## Before every commit

Run these from the workspace root and make sure they pass — CI enforces all
three:

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Format with **nightly** rustfmt: `rustfmt.toml` turns on unstable options
(`wrap_comments`, `imports_granularity`, …) that stable rustfmt silently
ignores, and CI enforces format with `cargo +nightly fmt --all -- --check`.
Run `rustup toolchain add nightly` once if you don't have it.
