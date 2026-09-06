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
version selector. Diffs, coverage overlays, and click-to-source editing
are still to come — see [`PLAN.md`](PLAN.md) for the architecture
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
the browser reloads with the change.

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
