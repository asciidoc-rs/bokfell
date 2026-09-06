//! The Bokfell command line.
//!
//! `bokfell build` runs the M1 pipeline: playbook → content catalog →
//! render → theme composition → static site. `bokfell serve` runs the same
//! pipeline into memory behind a watching dev server with live reload
//! (M2). The `diff` and `coverage` commands arrive with later milestones
//! (PLAN.md §10).

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};
use bokfell_model::{ContentCatalog, Coords, Family, NavTree, Playbook, ResourceRef};
use bokfell_render::Pipeline;
use bokfell_theme::{PageContext, Theme};
use clap::{Parser as ClapParser, Subcommand};

#[derive(ClapParser)]
#[command(
    name = "bokfell",
    version,
    about = "Documentation site generator for AsciiDoc"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build the site described by a playbook.
    Build {
        /// Path to the playbook file.
        #[arg(short, long, default_value = "bokfell.yml")]
        playbook: PathBuf,

        /// Output directory (overrides the playbook's `output.dir`).
        #[arg(short, long)]
        out: Option<PathBuf>,

        /// Theme override directory (a `layout.html` there replaces the
        /// built-in layout).
        #[arg(long)]
        theme: Option<PathBuf>,
    },

    /// Build into memory and serve with file watching and live reload.
    Serve {
        /// Path to the playbook file.
        #[arg(short, long, default_value = "bokfell.yml")]
        playbook: PathBuf,

        /// Theme override directory.
        #[arg(long)]
        theme: Option<PathBuf>,

        /// Port to serve on (binds 127.0.0.1).
        #[arg(long, default_value_t = 8000)]
        port: u16,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Build {
            playbook,
            out,
            theme,
        } => build(&playbook, out.as_deref(), theme.as_deref()),
        Command::Serve {
            playbook,
            theme,
            port,
        } => serve(&playbook, theme.as_deref(), port),
    }
}

/// One composed site output: `(site-root-relative URL, bytes)`.
type SiteFiles = Vec<(String, Vec<u8>)>;

/// Runs playbook → catalog → render → theme and returns every site file.
///
/// Warnings are printed to stderr as they surface; the count is returned
/// alongside the files.
fn compose_site(playbook_path: &Path, theme_dir: Option<&Path>) -> anyhow::Result<SiteFiles> {
    let playbook = Playbook::load(playbook_path)?;

    let mut catalog = ContentCatalog::new();
    for root in playbook.source_roots() {
        catalog
            .scan_source(&root)
            .with_context(|| format!("scanning content source {}", root.display()))?;
    }
    if catalog.components().is_empty() {
        bail!("no components found in the playbook's content sources");
    }

    let pipeline = Pipeline::new(catalog, playbook.asciidoc.attribute_seeds());
    let site = pipeline.render_site()?;
    let theme = Theme::load(theme_dir)?;

    let navs: std::collections::HashMap<&str, &NavTree> = site
        .navs
        .iter()
        .map(|(name, tree)| (name.as_str(), tree))
        .collect();
    let empty_nav = NavTree::default();
    let start_url = start_page_url(&playbook, pipeline.catalog(), &site.pages)?;

    let mut files: SiteFiles = Vec::new();

    for page in &site.pages {
        for warning in &page.warnings {
            eprintln!("warning: {}: {warning}", page.coords);
        }

        let nav = navs
            .get(page.coords.component.as_str())
            .copied()
            .unwrap_or(&empty_nav);

        let html = theme.compose_page(&PageContext {
            site_title: &playbook.site.title,
            url: &page.url,
            title_html: page.title_html.as_deref(),
            title_text: page.title_text.as_deref(),
            contents: &page.contents,
            nav,
            home_url: "index.html",
        })?;
        files.push((page.url.clone(), html.into_bytes()));
    }

    // Published static resources (images, attachments).
    for family in [Family::Image, Family::Attachment] {
        for file in pipeline.catalog().files_of(family) {
            if let Some(url) = &file.url {
                let bytes = std::fs::read(&file.src_path)
                    .with_context(|| format!("reading {}", file.src_path.display()))?;
                files.push((url.clone(), bytes));
            }
        }
    }

    // Theme assets and the root redirect.
    files.extend(theme.assets());
    files.push((
        "index.html".to_string(),
        Theme::redirect_page(&start_url).into_bytes(),
    ));

    Ok(files)
}

fn build(playbook_path: &Path, out: Option<&Path>, theme_dir: Option<&Path>) -> anyhow::Result<()> {
    let playbook = Playbook::load(playbook_path)?;
    let out_dir = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| playbook.output_dir());

    let files = compose_site(playbook_path, theme_dir)?;
    let count = files.len();
    for (url, bytes) in files {
        write_output(&out_dir, &url, &bytes)?;
    }

    println!(
        "Site generation complete: {count} files → {}",
        out_dir.display()
    );
    Ok(())
}

fn serve(playbook_path: &Path, theme_dir: Option<&Path>, port: u16) -> anyhow::Result<()> {
    // Watch the playbook, every content source root, and the theme
    // directory.
    let playbook = Playbook::load(playbook_path)?;
    let mut watch: Vec<PathBuf> = vec![playbook_path.to_path_buf()];
    watch.extend(playbook.source_roots());
    if let Some(dir) = theme_dir {
        watch.push(dir.to_path_buf());
    }

    let playbook_path = playbook_path.to_path_buf();
    let theme_dir = theme_dir.map(Path::to_path_buf);
    let builder: bokfell_serve::SiteBuilder = Box::new(move || {
        compose_site(&playbook_path, theme_dir.as_deref())
            .map(|files| files.into_iter().collect())
            .map_err(|e| format!("{e:#}"))
    });

    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    bokfell_serve::serve(
        builder,
        bokfell_serve::ServeOptions {
            addr,
            watch,
            ..Default::default()
        },
    )?;
    Ok(())
}

/// Resolves the playbook's `site.start_page` (or falls back to the first
/// page) to a site-root-relative URL.
fn start_page_url(
    playbook: &Playbook,
    catalog: &ContentCatalog,
    pages: &[bokfell_render::RenderedPage],
) -> anyhow::Result<String> {
    if let Some(spec) = &playbook.site.start_page {
        let reference = ResourceRef::parse(spec)
            .with_context(|| format!("invalid site.start_page {spec:?}"))?;

        // A start page spec should be fully qualified; default missing
        // coordinates from the first component.
        let first = &catalog.components()[0].desc;
        let from = Coords {
            component: first.name.clone(),
            version: first.version.clone(),
            module: "ROOT".to_string(),
            family: Family::Page,
            path: String::new(),
        };

        let file = catalog
            .resolve(&reference, &from, Family::Page)
            .with_context(|| format!("site.start_page {spec:?} does not match any page"))?;
        return file.url.clone().context("start page has no published URL");
    }

    pages
        .first()
        .map(|p| p.url.clone())
        .context("site has no pages")
}

fn write_output(out_dir: &Path, url: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let path = out_dir.join(url);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
