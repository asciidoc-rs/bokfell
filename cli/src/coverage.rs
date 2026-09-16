//! The spec-coverage scan (RFC 0001 §7): scoping the catalog's pages to
//! the playbook's `coverage.scan` repositories, collecting claims and
//! coverage maps from those repositories, resolving them into the
//! coverage database, and templating claim click-through links.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Context;
use bokfell_aggregate::{file_matches_head, local_repo_info, Aggregator, GitSource};
use bokfell_coverage::{
    load_spec_map, manifest_package_name, resolve, scan_test_root, Claim, CoverageDatabase,
    Sidecar, TestRoot,
};
use bokfell_model::{Playbook, ScanConfig, VirtualFile};
use bokfell_render::Pipeline;

/// One content root as loaded into the catalog, with what the coverage
/// scan needs to place its pages in a repository.
#[derive(Clone, Debug)]
pub struct LoadedRoot {
    /// The component version the root contributed.
    pub component: (String, Option<String>),
    /// The root directory that was scanned (holds `antora.yml`).
    pub root: PathBuf,
    /// For a git content source: its resolved URL (or local repository
    /// path) and start path within the repository.
    pub git: Option<(String, String)>,
}

/// Resolves a content source's `url` the way the site loader does: a
/// relative path naming a local repository resolves against the
/// playbook, anything else is taken verbatim.
pub fn resolve_repo_url(playbook: &Playbook, url: &str) -> String {
    let as_path = Path::new(url);
    if as_path.is_relative() && playbook.resolve_path(as_path).join(".git").exists() {
        playbook.resolve_path(as_path).display().to_string()
    } else {
        url.to_string()
    }
}

/// One comparable identity for a repository URL or local path, so
/// equivalent spellings name the same repository: a local path
/// canonicalizes; a URL drops its scheme, user, trailing slash, and
/// `.git` suffix, so `https://github.com/o/r.git`, `git@github.com:o/r`,
/// and `ssh://git@github.com/o/r/` all compare equal.
pub fn repo_identity(url: &str) -> String {
    let trimmed = url.trim();
    let local = Path::new(trimmed);
    if local.exists() {
        if let Ok(canonical) = local.canonicalize() {
            return canonical.display().to_string();
        }
    }
    let rest = match trimmed.split_once("://") {
        Some((_, rest)) => rest,
        None => trimmed,
    };
    // `user@host:path` (scp-like) → `host/path`; `user@host/path` →
    // `host/path`.
    let rest = match rest.split_once('@') {
        Some((user, after)) if !user.contains('/') => after,
        _ => rest,
    };
    let rest = match rest.split_once(':') {
        Some((host, path)) if !host.contains('/') && !path.starts_with("//") => {
            format!("{host}/{}", path.trim_start_matches('/'))
        }
        _ => rest.to_string(),
    };
    let rest = rest.trim_end_matches('/');
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    rest.trim_end_matches('/').to_ascii_lowercase()
}

/// A repository named by a `coverage.scan` entry, ready to scan.
struct ScanScope<'a> {
    index: usize,
    config: &'a ScanConfig,
    /// A local worktree (`path`), canonicalized.
    local_dir: Option<PathBuf>,
    /// A git repository (`repo`): the resolved URL.
    repo_url: Option<String>,
    /// The comparable identity of the repository (see [`repo_identity`]).
    identity: String,
    /// Provenance recorded on claims.
    repo: Option<String>,
    rev: Option<String>,
    /// For a local worktree: the enclosing repository's root, so a
    /// claim's file can be compared against `rev`.
    repo_root: Option<PathBuf>,
}

/// The repository-relative path of a page under one scan scope, when
/// the page's content root belongs to that repository.
fn locate_in_scope(scope: &ScanScope<'_>, root: &LoadedRoot, file: &VirtualFile) -> Option<String> {
    let rel = file
        .src_path
        .strip_prefix(&root.root)
        .ok()?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/");

    if let Some((url, start_path)) = &root.git {
        // A git content source belongs to the scan entry naming the same
        // repository, whichever way either side spells it.
        if repo_identity(url) != scope.identity {
            return None;
        }
        return Some(join_repo_path(start_path, &rel));
    }

    // A directory content source belongs to a local scan entry whose
    // directory encloses it, or to a `repo` entry naming that directory's
    // clone.
    let root_dir = root.root.canonicalize().ok()?;
    let base = match (&scope.local_dir, &scope.repo_url) {
        (Some(dir), _) => dir.clone(),
        (None, Some(url)) => Path::new(url).canonicalize().ok()?,
        (None, None) => return None,
    };
    let prefix = root_dir
        .strip_prefix(&base)
        .ok()?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/");
    Some(join_repo_path(&prefix, &rel))
}

fn join_repo_path(prefix: &str, rel: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{prefix}/{rel}")
    }
}

/// Runs the scan: measures the pages of every scanned repository, scans
/// its test roots and spec map, and resolves everything into the
/// database. The database's `errors` are the caller's to report.
pub fn scan(
    playbook: &Playbook,
    aggregator: &Aggregator,
    pipeline: &Pipeline,
    roots: &[LoadedRoot],
) -> anyhow::Result<CoverageDatabase> {
    let mut scopes = Vec::new();
    for (index, config) in playbook.coverage.scan.iter().enumerate() {
        config.validate().map_err(anyhow::Error::msg)?;
        let scope = if let Some(path) = &config.path {
            let dir = playbook.resolve_path(path);
            let dir = dir
                .canonicalize()
                .with_context(|| format!("coverage.scan path {}", dir.display()))?;
            let info = local_repo_info(&dir);
            ScanScope {
                index,
                config,
                identity: repo_identity(&dir.display().to_string()),
                local_dir: Some(dir),
                repo_url: None,
                repo: info.as_ref().and_then(|i| i.remote_url.clone()),
                rev: info.as_ref().and_then(|i| i.head.clone()),
                repo_root: info.map(|i| i.root),
            }
        } else {
            let url = resolve_repo_url(playbook, config.repo.as_deref().expect("validated"));
            ScanScope {
                index,
                config,
                identity: repo_identity(&url),
                local_dir: None,
                repo_url: Some(url.clone()),
                repo: Some(url),
                rev: None,
                repo_root: None,
            }
        };
        scopes.push(scope);
    }

    // Measure: each page belongs to the first scope whose repository
    // holds it and whose `pages` filter admits it. A component version
    // may be distributed over several roots, so the root is the one
    // whose directory holds the file, not the first with that key.
    let measured = pipeline.measure_pages(|file| {
        let root = roots.iter().find(|root| {
            root.component.0 == file.coords.component
                && root.component.1 == file.coords.version
                && file.src_path.starts_with(&root.root)
        })?;
        scopes.iter().find_map(|scope| {
            let repo_path = locate_in_scope(scope, root, file)?;
            scope
                .config
                .measures(&repo_path)
                .then_some((repo_path, scope.index))
        })
    })?;

    // Collect: claims from every test root, sidecars from every spec map.
    let mut claims: Vec<Claim> = Vec::new();
    let mut sidecars: Vec<Sidecar> = Vec::new();
    for scope in &scopes {
        if let Some(dir) = &scope.local_dir {
            for tests in &scope.config.tests {
                let root = TestRoot {
                    dir: dir.join(tests),
                    repo_prefix: tests.clone(),
                    local: true,
                    repo: scope.repo.clone(),
                    rev: scope.rev.clone(),
                    scope: scope.index,
                    krate: None,
                };
                let mut found = scan_test_root(&root)
                    .with_context(|| format!("scanning test root {}", root.dir.display()))?;

                // A worktree is read as is, uncommitted edits included: a
                // claim keeps `HEAD` as its revision only when its file is
                // what `HEAD` records, so click-through links never point
                // at code that is not there.
                let mut verdicts: HashMap<PathBuf, bool> = HashMap::new();
                for claim in &mut found {
                    let at_head = match (&scope.repo_root, &claim.site.local_path) {
                        (Some(repo_root), Some(path)) => {
                            *verdicts.entry(path.clone()).or_insert_with(|| {
                                path.strip_prefix(repo_root)
                                    .ok()
                                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                                    .and_then(|rel| file_matches_head(repo_root, &rel))
                                    .unwrap_or(false)
                            })
                        }
                        _ => false,
                    };
                    if !at_head {
                        claim.site.rev = None;
                    }
                }
                claims.extend(found);
            }
            if let Some(spec_map) = &scope.config.spec_map {
                sidecars.extend(load_spec_map(&dir.join(spec_map), spec_map, scope.index)?);
            }
        } else {
            let url = scope.repo_url.clone().expect("git scope");
            let reference = scope.config.reference.as_deref().unwrap_or("HEAD");
            let export = |start_path: &str| -> anyhow::Result<(PathBuf, String)> {
                let source = GitSource {
                    url: url.clone(),
                    branches: Vec::new(),
                    tags: Vec::new(),
                    start_path: start_path.to_string(),
                    version_from_ref: false,
                };
                let mut collected = aggregator.collect_ref(&source, reference)?;
                let root = collected.pop().context("no ref collected")?;
                Ok((root.path, root.commit))
            };
            for tests in &scope.config.tests {
                let (dir, commit) = export(tests)
                    .with_context(|| format!("exporting test root {tests} of {url}"))?;

                // The export is a bare subtree: the crate's `Cargo.toml`
                // usually sits above it, so read the nearest ancestor
                // manifest straight from the repository.
                let source = GitSource {
                    url: url.clone(),
                    branches: Vec::new(),
                    tags: Vec::new(),
                    start_path: String::new(),
                    version_from_ref: false,
                };
                let mut krate = None;
                let mut ancestor = Path::new(tests.trim_matches('/')).parent();
                while let Some(dir) = ancestor {
                    let manifest =
                        join_repo_path(&dir.to_string_lossy().replace('\\', "/"), "Cargo.toml");
                    if let Some(bytes) = aggregator.read_blob(&source, reference, &manifest)? {
                        krate = manifest_package_name(&String::from_utf8_lossy(&bytes));
                        if krate.is_some() {
                            break;
                        }
                    }
                    ancestor = dir.parent();
                }

                let root = TestRoot {
                    dir,
                    repo_prefix: tests.clone(),
                    local: false,
                    repo: scope.repo.clone(),
                    rev: Some(commit),
                    scope: scope.index,
                    krate,
                };
                claims.extend(scan_test_root(&root)?);
            }
            if let Some(spec_map) = &scope.config.spec_map {
                let (dir, _) = export(spec_map)
                    .with_context(|| format!("exporting spec map {spec_map} of {url}"))?;
                sidecars.extend(load_spec_map(&dir, spec_map, scope.index)?);
            }
        }
    }

    Ok(resolve(&measured, claims, &sidecars))
}

/// The click-through URL of a claim (RFC 0001 §8): the playbook's
/// `coverage.link_template`, else the GitHub blob URL when the claim's
/// repository is on GitHub; `None` when neither applies.
pub fn claim_url(claim: &Claim, template: Option<&str>) -> Option<String> {
    claim_url_at(claim, claim.site.line, template)
}

/// [`claim_url`] pointing at another line of the claim's file (the
/// enclosing test function's first line, for the inlined source).
pub fn claim_url_at(claim: &Claim, line: u32, template: Option<&str>) -> Option<String> {
    let repo = claim.site.repo.as_deref()?;
    let rev = claim.site.rev.as_deref()?;
    let slug = github_slug(repo);
    match template {
        Some(template) => Some(
            template
                .replace("{repo}", slug.as_deref().unwrap_or(repo))
                .replace("{repo_url}", repo)
                .replace("{rev}", rev)
                .replace("{path}", &claim.site.file)
                .replace("{line}", &line.to_string()),
        ),
        None => slug.map(|slug| {
            format!(
                "https://github.com/{slug}/blob/{rev}/{}#L{line}",
                claim.site.file
            )
        }),
    }
}

/// The `owner/name` slug of a GitHub repository URL (HTTPS or SSH).
fn github_slug(repo: &str) -> Option<String> {
    let rest = repo
        .strip_prefix("https://github.com/")
        .or_else(|| repo.strip_prefix("http://github.com/"))
        .or_else(|| repo.strip_prefix("git@github.com:"))
        .or_else(|| repo.strip_prefix("ssh://git@github.com/"))?;
    let slug = rest.trim_end_matches('/').trim_end_matches(".git");
    (slug.split('/').count() == 2).then(|| slug.to_string())
}

#[cfg(test)]
mod tests {
    use bokfell_coverage::{ClaimSite, ClaimTarget};

    use super::*;

    fn claim(repo: Option<&str>, rev: Option<&str>) -> Claim {
        Claim {
            target: ClaimTarget::Path("p.adoc".into()),
            anchor: None,
            excerpt: Some("x".into()),
            site: ClaimSite {
                file: "html5/src/tests/lists.rs".into(),
                local_path: None,
                line: 42,
                test_fn: None,
                fn_line: None,
                fn_source: None,
                krate: None,
                repo: repo.map(str::to_string),
                rev: rev.map(str::to_string),
                scope: 0,
            },
        }
    }

    #[test]
    fn github_repos_link_without_configuration() {
        let c = claim(
            Some("https://github.com/asciidoc-rs/asciidoc-html5"),
            Some("abc"),
        );
        assert_eq!(
            claim_url(&c, None).as_deref(),
            Some("https://github.com/asciidoc-rs/asciidoc-html5/blob/abc/html5/src/tests/lists.rs#L42")
        );
        let ssh = claim(
            Some("git@github.com:asciidoc-rs/asciidoc-html5.git"),
            Some("abc"),
        );
        assert!(claim_url(&ssh, None)
            .unwrap()
            .contains("/asciidoc-rs/asciidoc-html5/blob/"));

        assert_eq!(
            claim_url(&claim(Some("https://gitlab.com/o/r"), Some("abc")), None),
            None
        );
        assert_eq!(
            claim_url(&claim(Some("https://github.com/o/r"), None), None),
            None
        );
    }

    #[test]
    fn link_template_substitutes_every_token() {
        let c = claim(Some("https://gitlab.com/o/r"), Some("abc"));
        assert_eq!(
            claim_url(&c, Some("{repo_url}/-/blob/{rev}/{path}#L{line} ({repo})")).as_deref(),
            Some("https://gitlab.com/o/r/-/blob/abc/html5/src/tests/lists.rs#L42 (https://gitlab.com/o/r)")
        );
    }

    #[test]
    fn equivalent_repository_spellings_share_an_identity() {
        let canonical = repo_identity("https://github.com/asciidoc-rs/asciidoc-html5");
        for spelling in [
            "https://github.com/asciidoc-rs/asciidoc-html5.git",
            "https://github.com/asciidoc-rs/asciidoc-html5/",
            "git@github.com:asciidoc-rs/asciidoc-html5.git",
            "ssh://git@github.com/asciidoc-rs/asciidoc-html5",
            "HTTPS://GitHub.com/asciidoc-rs/asciidoc-html5",
        ] {
            assert_eq!(repo_identity(spelling), canonical, "{spelling}");
        }
        assert_ne!(
            repo_identity("https://github.com/asciidoc-rs/asciidoc-parser"),
            canonical
        );
        assert_ne!(
            repo_identity("https://gitlab.com/o/r"),
            repo_identity("https://github.com/o/r")
        );
    }

    #[test]
    fn relative_urls_resolve_only_to_local_repositories() {
        let dir = std::env::temp_dir().join(format!("bokfell-repo-url-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("clone/.git")).unwrap();
        std::fs::create_dir_all(dir.join("plain")).unwrap();
        std::fs::write(
            dir.join("bokfell.yml"),
            "site:\n  title: T\ncontent:\n  sources:\n    - path: docs\n",
        )
        .unwrap();
        let playbook = Playbook::load(&dir.join("bokfell.yml")).unwrap();

        assert_eq!(
            resolve_repo_url(&playbook, "clone"),
            dir.join("clone").display().to_string()
        );
        assert_eq!(resolve_repo_url(&playbook, "plain"), "plain");
        assert_eq!(
            resolve_repo_url(&playbook, "https://github.com/o/r"),
            "https://github.com/o/r"
        );

        // Identities of local paths canonicalize; a path that is not
        // there normalizes like a URL.
        assert_eq!(
            repo_identity(&dir.join("clone").display().to_string()),
            dir.join("clone")
                .canonicalize()
                .unwrap()
                .display()
                .to_string()
        );
        assert_eq!(repo_identity("../nowhere/"), "../nowhere");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repo_paths_join_cleanly() {
        assert_eq!(join_repo_path("", "modules/x.adoc"), "modules/x.adoc");
        assert_eq!(
            join_repo_path("docs/", "modules/x.adoc"),
            "docs/modules/x.adoc"
        );
    }
}
