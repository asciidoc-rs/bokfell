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

**Pre-alpha.** This repository currently holds the workspace skeleton, CI,
and the founding plan; nothing is implemented yet. See [`PLAN.md`](PLAN.md)
for the architecture, the design of the three differentiators, and the
milestone roadmap, and [`CLAUDE.md`](CLAUDE.md) for contributor conventions
(including important license boundaries around code borrowed from other
site generators).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
