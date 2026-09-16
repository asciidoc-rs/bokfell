//! End-to-end git aggregation test: a fixture repository with a branch and
//! a tag becomes a two-version component, with the branch as latest.

use std::{path::Path, process::Command};

use bokfell_aggregate::{file_matches_head, local_repo_info, Aggregator, GitSource};
use bokfell_model::{ContentCatalog, Coords, Family};
use bokfell_render::{DiffBase, PageBlockChange, Pipeline};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn aggregates_branch_and_tag_as_versions() {
    let base = std::env::temp_dir().join(format!("bokfell-git-e2e-{}", std::process::id()));
    let repo = base.join("repo");
    let cache = base.join("cache");
    std::fs::remove_dir_all(&base).ok();
    std::fs::create_dir_all(&repo).unwrap();

    // Fixture history: v1.0.0 says "old", main says "new" — and each ref
    // carries its own component attributes, so descriptor selection per
    // version is observable in the rendered output.
    git(&repo, &["init", "-q", "-b", "main"]);
    write(
        &repo.join("docs/antora.yml"),
        "name: demo\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n\
         asciidoc:\n  attributes:\n    flavor: vintage\n",
    );
    write(
        &repo.join("docs/modules/ROOT/nav.adoc"),
        "* xref:index.adoc[]\n",
    );
    write(
        &repo.join("docs/modules/ROOT/pages/index.adoc"),
        "= Demo\n\nThe old wording ({flavor}).\n",
    );
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    git(&repo, &["tag", "v1.0.0"]);
    write(
        &repo.join("docs/antora.yml"),
        "name: demo\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n\
         asciidoc:\n  attributes:\n    flavor: fresh\n",
    );
    write(
        &repo.join("docs/modules/ROOT/pages/index.adoc"),
        "= Demo\n\nThe new wording ({flavor}).\n\nOnly on main: xref:extra.adoc[].\n",
    );
    write(
        &repo.join("docs/modules/ROOT/pages/extra.adoc"),
        "= Extra\n\nNew page.\n",
    );
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "two"]);

    // Aggregate both refs.
    let aggregator = Aggregator::new(cache, false);
    let source = GitSource {
        url: repo.display().to_string(),
        branches: vec!["main".to_string()],
        tags: vec!["v*".to_string()],
        start_path: "docs".to_string(),
        version_from_ref: true,
    };
    let roots = aggregator.collect(&source).unwrap();
    assert_eq!(roots.len(), 2, "roots: {roots:?}");

    let mut catalog = ContentCatalog::new();
    for root in &roots {
        catalog
            .scan_source_versioned(&root.path, root.version_override.as_deref())
            .unwrap();
    }

    // Version model: two versions, named branch outranks semver tag.
    let versions: Vec<Option<&str>> = catalog
        .versions_of("demo")
        .iter()
        .map(|c| c.desc.version.as_deref())
        .collect();
    assert_eq!(versions, [Some("main"), Some("1.0.0")]);
    assert_eq!(
        catalog.latest_of("demo").unwrap().desc.version.as_deref(),
        Some("main")
    );

    // The same page exists in both versions; the extra page only on main.
    let index_coords = Coords {
        component: "demo".to_string(),
        version: Some("main".to_string()),
        module: "ROOT".to_string(),
        family: Family::Page,
        path: "index.adoc".to_string(),
    };
    let per_version = catalog.versions_of_resource(&index_coords);
    assert!(per_version.iter().all(|(_, file)| file.is_some()));

    let extra_coords = Coords {
        path: "extra.adoc".to_string(),
        ..index_coords.clone()
    };
    let per_version = catalog.versions_of_resource(&extra_coords);
    assert_eq!(per_version.len(), 2);
    assert!(per_version[0].1.is_some(), "extra page exists on main");
    assert!(per_version[1].1.is_none(), "extra page absent in 1.0.0");

    // Rendered pages carry versioned URLs and per-ref content; diffing
    // against the previous version is on.
    let site = Pipeline::new(catalog, Vec::new())
        .with_diff(DiffBase::PreviousVersion)
        .render_site()
        .unwrap();
    let main_index = site
        .pages
        .iter()
        .find(|p| p.url == "demo/main/index.html")
        .expect("main index rendered");
    let old_index = site
        .pages
        .iter()
        .find(|p| p.url == "demo/1.0.0/index.html")
        .expect("1.0.0 index rendered");
    // Content AND descriptor attributes are the matched version's own:
    // the tag's pages must see the tag's `flavor`, not main's.
    assert!(
        main_index.contents.contains("The new wording (fresh)."),
        "main index: {}",
        main_index.contents
    );
    assert!(
        old_index.contents.contains("The old wording (vintage)."),
        "1.0.0 index: {}",
        old_index.contents
    );

    // Both versions got their own nav tree.
    assert_eq!(site.navs.len(), 2);

    // Version-pair diffs (PLAN.md §9.1): main's index changed against
    // 1.0.0 — the wording paragraph was edited and a paragraph added —
    // while 1.0.0, the oldest version, has nothing to diff against.
    let diff = main_index.diff.as_ref().expect("main index diff");
    assert_eq!(diff.base_label, "1.0.0");
    assert!(!diff.new_page);
    assert_eq!((diff.added, diff.removed, diff.edited), (1, 0, 1));
    assert!(diff.blocks.iter().any(|b| matches!(
        b,
        PageBlockChange::Edited { diff_html }
            if diff_html.contains("<del>old</del>") && diff_html.contains("<ins>new</ins>")
    )));
    assert!(old_index.diff.is_none());

    // The page that only exists on main is a new page.
    let extra = site
        .pages
        .iter()
        .find(|p| p.url == "demo/main/extra.html")
        .expect("extra page rendered");
    let extra_diff = extra.diff.as_ref().expect("extra page diff");
    assert!(extra_diff.new_page);
    assert_eq!(extra_diff.base_label, "1.0.0");

    // PR-preview mode (PLAN.md §9.1): the same content diffed against an
    // explicit base ref instead of the previous version.
    let mut head_catalog = ContentCatalog::new();
    let head_roots = aggregator
        .collect(&GitSource {
            branches: vec!["main".to_string()],
            tags: Vec::new(),
            ..source.clone()
        })
        .unwrap();
    for root in &head_roots {
        head_catalog
            .scan_source_versioned(&root.path, root.version_override.as_deref())
            .unwrap();
    }
    let mut base_catalog = ContentCatalog::new();
    let base_roots = aggregator
        .collect(&GitSource {
            branches: Vec::new(),
            tags: vec!["v1.0.0".to_string()],
            version_from_ref: false,
            ..source.clone()
        })
        .unwrap();
    for root in &base_roots {
        base_catalog
            .scan_source_versioned(&root.path, root.version_override.as_deref())
            .unwrap();
    }

    let site = Pipeline::new(head_catalog, Vec::new())
        .with_diff(DiffBase::Catalog {
            pipeline: Box::new(Pipeline::new(base_catalog, Vec::new())),
            label: "v1.0.0".to_string(),
        })
        .render_site()
        .unwrap();
    let index = site
        .pages
        .iter()
        .find(|p| p.url == "demo/main/index.html")
        .expect("head index rendered");
    let diff = index.diff.as_ref().expect("head index diff");
    assert_eq!(diff.base_label, "v1.0.0");
    assert_eq!((diff.added, diff.removed, diff.edited), (1, 0, 1));

    // Aggregating again reuses the commit-keyed export cache.
    let again = aggregator.collect(&source).unwrap();
    assert_eq!(again.len(), 2);

    // A single named ref via collect_ref: even with a tag named like
    // the branch, the ref's content is aggregated exactly once (the
    // branch wins), so a --diff-base build never scans duplicates.
    git(&repo, &["tag", "main"]);
    let by_name = aggregator
        .collect_ref(
            &GitSource {
                branches: Vec::new(),
                tags: Vec::new(),
                ..source.clone()
            },
            "main",
        )
        .unwrap();
    assert_eq!(by_name.len(), 1, "roots: {by_name:?}");
    let by_tag = aggregator
        .collect_ref(
            &GitSource {
                branches: Vec::new(),
                tags: Vec::new(),
                ..source.clone()
            },
            "v1.0.0",
        )
        .unwrap();
    assert_eq!(by_tag.len(), 1);
    assert!(matches!(
        aggregator.collect_ref(
            &GitSource {
                branches: Vec::new(),
                tags: Vec::new(),
                ..source.clone()
            },
            "no-such-ref",
        ),
        Err(bokfell_aggregate::AggregateError::NoMatchingRef { .. })
    ));

    // The name is literal: a pattern would break the single-ref contract
    // (it could match several branches), so it is rejected outright.
    assert!(matches!(
        aggregator.collect_ref(
            &GitSource {
                branches: Vec::new(),
                tags: Vec::new(),
                ..source.clone()
            },
            "v*",
        ),
        Err(bokfell_aggregate::AggregateError::PatternRefName { .. })
    ));

    // A negative pattern excludes the branch HEAD resolves to.
    let negated = GitSource {
        branches: vec!["HEAD".to_string(), "!main".to_string()],
        tags: Vec::new(),
        ..source.clone()
    };
    assert!(matches!(
        aggregator.collect(&negated),
        Err(bokfell_aggregate::AggregateError::NoMatchingRef { .. })
    ));

    // Single files read at a ref without an export (how the coverage scan
    // finds a remote test root's `Cargo.toml`): the branch wins over the
    // same-named tag, a tag resolves when no branch matches, and a path
    // that is missing at the ref — or names a tree — is `None`.
    let plain = GitSource {
        branches: Vec::new(),
        tags: Vec::new(),
        ..source.clone()
    };
    let extra = aggregator
        .read_blob(&plain, "main", "docs/modules/ROOT/pages/extra.adoc")
        .unwrap();
    assert_eq!(extra.as_deref(), Some(b"= Extra\n\nNew page.\n".as_slice()));
    let old = aggregator
        .read_blob(&plain, "v1.0.0", "docs/modules/ROOT/pages/index.adoc")
        .unwrap();
    assert!(
        String::from_utf8(old.unwrap())
            .unwrap()
            .contains("The old wording"),
        "tag content"
    );
    assert_eq!(
        aggregator
            .read_blob(&plain, "v1.0.0", "docs/modules/ROOT/pages/extra.adoc")
            .unwrap(),
        None
    );
    assert_eq!(aggregator.read_blob(&plain, "main", "docs").unwrap(), None);
    assert!(matches!(
        aggregator.read_blob(&plain, "no-such-ref", "docs/antora.yml"),
        Err(bokfell_aggregate::AggregateError::NoMatchingRef { .. })
    ));
    assert!(matches!(
        aggregator.read_blob(&plain, "v*", "docs/antora.yml"),
        Err(bokfell_aggregate::AggregateError::PatternRefName { .. })
    ));

    // Worktree files compared against HEAD: identical, edited, untracked,
    // missing, and outside any repository.
    let extra_path = "docs/modules/ROOT/pages/extra.adoc";
    assert_eq!(file_matches_head(&repo, extra_path), Some(true));
    write(&repo.join(extra_path), "= Extra\n\nEdited locally.\n");
    assert_eq!(file_matches_head(&repo, extra_path), Some(false));
    write(&repo.join("docs/untracked.adoc"), "= Untracked\n");
    assert_eq!(file_matches_head(&repo, "docs/untracked.adoc"), Some(false));
    assert_eq!(file_matches_head(&repo, "docs/missing.adoc"), None);
    assert_eq!(file_matches_head(&base, "repo/docs/antora.yml"), None);

    // The repository's own provenance, for worktree scans.
    let info = local_repo_info(&repo.join("docs")).expect("inside the fixture repo");
    assert_eq!(
        info.root.canonicalize().unwrap(),
        repo.canonicalize().unwrap()
    );
    assert!(info.head.is_some_and(|h| h.len() == 40), "head commit id");
    assert!(local_repo_info(&base).is_none());

    std::fs::remove_dir_all(&base).ok();
}
