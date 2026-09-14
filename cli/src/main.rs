//! The Bokfell command line.
//!
//! `bokfell build` runs the pipeline: playbook → content catalog → render
//! → theme composition → static site. `bokfell serve` runs the same
//! pipeline into memory behind a watching dev server with live reload
//! (M2). `bokfell coverage scan|report|lint` drives the spec-coverage
//! engine (RFC 0001 §7) over the same playbook; `build` and `serve`
//! consume its database directly when the playbook configures a scan.

mod coverage;

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context};
use bokfell_aggregate::{Aggregator, GitSource};
use bokfell_coverage::{CoverageData, CoverageDatabase, CoverageScope, PageCoverage, StateCounts};
use bokfell_model::{relative_url, ContentCatalog, Coords, Family, NavTree, Playbook, ResourceRef};
use bokfell_render::{DiffBase, PageBlockChange, PageDiff, Pipeline, RenderedPage};
use bokfell_theme::{
    coverage_level, escape_html, CoverageView, DiffView, PageContext, Theme, VersionLink,
};
use clap::{Parser as ClapParser, Subcommand, ValueEnum};

use crate::coverage::LoadedRoot;

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

        /// Diff every page against this git ref of its source (PR
        /// preview mode) instead of against the previous component
        /// version.
        #[arg(long, value_name = "REF")]
        diff_base: Option<String>,
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

        /// Diff every page against this git ref of its source (PR
        /// preview mode) instead of against the previous component
        /// version.
        #[arg(long, value_name = "REF")]
        diff_base: Option<String>,

        /// Editor command for click-to-source editing; `{file}` and
        /// `{line}` are substituted (e.g. "code --goto {file}:{line}").
        /// Defaults to $BOKFELL_EDITOR, then `code` when available.
        #[arg(long, value_name = "TEMPLATE")]
        editor: Option<String>,
    },

    /// Spec coverage: scan claims and coverage maps, report, lint.
    Coverage {
        #[command(subcommand)]
        action: CoverageCommand,
    },
}

#[derive(Subcommand)]
enum CoverageCommand {
    /// Aggregate, parse the measured pages, scan the test roots and
    /// coverage maps, resolve, and write the coverage database.
    Scan {
        /// Path to the playbook file.
        #[arg(short, long, default_value = "bokfell.yml")]
        playbook: PathBuf,

        /// Refresh cached remote repositories first.
        #[arg(long)]
        fetch: bool,

        /// Where to write the database (overrides `coverage.database`).
        #[arg(short, long)]
        out: Option<PathBuf>,
    },

    /// Print the coverage rollups from the database (scanning first when
    /// there is none yet).
    Report {
        /// Path to the playbook file.
        #[arg(short, long, default_value = "bokfell.yml")]
        playbook: PathBuf,

        /// The output format.
        #[arg(long, value_enum, default_value_t = ReportFormat::Table)]
        format: ReportFormat,

        /// The database to read (overrides `coverage.database`).
        #[arg(long)]
        database: Option<PathBuf>,
    },

    /// Print review findings: unresolved targets, unclassified blocks,
    /// stale planned entries, heuristic disagreements.
    Lint {
        /// Path to the playbook file.
        #[arg(short, long, default_value = "bokfell.yml")]
        playbook: PathBuf,

        /// The database to read (overrides `coverage.database`).
        #[arg(long)]
        database: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ReportFormat {
    /// A plain-text table per page and component.
    Table,
    /// The coverage database as JSON.
    Json,
    /// Codecov-style per-line JSON (block states projected onto lines).
    Codecov,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Build {
            playbook,
            out,
            theme,
            fetch,
            diff_base,
        } => build(
            &playbook,
            out.as_deref(),
            theme.as_deref(),
            fetch,
            diff_base.as_deref(),
        ),
        Command::Serve {
            playbook,
            theme,
            port,
            fetch,
            diff_base,
            editor,
        } => serve(
            &playbook,
            theme.as_deref(),
            port,
            fetch,
            diff_base.as_deref(),
            editor.as_deref(),
        ),
        Command::Coverage { action } => match action {
            CoverageCommand::Scan {
                playbook,
                fetch,
                out,
            } => coverage_scan(&playbook, fetch, out.as_deref()),
            CoverageCommand::Report {
                playbook,
                format,
                database,
            } => coverage_report(&playbook, format, database.as_deref()),
            CoverageCommand::Lint { playbook, database } => {
                coverage_lint(&playbook, database.as_deref())
            }
        },
    }
}

/// One composed site output: `(site-root-relative URL, bytes)`.
type SiteFiles = Vec<(String, Vec<u8>)>;

/// The site-root-relative URL of the coverage dashboard page.
const COVERAGE_DASHBOARD_URL: &str = "coverage.html";

/// The site-root-relative URL of the what-changed index page.
const CHANGES_INDEX_URL: &str = "whats-changed.html";

/// The playbook's content, aggregated and cataloged.
struct LoadedSite {
    playbook: Playbook,
    aggregator: Aggregator,
    catalog: ContentCatalog,
    /// Every content root, with what the coverage scan needs to place its
    /// pages in a repository.
    roots: Vec<LoadedRoot>,
    /// Pre-computed per-line coverage (the interim input), per source.
    coverage_scopes: Vec<CoverageScope>,
}

/// Runs playbook → aggregate → catalog.
fn load_site(playbook_path: &Path, fetch: bool) -> anyhow::Result<LoadedSite> {
    let playbook = Playbook::load(playbook_path)?;

    let cache_dir = playbook
        .runtime
        .cache_dir
        .as_deref()
        .map(|dir| playbook.resolve_path(dir))
        .unwrap_or_else(Aggregator::default_cache_dir);
    let aggregator = Aggregator::new(cache_dir, fetch || playbook.runtime.fetch);

    let mut catalog = ContentCatalog::new();
    let mut roots: Vec<LoadedRoot> = Vec::new();
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
            contributed.push(key.clone());
            roots.push(LoadedRoot {
                component: key,
                root,
                git: None,
            });
        } else {
            // A git source: aggregate each matched ref into a content
            // root.
            let url =
                coverage::resolve_repo_url(&playbook, source.url.as_deref().expect("validated"));
            let git_source = GitSource {
                url: url.clone(),
                branches: source.branches.clone(),
                tags: source.tags.clone(),
                start_path: source.start_path.clone(),
                version_from_ref: source.version_from_ref,
            };
            let collected = aggregator.collect(&git_source)?;
            for root in collected {
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
                contributed.push(key.clone());
                roots.push(LoadedRoot {
                    component: key,
                    root: root.path,
                    git: Some((url.clone(), source.start_path.clone())),
                });
            }
        }

        // Pre-computed spec coverage (PLAN.md §9.2), scoped to this
        // source's component versions.
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

    Ok(LoadedSite {
        playbook,
        aggregator,
        catalog,
        roots,
        coverage_scopes,
    })
}

/// How to compose the site.
#[derive(Clone, Debug, Default)]
struct ComposeOptions {
    theme_dir: Option<PathBuf>,
    fetch: bool,
    diff_base_ref: Option<String>,
    /// Serve mode: emit edit targets and collect the editable allowlist.
    edit: bool,
    /// Whether coverage resolution errors abort the build (`build`)
    /// rather than print as warnings (`serve`).
    coverage_errors_fatal: bool,
}

/// A composed site plus the source files its pages trace back to (the
/// edit API's allowlist; empty unless `edit` was requested).
type ComposedSite = (SiteFiles, std::collections::HashSet<PathBuf>);

/// Runs playbook → catalog → render → theme and returns every site file.
///
/// Warnings are printed to stderr as they surface.
fn compose_site(playbook_path: &Path, options: &ComposeOptions) -> anyhow::Result<ComposedSite> {
    let site = load_site(playbook_path, options.fetch)?;
    let playbook = &site.playbook;

    // Page diffing (PLAN.md §9.1): PR-preview mode against a named ref,
    // or (by default) each page against its previous component version.
    let diff_base = match options.diff_base_ref.as_deref() {
        Some(reference) => {
            let base = base_catalog(playbook, &site.aggregator, reference)?;
            if base.components().is_empty() {
                bail!("--diff-base {reference}: no content source could be aggregated at that ref");
            }
            DiffBase::Catalog {
                pipeline: Box::new(Pipeline::new(base, playbook.asciidoc.attribute_seeds())),
                label: reference.to_string(),
            }
        }
        None => DiffBase::PreviousVersion,
    };

    let mut pipeline = Pipeline::new(site.catalog, playbook.asciidoc.attribute_seeds())
        .with_coverage(site.coverage_scopes)
        .with_diff(diff_base);

    // Spec coverage (RFC 0001): scan the configured repositories and hand
    // the resolved database straight to the pipeline.
    let mut database: Option<Arc<CoverageDatabase>> = None;
    if !playbook.coverage.scan.is_empty() {
        let db = coverage::scan(playbook, &site.aggregator, &pipeline, &site.roots)?;
        if !db.errors.is_empty() {
            for error in &db.errors {
                eprintln!("error[coverage/{}]: {error}", error.kind.token());
            }
            if options.coverage_errors_fatal {
                bail!(
                    "{} spec-coverage error(s); fix the claims or coverage maps above (or \
                     run `bokfell coverage lint`)",
                    db.errors.len()
                );
            }
        }
        let db = Arc::new(db);
        pipeline = pipeline.with_coverage_database(db.clone());
        database = Some(db);
    }

    let rendered = pipeline.render_site()?;
    let theme = Theme::load(options.theme_dir.as_deref())?;

    let navs: std::collections::HashMap<(&str, Option<&str>), &NavTree> = rendered
        .navs
        .iter()
        .map(|((name, version), tree)| ((name.as_str(), version.as_deref()), tree))
        .collect();
    let empty_nav = NavTree::default();
    let start_url = start_page_url(playbook, pipeline.catalog(), &rendered.pages)?;
    let link_template = playbook.coverage.link_template.as_deref();

    let mut files: SiteFiles = Vec::new();

    for page in &rendered.pages {
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
            diff: diff_view(page),
            overlay_json: overlay_json(page, database.as_deref(), link_template, options.edit),
        })?;
        files.push((page.url.clone(), html.into_bytes()));
    }

    // The what-changed index, when any page changed against its base.
    let changed: Vec<&RenderedPage> = rendered
        .pages
        .iter()
        .filter(|p| p.diff.as_ref().is_some_and(PageDiff::is_changed))
        .collect();
    if !changed.is_empty() {
        if rendered.pages.iter().any(|p| p.url == CHANGES_INDEX_URL) {
            eprintln!(
                "warning: skipping the generated what-changed index: an authored page \
                 already publishes at {CHANGES_INDEX_URL}"
            );
        } else {
            let contents = changes_index(&changed);
            let html = theme.compose_page(&PageContext {
                site_title: &playbook.site.title,
                url: CHANGES_INDEX_URL,
                title_html: Some("What Changed"),
                title_text: Some("What Changed"),
                contents: &contents,
                nav: &empty_nav,
                home_url: "index.html",
                versions: &[],
                coverage: None,
                diff: None,
                overlay_json: None,
            })?;
            files.push((CHANGES_INDEX_URL.to_string(), html.into_bytes()));
        }
    }

    // The site-wide coverage dashboard, when any page carries coverage.
    let covered: Vec<&RenderedPage> = rendered
        .pages
        .iter()
        .filter(|p| p.coverage.is_some())
        .collect();
    if rendered
        .pages
        .iter()
        .any(|p| p.url == COVERAGE_DASHBOARD_URL)
    {
        // An authored page owns the dashboard URL (a versionless ROOT
        // component can publish `coverage.adoc` there); never overwrite
        // authored content with generated output.
        eprintln!(
            "warning: skipping the generated coverage dashboard: an authored page \
             already publishes at {COVERAGE_DASHBOARD_URL}"
        );
    } else if !covered.is_empty() {
        let contents = coverage_dashboard(&covered, pipeline.catalog());
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
            diff: None,
            overlay_json: None,
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

    // The static search index (PLAN.md §12): a lead entry per page plus
    // one per anchored section, so results land on the section itself.
    let entries: Vec<bokfell_search::SearchEntry> = rendered
        .pages
        .iter()
        .flat_map(|page| {
            bokfell_search::page_entries(
                page.title_text.as_deref().unwrap_or(&page.url),
                &page.url,
                &page.contents,
            )
        })
        .collect();
    files.push((
        "_/search-index.json".to_string(),
        bokfell_search::index_json(&entries).into_bytes(),
    ));

    // Theme assets and the root redirect.
    files.extend(theme.assets());
    files.push((
        "index.html".to_string(),
        Theme::redirect_page(&start_url).into_bytes(),
    ));

    // The edit API's allowlist: every source file a rendered block
    // traces back to, plus every locally sourced test file a claim
    // points at (RFC 0001 §8's locality rule).
    let mut editable = std::collections::HashSet::new();
    if options.edit {
        for page in &rendered.pages {
            for source in page.block_sources.iter().flatten() {
                editable.insert(source.0.clone());
            }
        }
        if let Some(db) = &database {
            for claim in &db.claims {
                if let Some(path) = &claim.site.local_path {
                    editable.insert(path.clone());
                }
            }
        }
    }

    Ok((files, editable))
}

fn build(
    playbook_path: &Path,
    out: Option<&Path>,
    theme_dir: Option<&Path>,
    fetch: bool,
    diff_base: Option<&str>,
) -> anyhow::Result<()> {
    let playbook = Playbook::load(playbook_path)?;
    let out_dir = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| playbook.output_dir());

    let (files, _) = compose_site(
        playbook_path,
        &ComposeOptions {
            theme_dir: theme_dir.map(Path::to_path_buf),
            fetch,
            diff_base_ref: diff_base.map(str::to_string),
            edit: false,
            coverage_errors_fatal: true,
        },
    )?;
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
    diff_base: Option<&str>,
    editor: Option<&str>,
) -> anyhow::Result<()> {
    // Watch the playbook, every *local directory* content source, the
    // local test roots and spec maps of the coverage scan, and the theme
    // directory (git sources are cache-backed and not watched).
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
    for scan in &playbook.coverage.scan {
        if let Some(path) = &scan.path {
            let dir = playbook.resolve_path(path);
            for tests in &scan.tests {
                watch.push(dir.join(tests));
            }
            if let Some(spec_map) = &scan.spec_map {
                watch.push(dir.join(spec_map));
            }
        }
    }
    if let Some(dir) = theme_dir {
        watch.push(dir.to_path_buf());
    }

    let playbook_path = playbook_path.to_path_buf();
    let mut options = ComposeOptions {
        theme_dir: theme_dir.map(Path::to_path_buf),
        fetch,
        diff_base_ref: diff_base.map(str::to_string),
        edit: true,
        coverage_errors_fatal: false,
    };
    let builder: bokfell_serve::SiteBuilder = Box::new(move || {
        let options_now = options.clone();
        options.fetch = false;
        compose_site(&playbook_path, &options_now)
            .map(|(files, editable)| bokfell_serve::SiteBuild {
                files: files.into_iter().collect(),
                editable,
            })
            .map_err(|e| format!("{e:#}"))
    });

    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    bokfell_serve::serve(
        builder,
        bokfell_serve::ServeOptions {
            addr,
            watch,
            editor: editor_command(editor),
            ..Default::default()
        },
    )?;
    Ok(())
}

/// `bokfell coverage scan`: writes the database and lists every
/// resolution error with provenance; exits non-zero when there are any.
fn coverage_scan(playbook_path: &Path, fetch: bool, out: Option<&Path>) -> anyhow::Result<()> {
    let (db, path) = run_scan(playbook_path, fetch, out)?;
    let (components, total) = db.rollups();
    println!(
        "Scanned {} page(s) in {} component version(s), {} claim(s): {}% verified \
         ({} verified, {} planned, {} uncovered, {} unclassified; {} out of scope, {} \
         non-normative)",
        db.pages.len(),
        components.len(),
        db.claims.len(),
        total.percent_verified(),
        total.verified,
        total.planned,
        total.uncovered,
        total.unclassified,
        total.out_of_scope,
        total.non_normative,
    );
    println!("Coverage database written to {}", path.display());
    if !db.errors.is_empty() {
        for error in &db.errors {
            eprintln!("error[coverage/{}]: {error}", error.kind.token());
        }
        bail!("{} spec-coverage error(s)", db.errors.len());
    }
    Ok(())
}

/// Runs the scan and writes the database, returning both.
fn run_scan(
    playbook_path: &Path,
    fetch: bool,
    out: Option<&Path>,
) -> anyhow::Result<(CoverageDatabase, PathBuf)> {
    let site = load_site(playbook_path, fetch)?;
    if site.playbook.coverage.scan.is_empty() {
        bail!(
            "{}: no `coverage.scan` entries — nothing to scan",
            playbook_path.display()
        );
    }
    let pipeline = Pipeline::new(site.catalog, site.playbook.asciidoc.attribute_seeds());
    let db = coverage::scan(&site.playbook, &site.aggregator, &pipeline, &site.roots)?;

    let path = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| site.playbook.coverage_database());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, db.to_json()).with_context(|| format!("writing {}", path.display()))?;
    Ok((db, path))
}

/// Reads the database at `--database` (or the playbook's location),
/// scanning first when there is none yet.
fn load_database(
    playbook_path: &Path,
    database: Option<&Path>,
) -> anyhow::Result<CoverageDatabase> {
    let playbook = Playbook::load(playbook_path)?;
    let path = database
        .map(Path::to_path_buf)
        .unwrap_or_else(|| playbook.coverage_database());
    if path.is_file() {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        return CoverageDatabase::from_json(&text)
            .map_err(|e| anyhow::anyhow!("{}: not a coverage database: {e}", path.display()));
    }
    eprintln!(
        "note: no coverage database at {}; scanning first",
        path.display()
    );
    run_scan(playbook_path, false, Some(&path)).map(|(db, _)| db)
}

fn coverage_report(
    playbook_path: &Path,
    format: ReportFormat,
    database: Option<&Path>,
) -> anyhow::Result<()> {
    let db = load_database(playbook_path, database)?;
    let out = match format {
        ReportFormat::Table => bokfell_coverage::report::table(&db),
        ReportFormat::Json => db.to_json(),
        ReportFormat::Codecov => bokfell_coverage::report::codecov_json(&db),
    };
    print!("{out}");
    if !out.ends_with('\n') {
        println!();
    }
    Ok(())
}

fn coverage_lint(playbook_path: &Path, database: Option<&Path>) -> anyhow::Result<()> {
    let db = load_database(playbook_path, database)?;
    let out = bokfell_coverage::report::lint(&db);
    if out.is_empty() {
        println!("No findings: every target resolves and every block is classified.");
    } else {
        print!("{out}");
    }
    if !db.errors.is_empty() {
        bail!("{} spec-coverage error(s)", db.errors.len());
    }
    Ok(())
}

/// Resolves the editor for click-to-source editing: the `--editor` flag,
/// then `$BOKFELL_EDITOR`, then `code --goto` when VS Code's CLI is on
/// the PATH. `None` leaves the edit endpoint answering with guidance.
fn editor_command(flag: Option<&str>) -> Option<bokfell_serve::EditorCommand> {
    if let Some(template) = flag {
        return bokfell_serve::EditorCommand::parse(template);
    }
    if let Ok(template) = std::env::var("BOKFELL_EDITOR") {
        return bokfell_serve::EditorCommand::parse(&template);
    }

    // `code --goto file:line` is the one broadly-installed GUI editor
    // CLI; terminal editors need a TTY the server doesn't have.
    let code_available = std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join("code");
            candidate.is_file() || (cfg!(windows) && dir.join("code.cmd").is_file())
        })
    });
    if code_available {
        return bokfell_serve::EditorCommand::parse("code --goto {file}:{line}");
    }
    None
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

/// Builds one page's coverage presentation (the rollup numbers for the
/// badge).
fn coverage_view(coverage: &PageCoverage) -> CoverageView {
    CoverageView {
        percent: coverage.percent_verified(),
        verified: coverage.counts.verified,
        planned: coverage.counts.planned,
        uncovered: coverage.counts.uncovered,
        unclassified: coverage.counts.unclassified,
        out_of_scope: coverage.counts.out_of_scope,
        non_normative: coverage.counts.non_normative,
        dashboard_url: COVERAGE_DASHBOARD_URL.to_string(),
    }
}

/// Builds one page's diff presentation, when the page changed against
/// its base.
fn diff_view(page: &RenderedPage) -> Option<DiffView> {
    let diff = page.diff.as_ref().filter(|d| d.is_changed())?;
    Some(DiffView {
        base_label: diff.base_label.clone(),
        new_page: diff.new_page,
        added: diff.added,
        removed: diff.removed,
        edited: diff.edited,
        changes_url: CHANGES_INDEX_URL.to_string(),
    })
}

/// Builds the combined client overlay payload: the block-pairing
/// selector plus per-block arrays for the overlays this page carries —
/// coverage entries (`{s: state, r: reason, t: [tracking, url], c:
/// [claims]}`, each claim `{l: label, f: "file:line", n: line, t:
/// test index, u: url, e: [file, line]}` with `e` only in serve mode for
/// local test files) and a `tests` table holding each distinct
/// enclosing test function once (`{f: file, l: first line, n: name, h:
/// [highlighted line html], u: url, e: [file, line]}`), diff changes
/// (`null` unchanged, `"added"`, or `["edited", word_diff_html]`), and
/// (in serve mode) edit targets (`[file, line]` per block). `<` is
/// escaped so the JSON embeds safely in a `<script>` element.
fn overlay_json(
    page: &RenderedPage,
    database: Option<&CoverageDatabase>,
    link_template: Option<&str>,
    edit: bool,
) -> Option<String> {
    // The test functions this page's claims sit in, deduplicated by
    // file and start line, highlighted once each.
    let mut tests: Vec<serde_json::Value> = Vec::new();
    let mut test_index: std::collections::HashMap<(String, u32), usize> =
        std::collections::HashMap::new();
    let mut test_entry = |claim: &bokfell_coverage::Claim| -> Option<usize> {
        let (fn_line, source) = (claim.site.fn_line?, claim.site.fn_source.as_deref()?);
        let key = (claim.site.file.clone(), fn_line);
        if let Some(&index) = test_index.get(&key) {
            return Some(index);
        }
        let mut t = serde_json::Map::new();
        t.insert("f".into(), serde_json::json!(claim.site.file));
        t.insert("l".into(), serde_json::json!(fn_line));
        t.insert("n".into(), serde_json::json!(claim.site.test_fn));
        t.insert(
            "h".into(),
            serde_json::json!(bokfell_theme::highlight_rust_lines(source)),
        );
        if let Some(url) = coverage::claim_url_at(claim, fn_line, link_template) {
            t.insert("u".into(), serde_json::json!(url));
        }
        if let (true, Some(path)) = (edit, &claim.site.local_path) {
            t.insert(
                "e".into(),
                serde_json::json!([path.display().to_string(), fn_line]),
            );
        }
        let index = tests.len();
        tests.push(serde_json::Value::Object(t));
        test_index.insert(key, index);
        Some(index)
    };

    let coverage = page.coverage.as_ref().map(|cov| {
        serde_json::json!(cov
            .blocks
            .iter()
            .map(|block| {
                let mut entry = serde_json::Map::new();
                entry.insert("s".into(), serde_json::json!(block.state.token()));
                if let Some(reason) = &block.reason {
                    entry.insert("r".into(), serde_json::json!(reason));
                }
                if let Some(tracking) = &block.tracking {
                    entry.insert("t".into(), serde_json::json!([tracking.raw, tracking.url]));
                }
                if let Some(db) = database {
                    let claims: Vec<serde_json::Value> = block
                        .claims
                        .iter()
                        .filter_map(|&index| db.claims.get(index))
                        .map(|claim| {
                            let mut c = serde_json::Map::new();
                            c.insert("l".into(), serde_json::json!(claim.label()));
                            c.insert(
                                "f".into(),
                                serde_json::json!(format!(
                                    "{}:{}",
                                    claim.site.file, claim.site.line
                                )),
                            );
                            c.insert("n".into(), serde_json::json!(claim.site.line));
                            if let Some(index) = test_entry(claim) {
                                c.insert("t".into(), serde_json::json!(index));
                            }
                            if let Some(url) = coverage::claim_url(claim, link_template) {
                                c.insert("u".into(), serde_json::json!(url));
                            }
                            if let (true, Some(path)) = (edit, &claim.site.local_path) {
                                c.insert(
                                    "e".into(),
                                    serde_json::json!([
                                        path.display().to_string(),
                                        claim.site.line
                                    ]),
                                );
                            }
                            serde_json::Value::Object(c)
                        })
                        .collect();
                    if !claims.is_empty() {
                        entry.insert("c".into(), serde_json::Value::Array(claims));
                    }
                }
                serde_json::Value::Object(entry)
            })
            .collect::<Vec<_>>())
    });
    let diff = page.diff.as_ref().filter(|d| d.is_changed()).map(|d| {
        serde_json::json!(d
            .blocks
            .iter()
            .map(|change| match change {
                PageBlockChange::Unchanged => serde_json::Value::Null,
                PageBlockChange::Added => serde_json::json!("added"),
                PageBlockChange::Edited { diff_html } => serde_json::json!(["edited", diff_html]),
            })
            .collect::<Vec<_>>())
    });
    let edit_targets = edit
        .then(|| {
            page.block_sources
                .iter()
                .map(|source| match source {
                    Some((file, line)) => {
                        serde_json::json!([file.display().to_string(), line])
                    }
                    None => serde_json::Value::Null,
                })
                .collect::<Vec<_>>()
        })
        .filter(|targets| targets.iter().any(|t| !t.is_null()))
        .map(serde_json::Value::Array);

    if coverage.is_none() && diff.is_none() && edit_targets.is_none() {
        return None;
    }

    let mut payload = serde_json::Map::new();
    payload.insert(
        "selector".to_string(),
        serde_json::json!(bokfell_render::overlay_client_selector()),
    );
    // Exact anchors: each overlay block's preprocessed start line, the
    // value its rendered container carries as `data-source-line`
    // (asciidoc-html5 0.2.2, issue #339). The selector walk above stays
    // as the fallback for content without the annotations.
    payload.insert("lines".to_string(), serde_json::json!(page.block_lines));
    if let Some(coverage) = coverage {
        payload.insert("coverage".to_string(), coverage);
    }
    if !tests.is_empty() {
        payload.insert("tests".to_string(), serde_json::Value::Array(tests));
    }
    if let Some(diff) = diff {
        payload.insert("diff".to_string(), diff);
    }
    if let Some(edit_targets) = edit_targets {
        payload.insert("edit".to_string(), edit_targets);
    }
    Some(
        serde_json::Value::Object(payload)
            .to_string()
            .replace('<', "\\u003c"),
    )
}

/// Builds the what-changed index page body: every changed page with its
/// base and change counts.
fn changes_index(changed: &[&RenderedPage]) -> String {
    let mut out = String::from(
        "<div class=\"changes-index\">\n\
         <p>Pages that differ from their comparison base (the previous \
         component version, or the base ref in PR-preview mode).</p>\n\
         <table>\n<thead><tr><th>Page</th><th>Compared to</th>\
         <th>Added</th><th>Edited</th><th>Removed</th></tr></thead>\n<tbody>\n",
    );
    for page in changed {
        let diff = page.diff.as_ref().expect("filtered to changed pages");
        let label = page.title_text.as_deref().unwrap_or(&page.url);
        let counts = if diff.new_page {
            "<td colspan=\"3\"><span class=\"new-page\">new page</span></td>".to_string()
        } else {
            format!(
                "<td class=\"num\">{}</td><td class=\"num\">{}</td><td class=\"num\">{}</td>",
                diff.added, diff.edited, diff.removed
            )
        };
        out.push_str(&format!(
            "<tr><td><a href=\"{href}\">{label}</a> \
             <span class=\"page-url\">{url}</span></td>\
             <td>{base}</td>{counts}</tr>\n",
            href = escape_html(&relative_url(CHANGES_INDEX_URL, &page.url)),
            label = escape_html(label),
            url = escape_html(&page.url),
            base = escape_html(&diff.base_label),
        ));
    }
    out.push_str("</tbody>\n</table>\n</div>\n");
    out
}

/// Builds the base catalog for PR-preview mode: every content source
/// re-aggregated at `base_ref`. Git sources aggregate that ref directly;
/// a directory source is looked up in its enclosing git repository (and
/// skipped with a warning when it has none).
fn base_catalog(
    playbook: &Playbook,
    aggregator: &bokfell_aggregate::Aggregator,
    base_ref: &str,
) -> anyhow::Result<ContentCatalog> {
    let mut catalog = ContentCatalog::new();
    for source in &playbook.content.sources {
        let git_source = if let Some(path) = &source.path {
            let root = playbook.resolve_path(path);
            let Some((repo_root, start_path)) = enclosing_repo(&root) else {
                eprintln!(
                    "warning: --diff-base: skipping content source {} \
                     (not inside a git repository)",
                    root.display()
                );
                continue;
            };
            GitSource {
                url: repo_root.display().to_string(),
                branches: Vec::new(),
                tags: Vec::new(),
                start_path,
                version_from_ref: false,
            }
        } else {
            let url =
                coverage::resolve_repo_url(playbook, source.url.as_deref().expect("validated"));
            GitSource {
                url,
                branches: Vec::new(),
                tags: Vec::new(),
                start_path: source.start_path.clone(),
                version_from_ref: false,
            }
        };

        // `collect_ref` tries the ref as a branch, then as a tag — never
        // both, so a same-named branch and tag can't scan twice.
        for root in aggregator.collect_ref(&git_source, base_ref)? {
            catalog
                .scan_source_versioned(&root.path, root.version_override.as_deref())
                .with_context(|| {
                    format!("scanning diff base {base_ref} ({})", root.path.display())
                })?;
        }
    }
    Ok(catalog)
}

/// Finds the git repository containing `path`: the repository root and
/// `path` relative to it (as a start path).
fn enclosing_repo(path: &Path) -> Option<(PathBuf, String)> {
    let canonical = path.canonicalize().ok()?;
    let mut dir = canonical.as_path();
    loop {
        if dir.join(".git").exists() {
            let start_path = canonical
                .strip_prefix(dir)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            return Some((dir.to_path_buf(), start_path));
        }
        dir = dir.parent()?;
    }
}

/// Builds the dashboard page body (RFC 0001 §8): per-page and
/// per-component stacked bars over the five states, out-of-scope beside
/// the bar, unclassified called out as the review queue.
fn coverage_dashboard(covered: &[&RenderedPage], catalog: &ContentCatalog) -> String {
    let mut out = String::from(
        "<div class=\"coverage-dashboard\">\n\
         <p>Each block of a measured page is <span class=\"cov-verified\">verified</span> \
         (a test claims it), <span class=\"cov-planned\">planned</span> (a tracked gap), \
         <span class=\"cov-uncovered\">uncovered</span> (normative, unclaimed), or \
         <span class=\"cov-unclassified\">unclassified</span> (nobody has decided yet — \
         the review queue). Out-of-scope and non-normative blocks leave the denominator; \
         out-of-scope counts stay visible beside every bar.</p>\n\
         <p class=\"cov-legend\"><span class=\"verified\">verified</span>\
         <span class=\"planned\">planned</span><span class=\"uncovered\">uncovered</span>\
         <span class=\"unclassified\">unclassified</span></p>\n\
         <table>\n<thead><tr><th>Page</th><th>Coverage</th><th>Verified</th>\
         <th>Planned</th><th>Uncovered</th><th>Unclassified</th><th>Out of scope</th>\
         <th>Non-normative</th></tr></thead>\n<tbody>\n",
    );

    // Group by component version in catalog order; pages least-verified
    // first within a component.
    let mut total = StateCounts::default();
    for component in catalog.components() {
        let mut pages: Vec<&&RenderedPage> = covered
            .iter()
            .filter(|p| {
                p.coords.component == component.desc.name
                    && p.coords.version == component.desc.version
            })
            .collect();
        if pages.is_empty() {
            continue;
        }
        pages.sort_by(|a, b| {
            let (ca, cb) = (a.coverage.as_ref().unwrap(), b.coverage.as_ref().unwrap());
            ca.percent_verified()
                .cmp(&cb.percent_verified())
                .then_with(|| a.url.cmp(&b.url))
        });

        let mut subtotal = StateCounts::default();
        for page in &pages {
            subtotal.add_counts(&page.coverage.as_ref().unwrap().counts);
        }
        total.add_counts(&subtotal);
        let label = format!(
            "{}{}",
            component.desc.title(),
            component
                .desc
                .display_version
                .clone()
                .or_else(|| component.desc.version.clone())
                .map(|v| format!(" {v}"))
                .unwrap_or_default()
        );
        out.push_str(&format!(
            "<tr class=\"component\"><td>{label}</td>{cells}</tr>\n",
            label = escape_html(&label),
            cells = dashboard_cells(&subtotal),
        ));
        for page in pages {
            let counts = &page.coverage.as_ref().unwrap().counts;
            let label = page.title_text.as_deref().unwrap_or(&page.url);
            out.push_str(&format!(
                "<tr><td><a href=\"{href}\">{label}</a> \
                 <span class=\"page-url\">{url}</span></td>{cells}</tr>\n",
                href = escape_html(&relative_url(COVERAGE_DASHBOARD_URL, &page.url)),
                label = escape_html(label),
                url = escape_html(&page.url),
                cells = dashboard_cells(counts),
            ));
        }
    }

    out.push_str(&format!(
        "</tbody>\n<tfoot><tr><td>All measured pages</td>{cells}</tr></tfoot>\n\
         </table>\n</div>\n",
        cells = dashboard_cells(&total),
    ));
    out
}

/// The dashboard cells of one row: the stacked bar with the percentage,
/// then the five counts plus non-normative.
fn dashboard_cells(counts: &StateCounts) -> String {
    let denominator = counts.denominator();
    let width = |n: usize| -> String {
        (n * 100)
            .checked_div(denominator)
            .map_or_else(|| "0".to_string(), |w| w.to_string())
    };
    let percent = counts.percent_verified();
    let unclassified = if counts.unclassified > 0 {
        format!(
            "<span class=\"review-queue\">{}</span>",
            counts.unclassified
        )
    } else {
        "0".to_string()
    };
    format!(
        "<td class=\"state\"><span class=\"cov-{level}\">{percent}%</span> \
         <span class=\"cov-bar\" title=\"{v} verified, {p} planned, {u} uncovered, {n} unclassified of {d}\">\
         <span class=\"verified\" style=\"width:{wv}%\"></span>\
         <span class=\"planned\" style=\"width:{wp}%\"></span>\
         <span class=\"uncovered\" style=\"width:{wu}%\"></span>\
         <span class=\"unclassified\" style=\"width:{wn}%\"></span></span></td>\
         <td class=\"num\">{v}</td><td class=\"num\">{p}</td><td class=\"num\">{u}</td>\
         <td class=\"num\">{unclassified}</td><td class=\"num\">{o}</td><td class=\"num\">{nn}</td>",
        level = coverage_level(percent),
        v = counts.verified,
        p = counts.planned,
        u = counts.uncovered,
        n = counts.unclassified,
        d = denominator,
        o = counts.out_of_scope,
        nn = counts.non_normative,
        wv = width(counts.verified),
        wp = width(counts.planned),
        wu = width(counts.uncovered),
        wn = width(counts.unclassified),
    )
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
