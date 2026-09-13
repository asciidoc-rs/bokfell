# RFC 0001: Spec coverage, second generation

Status: **draft** · Supersedes the interim `sdd` tools in `asciidoc-parser`
and `asciidoc-html5`, and revises PLAN.md §9.2's data contract.

## 1. Problems with the interim design

The interim marker grammar makes one structural decision that causes every
fragility we've hit: it expresses **two different facts in the same place**.

1. *Coverage claims* — `verifies!` says "this test verifies these lines."
2. *Spec classification* — `non_normative!` says "these lines carry no rule
   to verify."

Classification is a property of the spec text, not of any test. Because it
lives in test files anyway, a tracked page must be reproduced **in full**,
line for line, in order — that reproduction is the only way the scanner can
reconstruct the page by concatenation. Everything painful follows from it:

- **Forced 1:1 mapping.** One test module per spec file (`track_file!`), and
  a test can only verify lines of *that* file.
- **Forced sequence.** Test functions must appear in the spec file's order;
  a misplaced blank line silently misaligns everything after it.
- **Positional merging.** When two crates track the same page, both must
  reproduce the *entire* page identically so the tool can merge by line
  position.
- **Global drift breakage.** An upstream edit anywhere in a page invalidates
  the alignment of every claim positioned after it.
- **Textual scanning.** The scanner reads `.rs` files line-by-line and is
  sensitive to rustfmt wrapping and raw-string delimiters.
- **Conflated states.** `non_normative!` is the only escape hatch, so
  out-of-scope spec text (things this implementation deliberately does not
  do) is misfiled as non-normative and disappears from view.

## 2. Design overview

Separate the two facts, and add the states the interim design couldn't
express:

- **Claims** live in test code, free-floating: *any* test function, in any
  file, in any crate or repo, claims one or more specific paragraphs of any
  spec page by quoting an excerpt. No reproduction, no ordering, no 1:1
  mapping.
- **Classification** lives with the implementation, per spec page, in a
  reviewed sidecar file (the *coverage map*), seeded by structural
  heuristics from the parsed AsciiDoc. It records what is non-normative,
  what is deliberately **out of scope**, and what is **planned** (not
  implemented yet, with a tracking link).
- **The unit of coverage is the block** (paragraph, list item, table, …),
  not the line. Bokfell parses every spec page with source maps already;
  blocks are what the overlay UI shades, what excerpts resolve to, and what
  the states attach to. Line ranges remain derivable from the source map
  for detail display and for Codecov export.

The denominator is computed from the spec itself: every normative block of
every measured page counts, whether or not any test mentions it. A page
nobody has written tests for shows all its normative blocks as uncovered —
with zero test-side boilerplate.

## 3. Block states

Each block of a measured page resolves to exactly one state:

| State | Meaning | Counts in denominator? |
|---|---|---|
| `verified` | ≥ 1 claim resolves to this block | yes |
| `planned` | not implemented yet; carries a tracking link | yes |
| `uncovered` | normative, no claim, no annotation | yes |
| `unclassified` | heuristics could not decide and no human has | yes |
| `out-of-scope` | deliberately not implemented; carries a reason | **no** |
| `non-normative` | describes rather than specifies | no |

Notes on the two states the interim design lacked:

- **`out-of-scope`** is a product decision, so it leaves the denominator —
  but it is never invisible: rollups always display its count beside the
  percentage, and the overlay renders it distinctly with its recorded
  reason, so the headline number cannot be quietly gamed by reclassifying
  gaps as out of scope.
- **`planned`** is a *gap with a plan*: it stays in the denominator (the
  headline percentage does not improve until the work ships), but the
  overlay and dashboard render it distinctly and link to the tracking
  ticket, so "known and scheduled" reads differently from "nobody has
  looked."
- **`unclassified`** exists to preserve the review pressure the old
  full-reproduction discipline provided: it is rendered as loudly as
  `uncovered`, and `bokfell coverage lint` (§7) reports it, so pages drift
  toward fully classified over time.

The headline metric is:

```
percent verified = verified / (verified + planned + uncovered + unclassified)
```

Dashboards show the full five-way breakdown (stacked bar per page and per
component), with `out-of-scope` displayed adjacent to, but outside, the
bar.

## 4. Claims: the `verifies!` marker

`verifies!` remains a no-op `macro_rules!` marker — scanned repos take no
dependency — but it becomes self-targeting and position-independent:

```rust
#[test]
fn nested_ordered_markers() {
    verifies!(
        "ref/asciidoc-lang/docs/modules/lists/pages/ordered.adoc",
        r#"To nest an ordered list, add a marker character for each level
of nesting."#
    );
    // ... the actual test ...
}
```

Grammar (three forms):

```rust
verifies!("<path>", r#"<excerpt>"#);              // claim one block
verifies!("<path>#<anchor>", r#"<excerpt>"#);     // scoped to a section
verifies!("<path>#<anchor>");                     // claim a whole section
```

- **Path** is as written in the scanned repo (matching today's
  `track_file!` convention). The tool resolves it by suffix-matching
  against the catalog's source paths; ambiguity is a hard error fixed by
  writing a longer path.
- **Excerpt** is matched against the page's block text after whitespace
  normalization (the same collapse rules the search indexer uses). The
  excerpt may be any contiguous substring of one block. Matching is
  **exact** after normalization — fuzziness would silently heal spec
  drift, and drift detection is a feature we keep on purpose: when the
  upstream paragraph changes, exactly the claims that quote it fail to
  resolve, and no others.
- **Ambiguity** (excerpt matches more than one block in scope) is a hard
  error; the fix is a longer excerpt or a `#anchor` scope.
- **Whole-section claims** are for tests that genuinely exercise an entire
  section; per-paragraph excerpts are preferred because they keep review
  honest.
- A test may carry any number of `verifies!` invocations; any number of
  tests may claim the same block. Rollup is set union — merging is
  order-free and cross-crate/cross-repo by construction.

Each resolved claim records `(spec page, block, rust file, line, enclosing
test fn, crate, repo, rev)` — the provenance the click-through (§8) renders.

## 5. Classification: the coverage map

Per measured spec page, an optional sidecar file in the implementation
repo (not upstream — upstream spec files are never annotated). Suggested
layout mirrors the spec path under one root, e.g.:

```
spec-map/ref/asciidoc-lang/docs/modules/lists/pages/ordered.adoc.toml
```

```toml
# Entries target blocks by the same excerpt grammar claims use.

[[non-normative]]
section = "_a_note_on_terminology"        # whole section by anchor

[[non-normative]]
excerpt = "Asciidoctor also supports…"    # single block

[[out-of-scope]]
excerpt = "the DocBook converter emits"
reason = "asciidoc-html5 targets HTML5 only; no DocBook backend planned"

[[planned]]
excerpt = "Footnotes may be defined once and reused"
tracking = "asciidoc-rs/asciidoc-html5#341"
```

- `tracking` accepts a full URL or `owner/repo#N` shorthand (templated to
  GitHub).
- `reason` is required on `out-of-scope` and is rendered in the overlay.
- Sidecar excerpts resolve with the same exact-match rules as claims, so
  upstream drift surfaces in the coverage map too, not just in tests.

**Structural defaults** classify blocks no sidecar entry touches: example
and listing blocks, block titles, images, and nav are non-normative by
default; prose paragraphs, admonitions, and tables default to
`unclassified` on pages that have no sidecar at all, and to *normative*
(i.e. `uncovered` until claimed) on pages whose sidecar declares
`reviewed = true` at the top. This keeps the pressure state meaningful:
a page becomes "fully classified" by an explicit human act, not by the
heuristic's silence.

## 6. Extraction: how claims are found

A `syn`-based static scan of the configured test roots: parse each `.rs`
file properly, find `verifies!` invocations anywhere (any nesting, any
formatting), record the claim plus the span of the enclosing `fn`. This
eliminates the rustfmt/raw-string fragility of line scanning, imposes no
dependency on scanned repos, and yields the test's source location — which
the click-through needs anyway.

*Deliberately deferred:* a proc-macro/`inventory` variant that collects
claims at test **run** time, so a claim only counts when its test compiled
and passed. Stronger semantics, but it puts a dev-dependency and a harness
hook into every covered repo; in practice CI already gates merges on green
tests, so the static scan's claims are backed by passing tests. The claim
schema (§4's tuple) is designed so a runtime collector can emit the same
records later without a format change.

## 7. CLI surface

Spec coverage becomes a Bokfell subcommand backed by the existing
`bokfell-coverage` crate (scanner and resolver live in the library; the
CLI stays thin). The spec sources to measure are the playbook's own
content sources — the spec pages *are* the site's pages — plus a new
`coverage:` section naming what to scan:

```yaml
coverage:
  scan:
    - repo: https://github.com/asciidoc-rs/asciidoc-html5
      tests: [html5/src/tests, cli/src/tests]
      spec_map: spec-map
    - repo: https://github.com/asciidoc-rs/asciidoc-parser
      tests: [parser/src/tests]
      spec_map: spec-map
```

Commands:

- `bokfell coverage scan` — aggregate (reusing `bokfell-aggregate`), parse
  measured pages, scan test roots, resolve claims and sidecar entries,
  write the coverage database. Unresolved or ambiguous excerpts are
  errors listed with file/line provenance.
- `bokfell coverage report [--format table|json|codecov]` — rollups.
  `--format codecov` projects block states onto line ranges via the source
  map and emits the interim tools' schema, so existing CI uploads swap
  over without pipeline changes.
- `bokfell coverage lint` — review affordances: unclassified blocks,
  sidecar entries the structural heuristic disagrees with (prime
  candidates for the out-of-scope vs non-normative cleanup, §9), planned
  entries whose tracking ticket is closed, claims and sidecar entries that
  no longer resolve.
- `bokfell build` / `serve` consume the database directly — no
  `coverage:` file handoff needed when the scan config is in the playbook
  (the existing external-file input remains for pre-computed data).

## 8. Site rendering

- **Overlay.** The §9.2 toggle gains the five-state palette: `verified`
  plain/green, `planned` amber with the ticket link, `uncovered` and
  `unclassified` red-family (distinct hatching), `out-of-scope` dimmed
  with a scope badge; `non-normative` dimmed as today. Clicking a block
  opens a detail panel (the diff panels' UI pattern) showing its state,
  reason or ticket where present, and its claims.
- **Click-through to the verifying code.** Each claim in the panel links
  to the test:
  - *Build mode:* URL templated from the claim's provenance — the
    aggregator knows repo and rev for git sources, so
    `https://github.com/{repo}/blob/{rev}/{path}#L{line}` works with no
    extra configuration; a `coverage.link_template` playbook key overrides
    it for non-GitHub hosts.
  - *Serve mode:* reuse the §9.3 edit round-trip verbatim — "open test"
    posts to `/__bokfell/edit?file&line` and lands in the local editor at
    the test function. Test roots join the editable allowlist.
- **Dashboard.** Per-page and per-component stacked bars over the five
  states, `out-of-scope` beside the bar, `unclassified` called out as the
  review queue.

## 9. Migration

The existing marker corpus is a hand-reviewed classification of every
tracked page — harvest it, don't discard it:

1. A one-shot converter rewrites each `verifies!` reproduction block into
   a targeted claim (path from `track_file!`, excerpt from the block
   body, trimmed of blank-line bookkeeping) and emits each
   `non_normative!` span as a sidecar `[[non-normative]]` entry.
2. Verify the converter by diffing coverage output before and after —
   identical modulo line→block granularity.
3. The interim corpus conflates non-normative with out-of-scope; the
   converter cannot distinguish them. The reclassification is a manual
   editorial pass, driven by `bokfell coverage lint`'s heuristic-disagreement
   list (blocks the heuristic reads as normative prose but the harvested
   sidecar marks non-normative are exactly the candidates).
4. Once parity is verified in both repos, the interim `sdd` binaries and
   the full-page-reproduction convention retire; `--format codecov` keeps
   the CI uploads unchanged.

## 10. Open questions

1. **Excerpt normalization details** — is whitespace collapse enough, or
   should inline markup be stripped before matching (so an excerpt can be
   quoted from the *rendered* page)? Leaning: match against the parsed
   block's source text with whitespace collapse only; quoting from source
   keeps claims reviewable against the `.adoc` file.
2. **Sub-block granularity** — a block-level claim occasionally covers a
   paragraph that states two rules. Is that acceptable rounding, or do we
   need an optional "partially verifies" qualifier? Leaning: accept the
   rounding; revisit with evidence.
3. **`reviewed = true` semantics** — per page, or per section for long
   pages that get classified incrementally?
4. **Where the coverage database lives** — ephemeral (recomputed by every
   `scan`) vs committed artifact. Leaning: ephemeral, with `--format
   codecov` as the only persisted export.
