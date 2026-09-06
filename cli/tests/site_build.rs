//! End-to-end pipeline test: a small fixture component goes through
//! catalog → render → theme, and the composed pages carry resolved
//! cross-references, includes, navigation, and assets.

use std::path::PathBuf;

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
        })
        .unwrap();
    assert!(html.contains("<title>Setting Up :: Demo</title>"));
    assert!(html.contains("<a href=\"setup.html\" class=\"current\">Setup</a>"));
    assert!(html.contains("href=\"../../_/bokfell.css\""));

    std::fs::remove_dir_all(&root).ok();
}
