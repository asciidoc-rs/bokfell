//! End-to-end git aggregation test: a fixture repository with a branch and
//! a tag becomes a two-version component, with the branch as latest.

use std::{path::Path, process::Command};

use bokfell_aggregate::{Aggregator, GitSource};
use bokfell_model::{ContentCatalog, Coords, Family};
use bokfell_render::Pipeline;

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

    // Rendered pages carry versioned URLs and per-ref content.
    let site = Pipeline::new(catalog, Vec::new()).render_site().unwrap();
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

    // Aggregating again reuses the commit-keyed export cache.
    let again = aggregator.collect(&source).unwrap();
    assert_eq!(again.len(), 2);

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

    std::fs::remove_dir_all(&base).ok();
}
