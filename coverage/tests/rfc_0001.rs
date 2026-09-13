//! The spec-coverage engine verified against its own specification.
//!
//! Every test here claims the RFC 0001 paragraphs it exercises with
//! `verifies!` — the no-op marker the engine scans for — so `bokfell
//! coverage scan` at the repository root rolls this file up onto the RFC
//! page of Bokfell's own documentation site.

use bokfell_coverage::{
    excerpt_matches, load_spec_map, normalize_whitespace, resolve, scan_source, scan_test_root,
    BlockState, Claim, ClaimSite, ClaimTarget, CoverageDatabase, DiagnosticKind, MeasuredPage,
    Sidecar, SpecBlock, StructuralKind, TestRoot, Tracking,
};
use bokfell_model::{Coords, Family, Playbook};

/// The no-op claim marker (RFC 0001 §4): scanned repositories take no
/// dependency on Bokfell to carry claims.
macro_rules! verifies {
    ($($tt:tt)*) => {};
}

const RFC: &str = "docs/modules/rfcs/pages/0001-spec-coverage.adoc";

fn block(text: &str, kind: StructuralKind, sections: &[&str], line: u32) -> SpecBlock {
    SpecBlock {
        context: match kind {
            StructuralKind::Prose => "paragraph".to_string(),
            StructuralKind::NonNormative => "listing".to_string(),
        },
        text: text.to_string(),
        start_line: line,
        line_count: 2,
        page_lines: Some((line, 2)),
        sections: sections.iter().map(|s| s.to_string()).collect(),
        kind,
    }
}

fn page(version: Option<&str>, blocks: Vec<SpecBlock>) -> MeasuredPage {
    let mut section_ids: Vec<String> = blocks.iter().flat_map(|b| b.sections.clone()).collect();
    section_ids.sort();
    section_ids.dedup();
    MeasuredPage {
        coords: Coords {
            component: "spec".into(),
            version: version.map(str::to_string),
            module: "ROOT".into(),
            family: Family::Page,
            path: "rules.adoc".into(),
        },
        url: match version {
            Some(v) => format!("spec/{v}/rules.html"),
            None => "spec/rules.html".into(),
        },
        repo_path: "docs/modules/ROOT/pages/rules.adoc".into(),
        scope: 0,
        blocks,
        section_ids,
    }
}

/// A page with two sections: a normative rule, a listing, and a note.
fn rules_page(version: Option<&str>) -> MeasuredPage {
    page(
        version,
        vec![
            block("An introduction.", StructuralKind::Prose, &[], 3),
            block(
                "A widget MUST be frobbed before use.",
                StructuralKind::Prose,
                &["_rules"],
                7,
            ),
            block(
                "----\nfrob(widget)\n----",
                StructuralKind::NonNormative,
                &["_rules"],
                10,
            ),
            block(
                "Frobbing twice is harmless.",
                StructuralKind::Prose,
                &["_rules"],
                14,
            ),
            block(
                "Legacy widgets emit DocBook.",
                StructuralKind::Prose,
                &["_notes"],
                18,
            ),
        ],
    )
}

fn claim(file: &str, line: u32, target: &str, excerpt: Option<&str>) -> Claim {
    let (path, anchor) = match target.split_once('#') {
        Some((p, a)) => (p.to_string(), Some(a.to_string())),
        None => (target.to_string(), None),
    };
    Claim {
        target: if path.contains(':') {
            ClaimTarget::ResourceId(path)
        } else {
            ClaimTarget::Path(path)
        },
        anchor,
        excerpt: excerpt.map(str::to_string),
        site: ClaimSite {
            file: file.to_string(),
            local_path: None,
            line,
            test_fn: Some("a_test".into()),
            krate: Some("widgets".into()),
            repo: None,
            rev: None,
            scope: 0,
        },
    }
}

fn sidecar(text: &str) -> Sidecar {
    Sidecar::parse(
        text,
        "spec-map/docs/modules/ROOT/pages/rules.adoc.toml",
        "rules.adoc",
        0,
    )
    .unwrap()
}

fn states(db: &CoverageDatabase, page: usize) -> Vec<BlockState> {
    db.pages[page]
        .coverage
        .blocks
        .iter()
        .map(|b| b.state)
        .collect()
}

#[test]
fn claims_and_classification_are_separate_facts() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        r#"*Claims* live in test code, free-floating: _any_ test function, in any
  file, in any crate or repo, claims one or more specific paragraphs of any
  spec page by quoting an excerpt. No reproduction, no ordering, no 1:1
  mapping."#
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Classification* lives with the implementation, per spec page, in a reviewed sidecar file (the _coverage map_)"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*The unit of coverage is the block* (paragraph, list item, table, ...), not the line."
    );

    // Two claims from unrelated files, in no particular order, and a
    // sidecar that knows nothing about either: each fact stands alone.
    let claims = vec![
        claim(
            "crates/b/tests/late.rs",
            90,
            "pages/rules.adoc",
            Some("harmless"),
        ),
        claim(
            "crates/a/src/tests/early.rs",
            4,
            "rules.adoc",
            Some("MUST be frobbed"),
        ),
    ];
    let map = sidecar("reviewed = true\n[[non-normative]]\nexcerpt = \"An introduction.\"\n");
    let db = resolve(&[rules_page(None)], claims, &[map]);
    assert!(db.errors.is_empty(), "{:?}", db.errors);
    assert_eq!(
        states(&db, 0),
        [
            BlockState::NonNormative,
            BlockState::Verified,
            BlockState::NonNormative,
            BlockState::Verified,
            BlockState::Uncovered,
        ]
    );

    // Block granularity: the claim covers the whole paragraph it quotes.
    assert_eq!(db.pages[0].coverage.blocks[1].claims, [1]);
    assert_eq!(db.pages[0].coverage.blocks[3].claims, [0]);
}

#[test]
fn the_denominator_comes_from_the_spec_itself() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "The denominator is computed from the spec itself: every normative block of every measured page counts, whether or not any test mentions it."
    );

    // No test mentions this page at all: every normative block is still
    // counted (uncovered once reviewed, unclassified before), with zero
    // test-side boilerplate.
    let db = resolve(
        &[rules_page(None)],
        Vec::new(),
        &[sidecar("reviewed = true\n")],
    );
    assert_eq!(db.pages[0].coverage.counts.uncovered, 4);
    assert_eq!(db.pages[0].coverage.counts.denominator(), 4);
    assert_eq!(db.pages[0].coverage.percent_verified(), 0);

    let db = resolve(&[rules_page(None)], Vec::new(), &[]);
    assert_eq!(db.pages[0].coverage.counts.unclassified, 4);
    assert_eq!(db.pages[0].coverage.counts.denominator(), 4);
}

#[test]
fn every_block_resolves_to_exactly_one_state() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Each block of a measured page resolves to exactly one state:"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        r#"| `out-of-scope`
| deliberately not implemented; carries a reason
| *no*

| `non-normative`
| describes rather than specifies
| no"#
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "| `unclassified` | heuristics could not decide and no human has | yes"
    );

    // The six states, each block exactly one of them.
    let map = sidecar(
        "[[out-of-scope]]\nexcerpt = \"DocBook\"\nreason = \"HTML5 only\"\n\
         [[planned]]\nexcerpt = \"harmless\"\ntracking = \"o/r#1\"\n",
    );
    let db = resolve(
        &[rules_page(None)],
        vec![claim("t.rs", 1, "rules.adoc", Some("MUST be frobbed"))],
        &[map],
    );
    assert!(db.errors.is_empty(), "{:?}", db.errors);
    let resolved = states(&db, 0);
    assert_eq!(
        resolved,
        [
            BlockState::Unclassified,
            BlockState::Verified,
            BlockState::NonNormative,
            BlockState::Planned,
            BlockState::OutOfScope,
        ]
    );
    assert_eq!(
        db.pages[0].coverage.blocks.len(),
        rules_page(None).blocks.len()
    );

    // Which states count in the denominator, per the table.
    for state in BlockState::ALL {
        let expected = !matches!(state, BlockState::OutOfScope | BlockState::NonNormative);
        assert_eq!(state.in_denominator(), expected, "{state}");
    }
    assert_eq!(db.pages[0].coverage.counts.denominator(), 3);
}

#[test]
fn out_of_scope_and_planned_semantics() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`out-of-scope` is a product decision, so it leaves the denominator -- but it is never invisible: rollups always display its count beside the percentage"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`planned` is a _gap with a plan_: it stays in the denominator (the headline percentage does not improve until the work ships)"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`unclassified` exists to preserve the review pressure the old full-reproduction discipline provided"
    );

    let map = sidecar(
        "reviewed = true\n\
         [[out-of-scope]]\nexcerpt = \"DocBook\"\nreason = \"HTML5 only\"\n\
         [[planned]]\nexcerpt = \"harmless\"\ntracking = \"o/r#1\"\n",
    );
    let db = resolve(
        &[rules_page(None)],
        vec![claim("t.rs", 1, "rules.adoc", Some("MUST be frobbed"))],
        &[map],
    );
    let counts = db.pages[0].coverage.counts;

    // Out of scope leaves the denominator but its count stays reported.
    assert_eq!(counts.out_of_scope, 1);
    assert_eq!(counts.denominator(), 3);
    assert!(bokfell_coverage::report::table(&db).contains("OUT-OF-SCOPE"));

    // Planned stays in the denominator: 1 verified of (1 + 1 planned +
    // 1 uncovered) — the headline does not improve until it ships.
    assert_eq!(counts.planned, 1);
    assert_eq!(counts.percent_verified(), 33);
    let planned = &db.pages[0].coverage.blocks[3];
    assert_eq!(
        planned.tracking.as_ref().unwrap().url,
        "https://github.com/o/r/issues/1"
    );
    let out = &db.pages[0].coverage.blocks[4];
    assert_eq!(out.reason.as_deref(), Some("HTML5 only"));

    // Unclassified blocks are lint findings on unreviewed pages.
    let db = resolve(&[rules_page(None)], Vec::new(), &[]);
    let unclassified: Vec<_> = db
        .lint
        .iter()
        .filter(|d| d.kind == DiagnosticKind::Unclassified)
        .collect();
    assert_eq!(unclassified.len(), 4);
    assert!(bokfell_coverage::report::lint(&db).contains("unclassified finding(s)"));
}

#[test]
fn conflict_precedence_is_deterministic() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Conflict precedence.* Claims and sidecar entries can target the same block; resolution is deterministic:"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "A claim on a `planned` block means the planned work shipped: the block is *verified*, and `bokfell coverage lint` (<<_cli_surface,§7>>) flags the now-stale `planned` entry for removal."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "A claim on a `non-normative` or `out-of-scope` block is a contradiction -- the test asserts a rule the sidecar says isn't there (or isn't ours) -- and is a hard `scan` error naming both sources"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "A block targeted by more than one sidecar list (e.g. both `non-normative` and `planned`) is a hard error when the sidecar loads."
    );

    // Claim on planned → verified plus a stale-planned lint finding.
    let map = sidecar("[[planned]]\nexcerpt = \"harmless\"\ntracking = \"o/r#1\"\n");
    let db = resolve(
        &[rules_page(None)],
        vec![claim("t.rs", 3, "rules.adoc", Some("Frobbing twice"))],
        &[map],
    );
    assert!(db.errors.is_empty(), "{:?}", db.errors);
    assert_eq!(states(&db, 0)[3], BlockState::Verified);
    let stale = db
        .lint
        .iter()
        .find(|d| d.kind == DiagnosticKind::StalePlanned)
        .expect("stale planned finding");
    assert_eq!(stale.at, "spec-map/docs/modules/ROOT/pages/rules.adoc.toml");

    // Claim on non-normative / out-of-scope → hard error naming both
    // the claim site and the sidecar entry.
    for entry in [
        "[[non-normative]]\nexcerpt = \"DocBook\"\n",
        "[[out-of-scope]]\nexcerpt = \"DocBook\"\nreason = \"HTML5 only\"\n",
    ] {
        let db = resolve(
            &[rules_page(None)],
            vec![claim("t.rs", 8, "rules.adoc", Some("DocBook"))],
            &[sidecar(entry)],
        );
        assert_eq!(db.errors.len(), 1, "{:?}", db.errors);
        let error = &db.errors[0];
        assert_eq!(error.kind, DiagnosticKind::Contradiction);
        assert!(error.kind.is_error());
        assert_eq!(error.at, "t.rs:8");
        assert!(error.message.contains("rules.adoc.toml"), "{error}");
        assert!(error.message.contains("excerpt = \"DocBook\""), "{error}");
    }

    // Two lists on one block → hard error.
    let map = sidecar(
        "[[non-normative]]\nsection = \"_notes\"\n\
         [[planned]]\nexcerpt = \"DocBook\"\ntracking = \"o/r#2\"\n",
    );
    let db = resolve(&[rules_page(None)], Vec::new(), &[map]);
    assert_eq!(db.errors[0].kind, DiagnosticKind::SidecarConflict);
    assert!(db.errors[0]
        .message
        .contains("[[non-normative]] section = \"_notes\""));
    assert!(db.errors[0]
        .message
        .contains("[[planned]] excerpt = \"DocBook\""));

    // The same outcome whichever order the inputs arrive in.
    let map = sidecar("[[planned]]\nexcerpt = \"harmless\"\ntracking = \"o/r#1\"\n");
    let claims = vec![
        claim("t.rs", 3, "rules.adoc", Some("Frobbing twice")),
        claim("t.rs", 1, "rules.adoc", Some("MUST be frobbed")),
    ];
    let forward = resolve(
        &[rules_page(None)],
        claims.clone(),
        std::slice::from_ref(&map),
    );
    let reversed = resolve(
        &[rules_page(None)],
        claims.into_iter().rev().collect(),
        &[map],
    );
    assert_eq!(states(&forward, 0), states(&reversed, 0));
}

#[test]
fn the_headline_metric() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "The headline metric is:"
    );

    // percent verified = verified / (verified + planned + uncovered +
    // unclassified); out-of-scope and non-normative are outside it.
    let mut counts = bokfell_coverage::StateCounts::default();
    for (state, n) in [
        (BlockState::Verified, 3),
        (BlockState::Planned, 1),
        (BlockState::Uncovered, 1),
        (BlockState::Unclassified, 1),
        (BlockState::OutOfScope, 10),
        (BlockState::NonNormative, 10),
    ] {
        for _ in 0..n {
            counts.add(state);
        }
    }
    assert_eq!(counts.denominator(), 6);
    assert_eq!(counts.percent_verified(), 50);
    assert_eq!(
        bokfell_coverage::StateCounts::default().percent_verified(),
        100
    );
}

#[test]
fn verifies_is_a_no_op_marker_found_anywhere() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`verifies!` remains a no-op `macro_rules!` marker -- scanned repos take no dependency -- but it becomes self-targeting and position-independent:"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Grammar (three forms):"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Each resolved claim records `(spec page, block, rust file, line, enclosing test fn, crate, repo, rev)` -- the provenance the click-through (<<_site_rendering,§8>>) renders."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        r#"A `syn`-based static scan of the configured test roots: parse each `.rs`
file properly, find `verifies!` invocations anywhere (any nesting, any
formatting), record the claim plus the span of the enclosing `fn`."#
    );

    // This very file defines the marker as a no-op and still compiles.
    let site = ClaimSite {
        file: "widgets/src/tests/frob.rs".into(),
        local_path: None,
        line: 0,
        test_fn: None,
        krate: Some("widgets".into()),
        repo: Some("https://github.com/o/widgets".into()),
        rev: Some("deadbeef".into()),
        scope: 2,
    };
    let source = r##"
macro_rules! verifies { ($($t:tt)*) => {}; }

mod deep { mod deeper {
    #[test]
    fn late_in_the_file() {
        let x = { if true {
            // Three forms, any nesting, any formatting:
            verifies!("docs/rules.adoc",
                      r#"one block"#);
            verifies!("docs/rules.adoc#_rules", "scoped to a section");
            verifies!(
                "docs/rules.adoc#_rules"
            );
        } };
        let _ = x;
    }
} }
"##;
    let claims = scan_source(source, &site).unwrap();
    assert_eq!(claims.len(), 3);
    assert_eq!(
        claims[0].target,
        ClaimTarget::Path("docs/rules.adoc".into())
    );
    assert_eq!(claims[0].excerpt.as_deref(), Some("one block"));
    assert_eq!(claims[1].anchor.as_deref(), Some("_rules"));
    assert_eq!(claims[1].excerpt.as_deref(), Some("scoped to a section"));
    assert_eq!(claims[2].anchor.as_deref(), Some("_rules"));
    assert_eq!(claims[2].excerpt, None);

    // Provenance: rust file, line, enclosing test fn, crate, repo, rev.
    let site = &claims[0].site;
    assert_eq!(site.file, "widgets/src/tests/frob.rs");
    assert_eq!(site.line, 9);
    assert_eq!(site.test_fn.as_deref(), Some("late_in_the_file"));
    assert_eq!(site.krate.as_deref(), Some("widgets"));
    assert_eq!(site.repo.as_deref(), Some("https://github.com/o/widgets"));
    assert_eq!(site.rev.as_deref(), Some("deadbeef"));
    assert_eq!(site.scope, 2);
}

#[test]
fn claim_resolution_is_scoped_exact_and_per_version() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "resolution is *scoped*, not global: a claim resolves by suffix-matching against catalog pages sourced from the same `coverage.scan` entry's repo as the test root"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "A cross-repo claim names the page with a full Antora-style resource ID (`component:module:page.adoc`) instead of a path."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "versions whose block text matches are verified; versions whose text has since diverged are simply unmatched, which is not an error."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Only a path matching no page in scope, an excerpt matching *no* measured version, or an excerpt ambiguous *within one version's page* is a hard error."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Matching is *exact* after normalization -- fuzziness would silently heal spec drift"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Ambiguity* (excerpt matches more than one block in scope) is a hard error; the fix is a longer excerpt or a `#anchor` scope."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Rollup is set union -- merging is order-free and cross-crate/cross-repo by construction."
    );

    // Scoping: an identical path in another scan entry's repository
    // never collides; a resource ID reaches it.
    let mut foreign = rules_page(None);
    foreign.scope = 1;
    foreign.coords.component = "other".into();
    let claims = vec![
        claim("t.rs", 1, "rules.adoc", Some("MUST be frobbed")),
        claim("t.rs", 2, "other::rules.adoc", Some("MUST be frobbed")),
    ];
    let db = resolve(&[rules_page(None), foreign], claims, &[]);
    assert!(db.errors.is_empty(), "{:?}", db.errors);
    assert_eq!(db.pages[0].coverage.blocks[1].claims, [0]);
    assert_eq!(db.pages[1].coverage.blocks[1].claims, [1]);

    // Per version: the excerpt decides; drift in one version is not an
    // error, and a claim matching no version is.
    let mut old = rules_page(Some("1.0"));
    old.blocks[1].text = "A widget SHOULD be frobbed before use.".into();
    let new = rules_page(Some("2.0"));
    let claims = vec![
        claim("t.rs", 1, "rules.adoc", Some("MUST be frobbed")),
        claim("t.rs", 2, "rules.adoc", Some("wording that exists nowhere")),
        claim("t.rs", 3, "nowhere.adoc", Some("MUST be frobbed")),
    ];
    let db = resolve(&[old, new], claims, &[]);
    assert_eq!(
        db.pages[0].coverage.blocks[1].state,
        BlockState::Unclassified
    );
    assert_eq!(db.pages[1].coverage.blocks[1].state, BlockState::Verified);
    let kinds: Vec<DiagnosticKind> = db.errors.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        [DiagnosticKind::NoMatch, DiagnosticKind::UnknownPage]
    );
    assert_eq!(db.errors[0].at, "t.rs:2");

    // Exact after normalization: whitespace is forgiven, words are not.
    assert!(excerpt_matches(
        "A widget MUST\n  be frobbed",
        "widget MUST be"
    ));
    assert!(!excerpt_matches(
        "A widget MUST be frobbed",
        "widget must be"
    ));

    // Ambiguity within one version's page is a hard error, and a
    // #anchor scope disambiguates.
    let mut twins = rules_page(None);
    twins.blocks[0].text = "Frobbing is described in the rules.".into();
    let db = resolve(
        &[twins.clone()],
        vec![claim("t.rs", 5, "rules.adoc", Some("Frobbing"))],
        &[],
    );
    assert_eq!(db.errors[0].kind, DiagnosticKind::Ambiguous);
    assert!(db.errors[0].message.contains("#anchor"));
    let db = resolve(
        &[twins],
        vec![claim("t.rs", 5, "rules.adoc#_rules", Some("Frobbing"))],
        &[],
    );
    assert!(db.errors.is_empty(), "{:?}", db.errors);

    // Set union: any number of tests may claim one block, from any file
    // in any order, and the block is verified exactly once.
    let claims = vec![
        claim("b/tests/z.rs", 1, "rules.adoc", Some("MUST be frobbed")),
        claim("a/src/tests/a.rs", 9, "rules.adoc", Some("A widget MUST")),
        claim("a/src/tests/a.rs", 2, "rules.adoc#_rules", None),
    ];
    let db = resolve(&[rules_page(None)], claims, &[]);
    assert!(db.errors.is_empty(), "{:?}", db.errors);
    assert_eq!(db.pages[0].coverage.blocks[1].claims, [0, 1, 2]);
    assert_eq!(db.pages[0].coverage.counts.verified, 2);
}

#[test]
fn the_coverage_map_sidecar() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Per measured spec page, an optional sidecar file in the implementation repo (not upstream -- upstream spec files are never annotated)."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`tracking` accepts a full URL or `owner/repo#N` shorthand (templated to GitHub)."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`reason` is required on `out-of-scope` and is rendered in the overlay."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Sidecar excerpts resolve with the same exact-match rules as claims, so upstream drift surfaces in the coverage map too, not just in tests."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Structural defaults* classify blocks no sidecar entry touches: example and listing blocks, block titles, images, and nav are non-normative by default; prose paragraphs, admonitions, and tables default to `unclassified` on pages that have no sidecar at all, and to _normative_ (i.e. `uncovered` until claimed) on pages whose sidecar declares `reviewed = true` at the top."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`reviewed` is *per page*"
    );

    // The sidecar mirrors the spec path under one root and is optional.
    let dir = std::env::temp_dir().join(format!("bokfell-rfc-sidecar-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("docs/modules/ROOT/pages")).unwrap();
    std::fs::write(
        dir.join("docs/modules/ROOT/pages/rules.adoc.toml"),
        "reviewed = true\n[[planned]]\nexcerpt = \"harmless\"\ntracking = \"o/r#7\"\n",
    )
    .unwrap();
    let sidecars = load_spec_map(&dir, "spec-map", 0).unwrap();
    assert_eq!(sidecars.len(), 1);
    assert_eq!(sidecars[0].spec_path, "docs/modules/ROOT/pages/rules.adoc");
    std::fs::remove_dir_all(&dir).ok();

    // Tracking shorthand and URL forms.
    assert_eq!(
        Tracking::parse("owner/repo#12").url,
        "https://github.com/owner/repo/issues/12"
    );
    assert_eq!(
        Tracking::parse("https://example.org/issues/12").url,
        "https://example.org/issues/12"
    );

    // Reason is required on out-of-scope; tracking on planned.
    assert!(Sidecar::parse("[[out-of-scope]]\nexcerpt = \"x\"\n", "f", "p", 0).is_err());
    assert!(Sidecar::parse("[[planned]]\nexcerpt = \"x\"\n", "f", "p", 0).is_err());

    // Sidecar drift is an error with the sidecar file as provenance.
    let db = resolve(
        &[rules_page(None)],
        Vec::new(),
        &[sidecar(
            "[[non-normative]]\nexcerpt = \"wording that drifted\"\n",
        )],
    );
    assert_eq!(db.errors[0].kind, DiagnosticKind::NoMatch);
    assert_eq!(
        db.errors[0].at,
        "spec-map/docs/modules/ROOT/pages/rules.adoc.toml"
    );

    // Structural defaults: the listing is non-normative untouched; prose
    // is unclassified without a sidecar and uncovered once reviewed.
    let db = resolve(&[rules_page(None)], Vec::new(), &[]);
    assert_eq!(states(&db, 0)[2], BlockState::NonNormative);
    assert_eq!(states(&db, 0)[1], BlockState::Unclassified);
    let db = resolve(
        &[rules_page(None)],
        Vec::new(),
        &[sidecar("reviewed = true\n")],
    );
    assert_eq!(states(&db, 0)[1], BlockState::Uncovered);
    assert_eq!(states(&db, 0)[2], BlockState::NonNormative);

    // Reviewed is per page: another version of the page with no sidecar
    // match stays unclassified.
    let mut other = rules_page(None);
    other.coords.path = "more.adoc".into();
    other.repo_path = "docs/modules/ROOT/pages/more.adoc".into();
    let db = resolve(
        &[rules_page(None), other],
        Vec::new(),
        &[sidecar("reviewed = true\n")],
    );
    assert_eq!(states(&db, 0)[1], BlockState::Uncovered);
    assert_eq!(states(&db, 1)[1], BlockState::Unclassified);
}

#[test]
fn the_cli_surface_and_codecov_projection() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "The spec sources to measure are the playbook's own content sources -- the spec pages _are_ the site's pages -- plus a new `coverage:` section naming what to scan:"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "The database is *ephemeral* -- recomputed by every scan, never committed; `--format codecov` is the only persisted export."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "every source line of a `verified` block emits `1`; every source line of a `planned`, `uncovered`, or `unclassified` block emits `0`; lines of `out-of-scope` and `non-normative` blocks, and lines belonging to no block (blank lines, delimiters), are omitted"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Coverage database* -- ephemeral, recomputed by every `scan` and never committed; `--format codecov` is the only persisted export"
    );

    // The playbook's `coverage:` section, as the RFC writes it.
    let dir = std::env::temp_dir().join(format!("bokfell-rfc-playbook-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let playbook = dir.join("bokfell.yml");
    std::fs::write(
        &playbook,
        "site:\n  title: Spec\ncontent:\n  sources:\n  - path: docs\n\
         coverage:\n  scan:\n\
         \x20   - repo: https://github.com/asciidoc-rs/asciidoc-html5\n\
         \x20     tests: [html5/src/tests, cli/src/tests]\n\
         \x20     spec_map: spec-map\n\
         \x20   - repo: https://github.com/asciidoc-rs/asciidoc-parser\n\
         \x20     tests: [parser/src/tests]\n\
         \x20     spec_map: spec-map\n",
    )
    .unwrap();
    let playbook = Playbook::load(&playbook).unwrap();
    assert_eq!(playbook.coverage.scan.len(), 2);
    assert_eq!(
        playbook.coverage.scan[0].tests,
        ["html5/src/tests", "cli/src/tests"]
    );
    assert_eq!(
        playbook.coverage.scan[1].spec_map.as_deref(),
        Some("spec-map")
    );
    std::fs::remove_dir_all(&dir).ok();

    // The database round-trips through JSON (the ephemeral form) and
    // the Codecov export projects block states onto lines.
    let map = sidecar(
        "reviewed = true\n\
         [[out-of-scope]]\nexcerpt = \"DocBook\"\nreason = \"HTML5 only\"\n\
         [[planned]]\nexcerpt = \"harmless\"\ntracking = \"o/r#1\"\n",
    );
    let db = resolve(
        &[rules_page(None)],
        vec![claim("t.rs", 1, "rules.adoc", Some("MUST be frobbed"))],
        &[map],
    );
    let db = CoverageDatabase::from_json(&db.to_json()).unwrap();
    let json = bokfell_coverage::report::codecov_json(&db);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let lines = &value["coverage"]["docs/modules/ROOT/pages/rules.adoc"];

    // Verified block (lines 7–8) → 1; planned (14–15) and uncovered
    // (3–4) → 0; out-of-scope (18–19), the listing (10–11), and the
    // blank lines between blocks are absent.
    assert_eq!(lines["7"], 1);
    assert_eq!(lines["8"], 1);
    assert_eq!(lines["14"], 0);
    assert_eq!(lines["3"], 0);
    assert!(lines.get("18").is_none());
    assert!(lines.get("10").is_none());
    assert!(lines.get("5").is_none());
    assert_eq!(lines.as_object().unwrap().len(), 6);
}

#[test]
fn excerpts_normalize_whitespace_only() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Excerpt normalization* -- whitespace collapse only. Excerpts match the parsed block's source text with inline markup intact"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Excerpt* is matched against the block's *source* text after whitespace normalization"
    );
    assert_eq!(
        normalize_whitespace("  keep *markup*\n\tintact  "),
        "keep *markup* intact"
    );
    assert!(excerpt_matches(
        "see xref:a.adoc[A] for `code`",
        "xref:a.adoc[A] for `code`"
    ));
    assert!(!excerpt_matches("see xref:a.adoc[A]", "see A"));
}

#[test]
fn scanning_a_real_test_root() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Whole-section claims* are for tests that genuinely exercise an entire section"
    );

    // The engine's own test root, scanned the way `bokfell coverage
    // scan` scans it, yields this file's claims against the RFC.
    let claims = scan_test_root(&TestRoot {
        dir: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests"),
        repo_prefix: "coverage/tests".into(),
        local: true,
        repo: None,
        rev: None,
        scope: 0,
        krate: None,
    })
    .unwrap();
    let mine: Vec<&Claim> = claims
        .iter()
        .filter(|c| c.site.file == "coverage/tests/rfc_0001.rs")
        .collect();
    assert!(mine.len() >= 30, "{}", mine.len());
    assert!(mine
        .iter()
        .all(|c| c.target == ClaimTarget::Path(RFC.into())));
    assert!(mine
        .iter()
        .any(|c| c.site.test_fn.as_deref() == Some("scanning_a_real_test_root")));
    assert!(mine
        .iter()
        .all(|c| c.site.krate.as_deref() == Some("bokfell-coverage")));
}
