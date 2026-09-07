//! End-to-end pipeline test: a small fixture component goes through
//! catalog → render → theme, and the composed pages carry resolved
//! cross-references, includes, navigation, and assets.

use std::path::PathBuf;

use bokfell_coverage::{BlockStatus, CoverageData, CoverageScope};
use bokfell_model::{ContentCatalog, Family};
use bokfell_render::Pipeline;
use bokfell_theme::{PageContext, Theme};

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn fixture_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bokfell-e2e-{tag}-{}", std::process::id()));

    write(
        &dir.join("antora.yml"),
        "name: demo\n\
         title: Demo Docs\n\
         version: ~\n\
         nav:\n\
         - modules/ROOT/nav.adoc\n",
    );
    write(
        &dir.join("modules/ROOT/nav.adoc"),
        "* xref:index.adoc[]\n\
         * Guides\n\
         ** xref:guide:setup.adoc[Setup]\n",
    );
    write(
        &dir.join("modules/ROOT/pages/index.adoc"),
        "= Demo Home\n\
         :navtitle: Home\n\
         \n\
         See xref:guide:setup.adoc[] and xref:guide:setup.adoc#deeper[the details].\n\
         \n\
         include::partial$shared.adoc[]\n",
    );
    write(
        &dir.join("modules/ROOT/partials/shared.adoc"),
        "A shared sentence.\n",
    );
    write(
        &dir.join("modules/guide/pages/setup.adoc"),
        "= Setting Up\n\
         \n\
         Back to xref:ROOT:index.adoc[].\n\
         \n\
         [#deeper]\n\
         == Deeper Section\n\
         \n\
         image::diagram.png[A diagram]\n",
    );
    std::fs::create_dir_all(dir.join("modules/guide/images")).unwrap();
    std::fs::write(dir.join("modules/guide/images/diagram.png"), b"png").unwrap();

    dir
}

#[test]
fn builds_a_cross_referenced_site() {
    let root = fixture_root("main");

    let mut catalog = ContentCatalog::new();
    catalog.scan_source(&root).unwrap();
    let pipeline = Pipeline::new(catalog, Vec::new());
    let site = pipeline.render_site().unwrap();

    assert_eq!(site.pages.len(), 2);
    let index = site
        .pages
        .iter()
        .find(|p| p.url == "demo/index.html")
        .expect("index page rendered");
    let setup = site
        .pages
        .iter()
        .find(|p| p.url == "demo/guide/setup.html")
        .expect("setup page rendered");

    for page in &site.pages {
        assert!(
            page.warnings.is_empty(),
            "unexpected warnings on {}: {:?}",
            page.url,
            page.warnings
        );
    }

    // Cross-module xref with default text (the target's title) and with a
    // fragment (the fragment target's reference text).
    assert!(
        index
            .contents
            .contains("<a href=\"guide/setup.html\">Setting Up</a>"),
        "index contents: {}",
        index.contents
    );
    assert!(index
        .contents
        .contains("<a href=\"guide/setup.html#deeper\">the details</a>"));

    // The include was served from the partials family.
    assert!(index.contents.contains("A shared sentence."));

    // Rendered blocks carry data-source-line anchors (asciidoc-html5
    // 0.2.2), and they line up with the overlay walk's block lines.
    assert!(
        index.contents.contains("data-source-line=\"4\""),
        "contents: {}",
        index.contents
    );
    assert_eq!(index.block_lines.len(), 2);
    assert_eq!(index.block_lines[0], 4);

    // The reverse xref climbs out of the module directory.
    assert!(setup
        .contents
        .contains("<a href=\"../index.html\">Demo Home</a>"));

    // The image resolves through the module's published _images directory.
    assert!(setup.contents.contains("src=\"_images/diagram.png\""));

    // The image itself is a publishable file.
    let image = pipeline
        .catalog()
        .files_of(Family::Image)
        .next()
        .expect("image cataloged");
    assert_eq!(image.url.as_deref(), Some("demo/guide/_images/diagram.png"));

    // Navigation: explicit text, navtitle fallback, nesting.
    let (_, nav) = &site.navs[0];
    assert_eq!(nav.items.len(), 2);
    assert_eq!(nav.items[0].text, "Home");
    assert_eq!(nav.items[0].url.as_deref(), Some("demo/index.html"));
    assert_eq!(nav.items[1].text, "Guides");
    assert_eq!(nav.items[1].url, None);
    assert_eq!(nav.items[1].children[0].text, "Setup");
    assert_eq!(
        nav.items[1].children[0].url.as_deref(),
        Some("demo/guide/setup.html")
    );

    // Theme composition ties it together.
    let theme = Theme::default_theme().unwrap();
    let html = theme
        .compose_page(&PageContext {
            site_title: "Demo",
            url: &setup.url,
            title_html: setup.title_html.as_deref(),
            title_text: setup.title_text.as_deref(),
            contents: &setup.contents,
            nav,
            home_url: "index.html",
            versions: &[],
            coverage: None,
            diff: None,
            overlay_json: None,
        })
        .unwrap();
    assert!(html.contains("<title>Setting Up :: Demo</title>"));
    assert!(html.contains("<a href=\"setup.html\" class=\"current\">Setup</a>"));
    assert!(html.contains("href=\"../../_/bokfell.css\""));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn projects_coverage_onto_rendered_blocks() {
    let root = fixture_root("coverage");

    // Coverage for the index page: the title line is verified, the xref
    // paragraph (line 4) is normative but uncovered. The included
    // paragraph's lines belong to the partial, not the page.
    let mut coverage = CoverageData::new();
    coverage
        .load_str(
            r#"{ "coverage": {
                "docs/modules/ROOT/pages/index.adoc": { "1": 1, "4": 0 }
            } }"#,
        )
        .unwrap();

    let mut catalog = ContentCatalog::new();
    let key = catalog.scan_source(&root).unwrap();
    assert_eq!(key, ("demo".to_string(), None));

    // A second scope with identical keys but a different component must
    // never leak onto this source's pages: were it consulted, every line
    // would read verified.
    let mut foreign = CoverageData::new();
    foreign
        .load_str(
            r#"{ "coverage": {
                "docs/modules/ROOT/pages/index.adoc": { "1": 1, "4": 1 }
            } }"#,
        )
        .unwrap();

    let pipeline = Pipeline::new(catalog, Vec::new()).with_coverage(vec![
        CoverageScope {
            data: foreign,
            prefix: "docs".to_string(),
            components: vec![("other".to_string(), Some("2.0".to_string()))],
        },
        CoverageScope {
            data: coverage,
            prefix: "docs".to_string(),
            components: vec![key],
        },
    ]);
    let site = pipeline.render_site().unwrap();

    let index = site
        .pages
        .iter()
        .find(|p| p.url == "demo/index.html")
        .expect("index page rendered");
    let setup = site
        .pages
        .iter()
        .find(|p| p.url == "demo/guide/setup.html")
        .expect("setup page rendered");

    // The index has two overlay blocks (its own paragraph plus the
    // included one); the xref paragraph is uncovered, the include-origin
    // paragraph carries no coverage of this page's lines.
    let index_coverage = index.coverage.as_ref().expect("index coverage");
    assert_eq!(
        index_coverage.blocks,
        vec![Some(BlockStatus::Uncovered), None]
    );
    assert_eq!(index_coverage.verified, 1);
    assert_eq!(index_coverage.uncovered, 1);
    assert_eq!(index_coverage.percent_verified(), 50);

    // Pages without coverage data stay overlay-free.
    assert!(setup.coverage.is_none());

    // Click-to-source targets (PLAN.md §9.3): the index's own paragraph
    // points into index.adoc at its line, while the included paragraph
    // resolves through the catalog to the partial it came from.
    assert_eq!(index.block_sources.len(), 2);
    let own = index.block_sources[0].as_ref().expect("own block source");
    assert!(own.0.ends_with("modules/ROOT/pages/index.adoc"), "{own:?}");
    assert_eq!(own.1, 4);
    let included = index.block_sources[1].as_ref().expect("included source");
    assert!(
        included.0.ends_with("modules/ROOT/partials/shared.adoc"),
        "{included:?}"
    );
    assert_eq!(included.1, 1);

    std::fs::remove_dir_all(&root).ok();
}
