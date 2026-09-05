# Plan: an Antora-class documentation site generator in Rust

> Status: founding plan, moved here from `asciidoc-rs/asciidoc-html5` where
> the initial discussion took place. This document evolves into
> `ARCHITECTURE.md` as milestones land; until then it is the source of truth
> for goals, architecture, and decisions.

## 1. Summary

The next stage of the asciidoc-rs project is a **documentation site
generator**: a single static binary that aggregates AsciiDoc content from one
or more git repositories, organizes it into versioned components, and publishes
a cross-linked, navigable static site — the job [Antora](https://antora.org)
does for the Asciidoctor ecosystem — built on `asciidoc-parser` and
`asciidoc-html5`.

It adopts Antora's proven **content model** (components, versions, modules,
families, resource IDs) and repository-side file formats (`antora.yml`,
`nav.adoc`, the standard directory set), while replacing Antora's Node.js
pipeline, Handlebars UI bundles, and Asciidoctor.js converter with a Rust
engine. On top of that baseline it adds three capabilities Antora does not
have:

1. **Version-to-version and PR diffs** — rendered pages that highlight what
   changed in a spec/document between published versions, or in a proposed
   update on a branch/pull request.
2. **Spec coverage overlays** — surfacing which portions of a spec are
   tested/verified/known-good, generalizing the spec-driven-development (SDD)
   marker system already used in `asciidoc-parser` and `asciidoc-html5`.
3. **Live preview and editing** — a built-in dev server with file watching,
   incremental rebuilds, and browser live-reload (in the spirit of
   `zola serve`), extended with click-to-source editing.

## 2. Goals and non-goals

**Goals**

- One `build` command from a playbook to a complete static site: multi-repo,
  multi-version, cross-referenced, with navigation, redirects, sitemap, and
  search.
- A single, dependency-free binary (`cargo install` / prebuilt releases). No
  Node, npm, or Ruby anywhere in the pipeline.
- Dogfooding from the first milestone: replace the `antora-playbook-local.yml`
  + npm toolchain that currently builds the `asciidoc-html5` repo's `docs/`
  component, and
  eventually build the whole asciidoc-rs documentation site.
- Fast enough that watch-mode rebuilds feel instant on a large site
  (parallel conversion, incremental rebuilds keyed on the dependency graph).
- The three differentiators above as first-class subsystems, not bolt-ons.

**Non-goals**

- Not a general-purpose SSG (no Markdown, no blogging/taxonomy features —
  Zola, Hugo, and mdBook cover that space). AsciiDoc documentation sites only.
- No compatibility with Antora **UI bundles** (Handlebars zips) or Antora
  **pipeline extensions** (Node modules). Content repositories are compatible;
  the site-level toolchain is not.
- No WYSIWYG editor. "Live editing" means fast source-level round-trips
  (watch, reload, click-to-source), not a rich-text control.
- Not a hosted service; output is static files plus an optional local dev
  server.

## 3. Name

**The project is named Bokfell** (Old Norse *bókfell*, "book-skin" — the
prepared vellum a manuscript is written on; the surface the sagas survive
on). Availability verified 2026-09-05: `bokfell` is unregistered on
crates.io, npm, and PyPI, and web searches surface no software project,
product, or organization using the name. Repo: `asciidoc-rs/bokfell`;
binary and crate prefix: `bokfell`.

Naming history: "Ascribe" was dropped as already in use in software
development. Two vetted rounds of alternatives preceded Bokfell; the
shortlist runner-up was `docent` (free on crates.io; a docent guides
visitors through a collection). Checked and rejected: pecia, quair,
vademecum, uncial, scrivano, emend, fascicle, cicerone (free in part, but
weaker fits or partial registry collisions); colophon, exemplar,
palimpsest, scriptorium, quire, octavo, ductus, stemma (crates.io taken);
variorum (LLNL HPC project), bindery (bindery.info), chapbook (Twine 2
story format), adscript (a programming language), skribo (dormant Rust
text-layout crate); quarto, foliant, vellum (established doc tools).

## 4. One project, or components first?

**Recommendation: a single new repository**, structured as a Cargo workspace of
focused crates (§6), with no independent projects built beforehand. Rationale:

- The heavy prerequisites already exist as products of `asciidoc-html5` and
  `asciidoc-parser`: parsing, inline substitution, block rendering with an
  embedded (body-only) output mode, document catalogs, always-on source maps,
  include resolution with safe-mode jails, and an `AssetWriter` seam for file
  side effects. The generator is a *consumer* of these crates.
- The pieces that are new — content aggregation, the site catalog, theming, the
  dev server, diffing, coverage overlays — are only meaningful in the
  generator's context. Splitting them into standalone projects would create
  version-coordination overhead with no independent users yet. A workspace
  gives the same modularity (separately publishable crates) inside one repo,
  one issue tracker, one CI.
- The one component with plausible standalone value is the **AsciiDoc-aware
  diff engine** (diff two documents → annotated HTML). It should be its own
  *crate* in the workspace so it can be published and consumed independently
  (e.g. by a future CLI `adoc diff`), but it does not need its own repo.
- Upstream gaps discovered along the way (e.g. finer-grained inline source
  spans) are filed as issues on `asciidoc-parser`/`asciidoc-html5`, not spun
  into new projects. The critical seams were verified to exist already —
  see §8.

## 5. What we take from Antora, and what we deliberately change

Antora (Node.js, MPL-2.0, `@antora/*` packages) runs a linear pipeline:
playbook building → content aggregation (isomorphic-git, bare no-checkout
clones into a cache) → content classification into a **ContentCatalog** of
virtual files with 5-coordinate resource IDs → document conversion
(Asciidoctor.js with catalog-aware include/xref resolution) → navigation
building (from `nav.adoc` lists) → page composition (Handlebars UI bundle) →
redirects → sitemap → publishing. Everything between aggregation and publish
operates on in-memory virtual files.

### Adopted (repository-side compatibility)

Content repositories that work with Antora should work with this generator
unchanged:

- **The content model**: components and component versions; modules (with
  `ROOT`); families (`pages/`, `partials/`, `images/`, `attachments/`,
  `examples/`); the standard directory set rooted at `antora.yml`; distributed
  component versions assembled from multiple repos/branches/start paths.
- **`antora.yml`** as the component-version descriptor: `name`, `version`
  (incl. `~` for versionless), `title`, `display_version`, `prerelease`,
  `nav`, `start_page`, `asciidoc.attributes`.
- **Resource IDs and xrefs**: the
  `version@component:module:family$path` coordinate syntax, family defaults
  per context, latest-version resolution when the version coordinate is
  omitted.
- **`nav.adoc`** navigation files (unordered lists of xrefs, registered in
  `antora.yml`, order = menu order).
- **URL construction**: `/<component>/<version>/<module>/<page>.html` with the
  established segment-dropping rules (ROOT component/module, versionless
  components), `_images/`/`_attachments/` family segments, and the
  latest-version symbolic segment options (`stable`/`current`, redirect
  strategies).
- **Version semantics**: the sorting rules (versionless → named → semver
  descending), "latest = first non-prerelease", prerelease identifiers.
- **Playbook concepts**: site keys, content sources with branch/tag patterns,
  start paths, edit-URL patterns, worktree support. The playbook will be our
  own schema (it must describe diff/coverage/preview configuration that Antora
  has no notion of), but it will accept-or-translate the core Antora playbook
  keys so existing playbooks migrate mechanically. *(Decided; see §12.)*

### Replaced

- **Runtime**: Node.js + Asciidoctor.js (Opal-transpiled Ruby) → one Rust
  binary using `asciidoc-parser`/`asciidoc-html5`.
- **Git**: isomorphic-git (pure-JS, slow, memory-hungry on large repos — the
  `fetch_concurrency`/`read_concurrency` knobs exist because of it) → `gix`
  (gitoxide), reading files from bare clones without checkouts, same cache
  model.
- **UI bundles**: remote-fetched Handlebars zips with a gulp toolchain →
  a built-in default theme compiled into the binary, overridable per-file by a
  local theme directory (the moral equivalent of Antora's
  `supplemental_files`, minus the fork-and-host-a-zip workflow). Templating
  via a Rust engine (minijinja; see §7 technology notes).
- **Extensions**: Antora's Node pipeline-event extensions → initially, *no*
  runtime plugin system. The workspace crates expose the pipeline as a library
  (like Antora's replaceable generator functions, but typed), so bespoke
  generators are Rust programs; a scripting/plugin story is deliberately
  deferred. AsciiDoc *syntax* extension points remain whatever
  `asciidoc-parser` offers.

### Antora pain points this project fixes by design

From community retrospectives and Antora's own tracker (notably
antora#540 "Support editor tooling and preview"):

1. **No watch mode / live preview / incremental build in core** — our §9.3 is
   a core subsystem, not an extension.
2. **The author-mode problem** — previewing unpushed, multi-repo changes in
   Antora requires a hand-maintained second playbook plus worktree gymnastics.
   `bokfell serve` treats "this worktree, uncommitted state" as a first-class
   source overlay, and the PR-diff mode (§9.1) makes "what did my branch
   change" a rendered artifact.
3. **Node.js install friction and supply-chain surface** — single binary.
4. **UI customization DX** — no forked zip bundles; a theme is files in a
   directory.
5. **No built-in search** — ship static search-index generation in core.
6. **Performance/memory on large multi-version sites** — native git, parallel
   conversion (rayon), incremental rebuilds.

## 6. Architecture: workspace layout

One repo, one Cargo workspace, publishable crates prefixed with the project
name (shown here as `bokfell-*`):

```
bokfell/            CLI binary: build, serve, diff, coverage, init
bokfell-model/      playbook + antora.yml parsing; component/version/module/
                    family model; resource IDs; the ContentCatalog (virtual
                    files, out-paths, URLs); nav model
bokfell-aggregate/  content sources → virtual files: local dirs, worktree
                    overlays, gix-based bare clones + cache, branch/tag
                    pattern matching, origin metadata
bokfell-render/     catalog-aware conversion: deferred parse of all pages,
                    merged reference resolution (ReferenceResolver over the
                    ContentCatalog), asciidoc-html5 embedded rendering,
                    syntect server-side highlighting, include resolution
                    through the catalog
bokfell-theme/      minijinja templating, built-in default theme, theme-dir
                    overrides, page/site template model, nav/breadcrumb/
                    version-selector composition
bokfell-diff/       AsciiDoc-aware structural diff (standalone value): block
                    alignment + classification, inline word-diff,
                    annotated-HTML emission
bokfell-coverage/   SDD coverage ingestion (per-line JSON), line→block
                    mapping via source maps, overlay + rollup model
bokfell-serve/      dev server: notify watcher, debounce, incremental rebuild
                    orchestration, HTTP + WebSocket livereload, edit API
                    (click-to-source, write-back)
bokfell-search/     static search index generation + client JS asset
```

### The pipeline

Mirrors Antora's stages, as typed functions over shared catalogs (each stage a
library API, so `serve` can re-run only what a change invalidates):

```
playbook ─► aggregate ─► classify ─► parse (deferred) ─► resolve xrefs ─► convert ─► compose ─► publish
              (git/fs)    ContentCatalog   all pages        site-wide          HTML     theme      out dir
                                            │                catalog            │        │
                                            └── nav build ◄──────────────┘        ├─ redirects, sitemap,
                                                                                  │  404, search index
                                                                                  └─ diff + coverage views
```

Conversion parallelizes per page with rayon; the parse/resolve split uses
`asciidoc-parser`'s two-phase API (§8) so cross-page references resolve against
the whole site before any page renders.

## 7. What we take from Zola

Zola is not an architectural template — it is single-repo,
Markdown/CommonMark, unversioned — but it is the best existing evidence that a
one-binary Rust SSG with fast serve/reload works. Findings from a source
review of Zola master (v0.23.4 era) and the v0.21.0 tag:

- **The serve/livereload stack ports directly** (Zola's most reusable
  subsystem, `src/cmd/serve.rs` + `src/fs_utils.rs`): `notify-debouncer-full`
  file watching (default 1 s debounce, per-root Required/Optional watch
  modes), an event-filtering layer full of hard-won per-platform/per-editor
  cases (atomic saves, temp files, rename-event zoo), an embedded axum server
  with a WebSocket endpoint hand-speaking the LiveReload protocol to an
  embedded `livereload.js` (itself MIT), a broadcast channel fanning reloads
  to all clients, CSS hot-swap without full reload, and a middleware that
  injects a build-error overlay into served pages so errors surface in the
  browser. Rendered output is served **memory-first** (a path→String map)
  with disk fallback for static assets.
- **Rebuild classification, and its cautionary tale.** Zola bins changes by
  path (content / templates / sass / static / config) and handles each
  differently — but a *content* change still rebuilds the whole site by
  default, because its opt-in partial mode (`--fast`) is unsound: without
  dependency tracking, section listings, backlinks, and search indexes go
  stale (their issue #757), and one template that leaks into the markdown
  phase forces a hardcoded full-rebuild list (#2940). The lesson for
  `bokfell-serve`: "fast full rebuild + cheap special cases" is a defensible
  v1, and *correct* partial rebuilds require the real dependency graph — 
  which our ContentCatalog (includes, xrefs, images per page) exists to
  provide.
- **Within-build performance techniques**: rayon `par_iter` over parse,
  convert, and a flat render-job queue; pre-serializing shared template
  context once per build (Zola's `RenderCache`) instead of per page.
- **Internal link validation**: collect internal links + heading anchors
  during rendering, then validate in one pass with warn-vs-error levels —
  exactly the shape site-wide xref/anchor validation should take.
- **Anchor/slug policy**: three-way slugify strategy, duplicate-anchor
  suffixing, explicit-id overrides — policies to mirror (Asciidoctor
  compatibility permitting), not code to port.
- **Search**: build-time static index (elasticlunr-rs, plus a trivially
  reimplementable Fuse.js JSON format), HTML cleaned to text via ammonia
  with `script`/`style`/`pre` content dropped. The shape (static index +
  small client JS) is right; the format is an open evaluation (§12).
- **What does not transfer**: the markdown pipeline is an 895-line
  pulldown-cmark event-stream rewriter (an AST-based AsciiDoc pipeline
  should avoid, not port, its sentinel/regex hacks); the content model
  assumes Zola's filesystem conventions; serve state lives in global statics
  (a site generator that aggregates many repos should own its state); and
  Zola's component crates are unpublished, so reuse means vendoring, not
  `cargo add`.

**License note — important.** Zola relicensed from MIT to **EUPL-1.2
(copyleft)** effective v0.22.0 (January 2026). Code may **not** be copied
from Zola master into this `MIT OR Apache-2.0` project. The v0.21.0 tag and
earlier remain MIT — but v0.21's serve stack is the older two-port
hyper + `ws` design. Practical rule: learn the architecture from master and
reimplement, borrow code (with attribution) only from ≤ v0.21.0. The
relicensing also covers `giallo` (EUPL-1.2 on crates.io), which rules it
out even as a dependency; it does **not** extend to Tera (still MIT through
2.3.0, 2026-08) or to the MIT `livereload.js` client Zola embeds. See
CLAUDE.md's license-boundaries section, which restates these rules for
contributors.

**Technology choices** (initial; each is a workspace-level decision recorded
here, revisited only with evidence):

| Concern | Choice | Rationale |
|---|---|---|
| Git | `gix` | Native-speed bare clones/reads, pure Rust, active; avoids shelling out to git |
| Templates | `minijinja` (evaluate Tera 2) | Both are maintained Jinja2-family engines in 2026. minijinja: minimal deps, years-stable API. Tera 2 (a from-scratch rewrite Zola itself drove, stable mid-2026): built-in glob loading and a hygienic *components* feature that suits page composition, but a young 2.x API. Decide at M0 — the default theme is written against the winner |
| Watcher | `notify-debouncer-full` (`notify`) | Ecosystem standard (Zola, cargo-watch); Zola's event-filtering layer maps the platform quirks to copy |
| HTTP/WS | `axum` (dev server only) | What Zola converged on for serve+WS on one port; only `bokfell-serve` depends on it |
| Highlighting | `syntect` | Build-time, no client JS. `giallo` (the TextMate-grammar highlighter Zola moved to in 0.22) was evaluated and ruled out: it is EUPL-1.2 (verified on crates.io, 2026-09), incompatible as a dependency of this MIT OR Apache-2.0 project |
| Parallelism | `rayon` | Per-page conversion, matches Zola's model |
| Diff primitives | `similar` | Word/line diffs for changed inline content |

## 8. Relationship to `asciidoc-parser` / `asciidoc-html5` (verified seams)

The generator consumes the two existing crates through APIs that already
exist — verified against `asciidoc-parser` 0.29.19 and `asciidoc-html5`
0.1.7:

- **Two-phase parse and site-wide reference resolution**:
  `Parser::parse_deferred` + `Document::resolve_references(resolver, …)`,
  where `ReferenceResolver` is a public trait and path-bearing xref targets
  (`other-page.adoc#frag`) are deliberately left to a custom resolver.
  `bokfell-render` implements `ReferenceResolver` over the ContentCatalog,
  translating resource IDs into site URLs — Antora-style xrefs with no
  upstream changes.
- **Embedded rendering**: `asciidoc_html5::convert_document*` with
  `Options::embedded` produces body-only HTML for the theme layer, exactly as
  Antora composes Asciidoctor's embeddable output into Handlebars layouts.
- **Always-on source maps**: every block carries a `Span` (line/col/byte
  offset), and `Document::source_map().original_file_and_line()` translates
  through includes. This powers diff anchoring (§9.1), coverage line→block
  mapping (§9.2), and click-to-source editing (§9.3).
- **Catalogs**: `Document::catalog()` (ids, reftexts, footnotes; images/links
  with `catalog_assets`) feeds the site catalog, asset validation, and search
  extraction.
- **Include handling**: the library's include/docinfo jail and `AssetWriter`
  seam already model "the caller owns the filesystem", which is what a
  virtual-file pipeline needs. One likely upstream ask: an include *resolver*
  hook (resolve `include::` targets through the ContentCatalog rather than the
  filesystem) so `partial$`/`example$` resource-ID includes work for content
  read from bare git repos. If the current handler traits don't accommodate
  that, it becomes an `asciidoc-parser`/`asciidoc-html5` issue — the only
  foreseeable upstream dependency of the early milestones.

Renderer completeness is tracked upstream and is not a blocker: unsupported
constructs render as visible `<!-- unsupported -->` comments, and the
first dogfood target (`asciidoc-html5`'s `docs/`) already restricts itself to
supported constructs.

## 9. The three differentiators

### 9.1 Version and PR diffs

*What changed between 1.3 and 1.4 of this spec? What does this PR change?*

- **Comparison sources.** The generator already builds N versions of each
  component (from branches/tags); diffing needs only pairing logic:
  adjacent published versions (`1.4` vs `1.3`), any user-selected pair, or —
  for PR/preview mode — the same component version aggregated twice (base ref
  vs head ref/worktree).
- **Diff model.** Diff at the *block* level using the parse tree, not raw text
  lines: align blocks between the two documents (by stable id where present,
  then by section path + similarity of source spans/content hashes, LCS over
  the block sequence), classify each as unchanged/added/removed/moved/edited;
  for edited blocks, word-level diff of the rendered inline HTML's text
  content (`similar`), re-annotated as `<ins>`/`<del>` spans.
- **Presentation.** Three artifacts per component-version pair: (a) a
  per-page diff view (toggle on the normal page: change bars in the gutter,
  ins/del highlighting, "jump to next change"); (b) a per-version "what
  changed" index listing added/removed/edited pages (nav-aware); (c) for PR
  mode, a self-contained preview site where every page carries its diff
  toggle — the reviewable artifact for documentation PRs, publishable from CI.
- **Placement.** `bokfell-diff` is generator-independent (two `Document`s +
  their sources in, change model + annotated HTML out) so it can later back an
  `adoc diff` CLI feature; `bokfell` wires it to version pairs and git refs.

### 9.2 Spec coverage ("what is tested / known good")

Generalizes the SDD workflow already used here and in `asciidoc-parser`: test
modules reproduce a spec page line-for-line inside `verifies!` /
`non_normative!` markers, and the `sdd` tool emits Codecov-style per-line
coverage JSON keyed by spec-file path.

- **Data contract.** Define a small, documented coverage format (per source
  file: line → status, where status ∈ verified / non-normative / uncovered,
  plus provenance: which crate/test), produced by repos and consumed by the
  generator. The existing `sdd` output is the seed; `bokfell-coverage` ships
  the reader, and a `bokfell coverage` subcommand ports the (currently
  "proof-of-concept, hard-coded") scanner so any repo using the marker
  convention can emit it without bespoke tooling.
- **Mapping.** Coverage is per *source line*; pages render from blocks. The
  always-on source map lets `bokfell-coverage` project line statuses onto
  blocks (and through includes), yielding block-level shading with
  line-level detail.
- **Presentation.** (a) A per-page overlay toggle: verified content plain,
  non-normative dimmed/badged, uncovered flagged; (b) per-page and
  per-component rollups (percent verified) as badges in nav and page headers;
  (c) a component "coverage dashboard" page; (d) optional page-status
  front-matter (draft/reviewed/known-good) folded into the same display.
- This is also the honest answer to "is this documentation trustworthy?" — a
  site can *show* that a claim is verified against the implementation, which
  no mainstream doc generator does.

### 9.3 Live preview and editing

- **Baseline (Zola-class).** `bokfell serve`: build into memory/temp dir, watch
  all aggregated *local* sources (worktrees, theme dir, playbook), debounce,
  classify the change (page content / partial / nav / antora.yml / theme /
  static asset), invalidate via the dependency graph the ContentCatalog
  already implies (page → includes, xref targets, images; template → all
  composed pages but no re-conversion), rebuild only that, push reload over
  WebSocket. Remote git sources stay cached — watch applies to local overlays.
- **Author mode done right.** The serve playbook overlay maps a remote source
  to a local worktree (`--local url=path`), including uncommitted state, so
  multi-repo preview needs no second playbook. Combined with §9.1's PR mode:
  `bokfell serve --diff base=main` gives a live-updating view of *exactly what
  your edits change*.
- **Editing round-trip.** The dev server exposes a small edit API: every
  rendered block carries its source location (file:line via span + include
  translation), the page chrome offers "edit this block", which opens the
  user's `$EDITOR`/IDE at that location (`vscode://file/...`-style deep links
  and a plain endpoint) — the watcher completes the loop. An optional
  in-browser plain-text editor pane (edit the page's AsciiDoc, save through
  the API, preview updates live, with scroll-sync via source maps) is the
  stretch goal; rich-text editing is out of scope (§2).

## 10. Milestones

Ordered to dogfood early and keep every milestone shippable:

- **M0 — Bootstrap.** New repo, workspace skeleton, CI mirroring
  `asciidoc-html5`'s conventions (fmt/clippy/test gates, conventional
  commits; release tooling to follow), MSRV aligned with `asciidoc-parser`.
  *Done — this repository.*
- **M1 — Single-component build.** Local-directory source → ContentCatalog →
  deferred parse + site-wide xref resolution → embedded render → default
  theme → static site with nav, start page, 404. **Exit criterion: builds
  `asciidoc-html5`'s `docs/` component and replaces its
  `antora-playbook-local.yml`/npm toolchain for local preview.**
- **M2 — Serve + live reload.** Watcher, change classification, incremental
  recompose vs re-render, WebSocket reload. Exit: sub-second edit→reload on
  the dogfood site.
- **M3 — Multi-repo, multi-version.** gix aggregation with cache, branch/tag
  patterns, worktree overlays, version sorting/latest/prerelease, version
  selector UI, redirects, sitemap, edit links. Exit: an aggregated
  asciidoc-rs site (parser + html5 + generator docs) with at least two
  versions of one component.
- **M4 — Coverage overlays.** Coverage format + reader, `bokfell coverage`
  scanner (ported/generalized `sdd`), per-page overlay + rollups + dashboard.
  Exit: the dogfood site displays live verification status for these repos'
  docs against their test suites.
- **M5 — Diff views.** `bokfell-diff` engine, version-pair pages, "what
  changed" index, PR preview mode (base vs head), CI recipe. Exit: a PR
  against a docs repo produces a browsable diff-highlighted preview site.
- **M6 — Edit round-trip.** Edit API, click-to-source with editor deep links,
  scroll-sync; optional in-browser source pane. Exit: §9.3 loop demoable
  end-to-end.
- **M7 — Search + polish → 0.1.** Static search index + UI, theme override
  docs, published crates + release binaries, migration guide from Antora.

Coverage (M4) deliberately precedes diff (M5): its data pipeline is simpler,
the `sdd` seed exists, and it exercises the source-map plumbing diff also
needs.

## 11. Risks

- **Include resolution through the catalog** (§8) may need an upstream hook;
  it sits on M1's critical path, so verify the handler traits in week one and
  file upstream early if needed.
- **Renderer gaps**: any Antora-compatible corpus will hit constructs
  `asciidoc-html5` doesn't render yet; mitigated by the visible-comment
  fallback, the dogfood-first milestone order, and upstream parity work
  continuing in parallel.
- **Block alignment quality** in `bokfell-diff` (movement vs edit ambiguity) is
  an open-ended tuning problem; contained by shipping per-page diffs before
  cross-page/moved-content detection.
- **gix API churn**: gitoxide is active but younger than libgit2; contained in
  `bokfell-aggregate` behind a narrow trait, with `git` CLI fallback as an
  escape hatch if needed.
- **Scope creep in the editor** (§9.3): the milestone gates it behind
  click-to-source; the in-browser pane ships only if the edit API proves out.

## 12. Decisions and open questions

**Decided** (2026-09-05):

1. **Name** — `bokfell` (§3).
2. **Antora playbook compatibility** — Bokfell defines its own playbook
   schema with a documented mapping from Antora's keys (and possibly a
   `bokfell init --from-antora` translator), rather than reading Antora
   playbooks directly. Full compatibility would drag in UI-bundle and
   extension keys the project intentionally doesn't honor.
   Repository-side formats (`antora.yml`, `nav.adoc`, the module layout,
   resource IDs) remain Antora-compatible per §5.
3. **Repo creation** — `asciidoc-rs/bokfell` exists and is bootstrapped;
   this document now lives here (the copy in `asciidoc-html5` was removed).

**Open:**

1. **Search index tech** — build-time index format (elasticlunr-compatible vs
   Pagefind-style chunked index vs a small custom format) — evaluate at M7.
2. **Theme engine** — minijinja vs Tera 2 (see §7 table); decide at M0 since
   the default theme is written against it.
