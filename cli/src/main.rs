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
use bokfell_coverage::{BlockStatus, CoverageData, CoverageScope, PageCoverage};
use bokfell_model::{relative_url, ContentCatalog, Coords, Family, NavTree, Playbook, ResourceRef};
use bokfell_render::{Pipeline, RenderedPage};
use bokfell_theme::{coverage_level, escape_html, CoverageView, PageContext, Theme, VersionLink};
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

/// The site-root-relative URL of the coverage dashboard page.
const COVERAGE_DASHBOARD_URL: &str = "coverage.html";

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
    let mut coverage_scopes: Vec<CoverageScope> = Vec::new();
    for source in &playbook.content.sources {
        source.validate().map_err(anyhow::Error::msg)?;

        // The component versions this source contributes, so its coverage
        // (if any) applies to exactly those pages and no others.
        let mut contributed: Vec<(String, Option<String>)> = Vec::new();

        if let Some(path) = &source.path {
            let root = playbook.resolve_path(path);
            let key = catalog
                .scan_source(&root)
                .with_context(|| format!("scanning content source {}", root.display()))?;
            contributed.push(key);
        } else {
            // A git source: aggregate each matched ref into a content
            // root.
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
                let key = catalog
                    .scan_source_versioned(&root.path, root.version_override.as_deref())
                    .with_context(|| {
                        format!(
                            "scanning {} ref {} ({})",
                            url,
                            root.refname,
                            root.path.display()
                        )
                    })?;
                contributed.push(key);
            }
        }

        // Spec coverage (PLAN.md §9.2), scoped to this source's component
        // versions.
        if !source.coverage.is_empty() {
            let mut data = CoverageData::new();
            for file in &source.coverage {
                data.load(&playbook.resolve_path(file))?;
            }
            coverage_scopes.push(CoverageScope {
                data,
                prefix: source.effective_coverage_prefix(),
                components: contributed,
            });
        }
    }
    if catalog.components().is_empty() {
        bail!("no components found in the playbook's content sources");
    }

    let pipeline =
        Pipeline::new(catalog, playbook.asciidoc.attribute_seeds()).with_coverage(coverage_scopes);
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
            coverage: page.coverage.as_ref().map(coverage_view),
        })?;
        files.push((page.url.clone(), html.into_bytes()));
    }

    // The site-wide coverage dashboard, when any page carries coverage.
    let covered: Vec<&RenderedPage> = site.pages.iter().filter(|p| p.coverage.is_some()).collect();
    if covered.iter().any(|p| p.url == COVERAGE_DASHBOARD_URL)
        || site.pages.iter().any(|p| p.url == COVERAGE_DASHBOARD_URL)
    {
        // An authored page owns the dashboard URL (a versionless ROOT
        // component can publish `coverage.adoc` there); never overwrite
        // authored content with generated output.
        eprintln!(
            "warning: skipping the generated coverage dashboard: an authored page \
             already publishes at {COVERAGE_DASHBOARD_URL}"
        );
    } else if !covered.is_empty() {
        let contents = coverage_dashboard(&covered);
        let html = theme.compose_page(&PageContext {
            site_title: &playbook.site.title,
            url: COVERAGE_DASHBOARD_URL,
            title_html: Some("Spec Coverage"),
            title_text: Some("Spec Coverage"),
            contents: &contents,
            nav: &empty_nav,
            home_url: "index.html",
            versions: &[],
            coverage: None,
        })?;
        files.push((COVERAGE_DASHBOARD_URL.to_string(), html.into_bytes()));
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
        for file in &source.coverage {
            watch.push(playbook.resolve_path(file));
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

/// Builds one page's coverage presentation: the rollup numbers plus the
/// client overlay payload (the block-pairing selector and per-block
/// status tokens; `<` is escaped so the JSON embeds safely in a
/// `<script>` element).
fn coverage_view(coverage: &PageCoverage) -> CoverageView {
    let blocks: Vec<Option<&str>> = coverage
        .blocks
        .iter()
        .map(|b| b.map(BlockStatus::css_token))
        .collect();
    let data_json = serde_json::json!({
        "selector": bokfell_render::coverage_client_selector(),
        "blocks": blocks,
    })
    .to_string()
    .replace('<', "\\u003c");

    CoverageView {
        percent: coverage.percent_verified(),
        verified: coverage.verified,
        uncovered: coverage.uncovered,
        data_json,
        dashboard_url: COVERAGE_DASHBOARD_URL.to_string(),
    }
}

/// Builds the dashboard page body: every covered page's rollup in a
/// table, least-verified first, with a site-wide total.
fn coverage_dashboard(covered: &[&RenderedPage]) -> String {
    let mut rows: Vec<&&RenderedPage> = covered.iter().collect();
    rows.sort_by(|a, b| {
        let (ca, cb) = (a.coverage.as_ref().unwrap(), b.coverage.as_ref().unwrap());
        ca.percent_verified()
            .cmp(&cb.percent_verified())
            .then_with(|| a.url.cmp(&b.url))
    });

    let mut out = String::from(
        "<div class=\"coverage-dashboard\">\n\
         <p>Verified means a normative line is reproduced and exercised by \
         a test; uncovered means it is normative but not yet verified.</p>\n\
         <table>\n<thead><tr><th>Page</th><th>Verified</th>\
         <th>Uncovered</th><th>Coverage</th></tr></thead>\n<tbody>\n",
    );
    let (mut total_verified, mut total_uncovered) = (0usize, 0usize);
    for page in rows {
        let coverage = page.coverage.as_ref().unwrap();
        total_verified += coverage.verified;
        total_uncovered += coverage.uncovered;
        let percent = coverage.percent_verified();
        let label = page.title_text.as_deref().unwrap_or(&page.url);
        out.push_str(&format!(
            "<tr><td><a href=\"{href}\">{label}</a> \
             <span class=\"page-url\">{url}</span></td>\
             <td class=\"num\">{verified}</td>\
             <td class=\"num\">{uncovered}</td>\
             <td class=\"num\"><span class=\"cov-{level}\">{percent}%</span></td></tr>\n",
            href = escape_html(&relative_url(COVERAGE_DASHBOARD_URL, &page.url)),
            label = escape_html(label),
            url = escape_html(&page.url),
            verified = coverage.verified,
            uncovered = coverage.uncovered,
            level = coverage_level(percent),
        ));
    }

    let total = total_verified + total_uncovered;
    let total_percent = (total_verified * 100)
        .checked_div(total)
        .map_or(100, |percent| percent as u32);
    out.push_str(&format!(
        "</tbody>\n<tfoot><tr><td>All covered pages</td>\
         <td class=\"num\">{total_verified}</td>\
         <td class=\"num\">{total_uncovered}</td>\
         <td class=\"num\"><span class=\"cov-{level}\">{total_percent}%</span></td></tr></tfoot>\n\
         </table>\n</div>\n",
        level = coverage_level(total_percent),
    ));
    out
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
