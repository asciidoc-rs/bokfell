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
use bokfell_render::{DiffBase, PageBlockChange, PageDiff, Pipeline, RenderedPage};
use bokfell_theme::{
    coverage_level, escape_html, CoverageView, DiffView, PageContext, Theme, VersionLink,
};
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
    }
}

/// One composed site output: `(site-root-relative URL, bytes)`.
type SiteFiles = Vec<(String, Vec<u8>)>;

/// The site-root-relative URL of the coverage dashboard page.
const COVERAGE_DASHBOARD_URL: &str = "coverage.html";

/// The site-root-relative URL of the what-changed index page.
const CHANGES_INDEX_URL: &str = "whats-changed.html";

/// Runs playbook → catalog → render → theme and returns every site file.
///
/// Warnings are printed to stderr as they surface; the count is returned
/// alongside the files.
/// A composed site plus the source files its pages trace back to (the
/// edit API's allowlist; empty unless `edit` was requested).
type ComposedSite = (SiteFiles, std::collections::HashSet<PathBuf>);

fn compose_site(
    playbook_path: &Path,
    theme_dir: Option<&Path>,
    fetch: bool,
    diff_base_ref: Option<&str>,
    edit: bool,
) -> anyhow::Result<ComposedSite> {
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

    // Page diffing (PLAN.md §9.1): PR-preview mode against a named ref,
    // or (by default) each page against its previous component version.
    let diff_base = match diff_base_ref {
        Some(reference) => {
            let base = base_catalog(&playbook, &aggregator, reference)?;
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

    let pipeline = Pipeline::new(catalog, playbook.asciidoc.attribute_seeds())
        .with_coverage(coverage_scopes)
        .with_diff(diff_base);
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
            diff: diff_view(page),
            overlay_json: overlay_json(page, edit),
        })?;
        files.push((page.url.clone(), html.into_bytes()));
    }

    // The what-changed index, when any page changed against its base.
    let changed: Vec<&RenderedPage> = site
        .pages
        .iter()
        .filter(|p| p.diff.as_ref().is_some_and(PageDiff::is_changed))
        .collect();
    if !changed.is_empty() {
        if site.pages.iter().any(|p| p.url == CHANGES_INDEX_URL) {
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

    // Theme assets and the root redirect.
    files.extend(theme.assets());
    files.push((
        "index.html".to_string(),
        Theme::redirect_page(&start_url).into_bytes(),
    ));

    // The edit API's allowlist: every source file a rendered block
    // traces back to.
    let mut editable = std::collections::HashSet::new();
    if edit {
        for page in &site.pages {
            for source in page.block_sources.iter().flatten() {
                editable.insert(source.0.clone());
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

    let (files, _) = compose_site(playbook_path, theme_dir, fetch, diff_base, false)?;
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
    let diff_base = diff_base.map(str::to_string);
    let mut first = fetch;
    let builder: bokfell_serve::SiteBuilder = Box::new(move || {
        let fetch_now = std::mem::take(&mut first);
        compose_site(
            &playbook_path,
            theme_dir.as_deref(),
            fetch_now,
            diff_base.as_deref(),
            true,
        )
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
        verified: coverage.verified,
        uncovered: coverage.uncovered,
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
/// coverage status tokens, diff changes (`null` unchanged, `"added"`,
/// or `["edited", word_diff_html]`), and (in serve mode) edit targets
/// (`[file, line]` per block). `<` is escaped so the JSON embeds safely
/// in a `<script>` element.
fn overlay_json(page: &RenderedPage, edit: bool) -> Option<String> {
    let coverage = page.coverage.as_ref().map(|cov| {
        serde_json::json!(cov
            .blocks
            .iter()
            .map(|b| b.map(BlockStatus::css_token))
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
    if let Some(coverage) = coverage {
        payload.insert("coverage".to_string(), coverage);
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
            let url = source.url.clone().expect("validated: url set");
            let as_path = Path::new(&url);
            let url =
                if as_path.is_relative() && playbook.resolve_path(as_path).join(".git").exists() {
                    playbook.resolve_path(as_path).display().to_string()
                } else {
                    url
                };
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
