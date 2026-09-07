# Bokfell

Bokfell (Old Norse *bókfell*, "book-skin" — the prepared vellum a manuscript
is written on) is a documentation site generator for AsciiDoc, built on
[`asciidoc-parser`](https://crates.io/crates/asciidoc-parser) and
[`asciidoc-html5`](https://crates.io/crates/asciidoc-html5).

It aggregates AsciiDoc content from one or more git repositories, organizes
it into versioned components using [Antora](https://antora.org)'s proven
content model (`antora.yml`, modules, families, resource IDs, `nav.adoc`),
and publishes a cross-linked, navigable static site — from a single static
binary, with no Node.js, npm, or Ruby anywhere in the pipeline.

On top of that baseline, Bokfell adds three capabilities Antora doesn't
have:

1. **Version and PR diffs** — rendered pages that highlight what changed in
   a spec or document between published versions, or in a proposed update on
   a branch or pull request.
2. **Spec coverage overlays** — surfacing which portions of a spec are
   tested, verified, or known-good, generalizing the spec-driven-development
   workflow used across the asciidoc-rs crates.
3. **Live preview and editing** — a built-in dev server with file watching,
   incremental rebuilds, browser live-reload, and click-to-source editing.

## Status

**Pre-alpha.** The M1 single-component pipeline works: `bokfell build`
takes a playbook, scans an Antora-compatible content source, resolves
cross-references and includes site-wide, and publishes a themed static
site — and `bokfell serve` runs the same build into memory behind a
watching dev server with live browser reload (~150 ms rebuilds on a
37-page site). Content sources can be git repositories: matched branches
and tags become component versions (bare cache clones, no checkouts),
with Antora's version ordering, latest-version routing, and a per-page
version selector. Spec-coverage overlays work end to end: per-line
coverage JSON (from a tool like asciidoc-rs's `sdd`) becomes a
verified-percentage badge on each covered page, a click-to-toggle
per-block shading overlay, and a site-wide `/coverage.html` dashboard.
Diff views work end to end too: each versioned page diffs against its
previous version (or against a git ref with `--diff-base`, previewing a
working tree or PR branch), changed pages get a "Changed since X" badge
that highlights added/edited blocks — click an edited block for its
word-level diff — and a site-wide `whats-changed.html` lists what
changed. And `bokfell serve` closes the edit loop: every rendered block
carries an edit button that opens your editor at the exact source file
and line (`--editor "cmd {file}:{line}"`, `$BOKFELL_EDITOR`, or VS
Code's `code --goto` by default) — save, and the watcher rebuilds and
reloads the browser. Every site ships client-side search: a header box
backed by a build-time static index, no services involved. See
[`PLAN.md`](PLAN.md) for the architecture
and milestone roadmap, and [`CLAUDE.md`](CLAUDE.md) for contributor
conventions (including important license boundaries around code borrowed
from other site generators).

## Try it

A self-contained sample site lives in
[`examples/hello-bokfell/`](examples/hello-bokfell/):

```sh
cd examples/hello-bokfell
cargo run --bin bokfell -- serve     # http://127.0.0.1:8000/, live reload
# or: cargo run --bin bokfell -- build   (site lands in build/site/)
```

Edit anything under `examples/hello-bokfell/docs/` while `serve` runs and
the browser reloads with the change. The home page also demos the
spec-coverage overlay: click its "60% verified" badge to shade each block
by verification status. And after editing a page, rebuild with
`cargo run --bin bokfell -- build --diff-base main` — every page you
changed gets a "Changed since main" badge that highlights exactly what
you added or edited. While `serve` runs, hover any block and click its
pencil button to jump straight to that block's source in your editor.

## Building a site

Point a playbook at one or more content source roots (directories holding
an `antora.yml` and a `modules/` tree):

```yaml
# bokfell.yml
site:
  title: AsciiDoc HTML5
  start_page: html5::index.adoc
content:
  sources:
    - path: ../asciidoc-html5/docs   # a plain directory…
    - url: https://github.com/asciidoc-rs/asciidoc-html5
      branches: [main]               # …or a git repo: refs become versions
      tags: ['asciidoc-html5-v*']
      start_path: docs
      version_from_ref: true
      coverage:                      # optional spec-coverage JSON
        - spec-coverage.json         # (per-line, Codecov-style; see PLAN.md)
output:
  dir: build/site
```

Then:

```sh
bokfell build            # or: bokfell build -p path/to/bokfell.yml
```

The site lands in `output.dir` with a root `index.html` redirecting to the
start page. `--theme <dir>` overrides the built-in layout per file.

For a live-reloading preview while editing:

```sh
bokfell serve            # http://127.0.0.1:8000/, --port to change
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
