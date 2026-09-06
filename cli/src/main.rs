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
use bokfell_aggregate::{Aggregator, GitSource};
use bokfell_model::{ContentCatalog, Coords, Family, NavTree, Playbook, ResourceRef};
use bokfell_render::Pipeline;
use bokfell_theme::{PageContext, Theme, VersionLink};
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

        /// Refresh cached remote repositories before building.
        #[arg(long)]
        fetch: bool,
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

        /// Refresh cached remote repositories before the initial build.
        #[arg(long)]
        fetch: bool,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Build {
            playbook,
            out,
            theme,
            fetch,
        } => build(&playbook, out.as_deref(), theme.as_deref(), fetch),
        Command::Serve {
            playbook,
            theme,
            port,
            fetch,
        } => serve(&playbook, theme.as_deref(), port, fetch),
    }
}

/// One composed site output: `(site-root-relative URL, bytes)`.
type SiteFiles = Vec<(String, Vec<u8>)>;

/// Runs playbook → catalog → render → theme and returns every site file.
///
/// Warnings are printed to stderr as they surface; the count is returned
/// alongside the files.
fn compose_site(
    playbook_path: &Path,
    theme_dir: Option<&Path>,
    fetch: bool,
) -> anyhow::Result<SiteFiles> {
    let playbook = Playbook::load(playbook_path)?;

    let cache_dir = playbook
        .runtime
        .cache_dir
        .as_deref()
        .map(|dir| playbook.resolve_path(dir))
        .unwrap_or_else(Aggregator::default_cache_dir);
    let aggregator = Aggregator::new(cache_dir, fetch || playbook.runtime.fetch);

    let mut catalog = ContentCatalog::new();
    for source in &playbook.content.sources {
        source.validate().map_err(anyhow::Error::msg)?;

        if let Some(path) = &source.path {
            let root = playbook.resolve_path(path);
            catalog
                .scan_source(&root)
                .with_context(|| format!("scanning content source {}", root.display()))?;
            continue;
        }

        // A git source: aggregate each matched ref into a content root.
        let url = source.url.clone().expect("validated: url set");
        let url = {
            // Relative local paths resolve against the playbook.
            let as_path = Path::new(&url);
            if as_path.is_relative() && playbook.resolve_path(as_path).join(".git").exists() {
                playbook.resolve_path(as_path).display().to_string()
            } else {
                url
            }
        };
        let git_source = GitSource {
            url: url.clone(),
            branches: source.branches.clone(),
            tags: source.tags.clone(),
            start_path: source.start_path.clone(),
            version_from_ref: source.version_from_ref,
        };
        let roots = aggregator.collect(&git_source)?;
        for root in roots {
            catalog
                .scan_source_versioned(&root.path, root.version_override.as_deref())
                .with_context(|| {
                    format!(
                        "scanning {} ref {} ({})",
                        url,
                        root.refname,
                        root.path.display()
                    )
                })?;
        }
    }
    if catalog.components().is_empty() {
        bail!("no components found in the playbook's content sources");
    }

    let pipeline = Pipeline::new(catalog, playbook.asciidoc.attribute_seeds());
    let site = pipeline.render_site()?;
    let theme = Theme::load(theme_dir)?;

    let navs: std::collections::HashMap<(&str, Option<&str>), &NavTree> = site
        .navs
        .iter()
        .map(|((name, version), tree)| ((name.as_str(), version.as_deref()), tree))
        .collect();
    let empty_nav = NavTree::default();
    let start_url = start_page_url(&playbook, pipeline.catalog(), &site.pages)?;

    let mut files: SiteFiles = Vec::new();

    for page in &site.pages {
        for warning in &page.warnings {
            eprintln!("warning: {}: {warning}", page.coords);
        }

        let nav = navs
            .get(&(
                page.coords.component.as_str(),
                page.coords.version.as_deref(),
            ))
            .copied()
            .unwrap_or(&empty_nav);

        // The page-version selector: this page across the component's
        // versions, highest first.
        let versions: Vec<VersionLink> = pipeline
            .catalog()
            .versions_of_resource(&page.coords)
            .into_iter()
            .map(|(component, file)| VersionLink {
                label: component
                    .desc
                    .display_version
                    .clone()
                    .or_else(|| component.desc.version.clone())
                    .unwrap_or_else(|| "default".to_string()),
                url: file.and_then(|f| f.url.clone()),
                current: component.desc.version == page.coords.version,
            })
            .collect();

        let html = theme.compose_page(&PageContext {
            site_title: &playbook.site.title,
            url: &page.url,
            title_html: page.title_html.as_deref(),
            title_text: page.title_text.as_deref(),
            contents: &page.contents,
            nav,
            home_url: "index.html",
            versions: &versions,
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

fn build(
    playbook_path: &Path,
    out: Option<&Path>,
    theme_dir: Option<&Path>,
    fetch: bool,
) -> anyhow::Result<()> {
    let playbook = Playbook::load(playbook_path)?;
    let out_dir = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| playbook.output_dir());

    let files = compose_site(playbook_path, theme_dir, fetch)?;
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

fn serve(
    playbook_path: &Path,
    theme_dir: Option<&Path>,
    port: u16,
    fetch: bool,
) -> anyhow::Result<()> {
    // Watch the playbook, every *local directory* content source, and the
    // theme directory (git sources are cache-backed and not watched).
    let playbook = Playbook::load(playbook_path)?;
    let mut watch: Vec<PathBuf> = vec![playbook_path.to_path_buf()];
    for source in &playbook.content.sources {
        if let Some(path) = &source.path {
            watch.push(playbook.resolve_path(path));
        }
    }
    if let Some(dir) = theme_dir {
        watch.push(dir.to_path_buf());
    }

    let playbook_path = playbook_path.to_path_buf();
    let theme_dir = theme_dir.map(Path::to_path_buf);
    let mut first = fetch;
    let builder: bokfell_serve::SiteBuilder = Box::new(move || {
        let fetch_now = std::mem::take(&mut first);
        compose_site(&playbook_path, theme_dir.as_deref(), fetch_now)
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
