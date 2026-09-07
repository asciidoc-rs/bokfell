//! Content aggregation for the Bokfell documentation site generator.
//!
//! Antora's defining trick, in Rust: content sources can be **git
//! repositories** — each ref matching the source's branch/tag patterns
//! becomes a content root, so branches and tags become component versions
//! (PLAN.md §5, M3). Remote repositories are cloned bare into a cache and
//! read without a checkout; the matched trees are exported into
//! content-addressed cache directories (keyed by commit id, so unchanged
//! refs cost nothing on rebuild) that the content catalog then scans like
//! any local root.
//!
//! Local, non-git directory sources bypass this crate entirely; a `url`
//! may also point at a local git repository (a clone or worktree), which
//! is opened in place — no cache copy of the repository itself.

use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use bokfell_model::version_from_refname;

/// Errors from aggregating git content sources.
#[derive(Debug, thiserror::Error)]
pub enum AggregateError {
    /// A repository could not be opened or cloned.
    #[error("cannot open or clone {url}: {message}")]
    Repo {
        /// The source URL.
        url: String,
        /// The underlying error, stringified (gix error types are deep).
        message: String,
    },

    /// Ref enumeration failed.
    #[error("cannot enumerate refs of {url}: {message}")]
    Refs {
        /// The source URL.
        url: String,
        /// The underlying error, stringified.
        message: String,
    },

    /// No ref matched the source's branch/tag patterns.
    #[error("no ref of {url} matches branches {branches:?} / tags {tags:?}")]
    NoMatchingRef {
        /// The source URL.
        url: String,
        /// The configured branch patterns.
        branches: Vec<String>,
        /// The configured tag patterns.
        tags: Vec<String>,
    },

    /// The start path does not exist in a matched ref.
    #[error("start path {start_path:?} not found in {url} ref {refname}")]
    StartPathMissing {
        /// The source URL.
        url: String,
        /// The matched ref.
        refname: String,
        /// The missing start path.
        start_path: String,
    },

    /// Exporting a tree to the cache failed.
    #[error("cannot export {url} ref {refname}: {message}")]
    Export {
        /// The source URL.
        url: String,
        /// The matched ref.
        refname: String,
        /// The underlying error, stringified.
        message: String,
    },
}

/// One git content source, as configured in the playbook.
#[derive(Clone, Debug)]
pub struct GitSource {
    /// Repository URL (remote) or filesystem path (a local clone or
    /// worktree, opened in place).
    pub url: String,
    /// Branch name patterns (`*` glob, `!` negation). `HEAD` matches the
    /// repository's current branch. Defaults to `[HEAD]` when both this
    /// and `tags` are empty.
    pub branches: Vec<String>,
    /// Tag name patterns (`*` glob, `!` negation).
    pub tags: Vec<String>,
    /// Path of the content root (the directory holding `antora.yml`)
    /// within the repository. Empty = repository root.
    pub start_path: String,
    /// Derive each matched ref's component version from the ref name
    /// (`asciidoc-html5-v0.2.1` → `0.2.1`, `main` → `main`) instead of the
    /// version in `antora.yml`.
    pub version_from_ref: bool,
}

/// One content root produced by aggregation, ready for the catalog.
#[derive(Clone, Debug)]
pub struct CollectedRoot {
    /// The directory to scan (an export cache dir, or a local worktree
    /// subdirectory).
    pub path: PathBuf,
    /// Version override for `scan_source_versioned`, when
    /// `version_from_ref` is set.
    pub version_override: Option<String>,
    /// The ref this root came from (for messages).
    pub refname: String,
}

/// The aggregator: owns the cache location and fetch policy.
pub struct Aggregator {
    cache_dir: PathBuf,
    fetch: bool,
}

impl Aggregator {
    /// Creates an aggregator caching under `cache_dir`; `fetch` refreshes
    /// already-cached remote repositories.
    pub fn new(cache_dir: PathBuf, fetch: bool) -> Self {
        Aggregator { cache_dir, fetch }
    }

    /// The default cache directory (`$XDG_CACHE_HOME/bokfell` or
    /// `~/.cache/bokfell`, falling back to the system temp dir).
    pub fn default_cache_dir() -> PathBuf {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("bokfell")
    }

    /// Aggregates one git source into content roots, one per matched ref.
    /// Collects one *named* ref: tried as a branch first, then as a tag
    /// — never both namespaces at once, so a repository holding a branch
    /// and a tag with the same name yields that ref's content exactly
    /// once (the branch wins). `source`'s own branch/tag patterns are
    /// ignored; everything else (URL, start path, versioning) applies.
    pub fn collect_ref(
        &self,
        source: &GitSource,
        refname: &str,
    ) -> Result<Vec<CollectedRoot>, AggregateError> {
        let as_branch = GitSource {
            branches: vec![refname.to_string()],
            tags: Vec::new(),
            ..source.clone()
        };
        match self.collect(&as_branch) {
            Err(AggregateError::NoMatchingRef { .. }) => self.collect(&GitSource {
                branches: Vec::new(),
                tags: vec![refname.to_string()],
                ..source.clone()
            }),
            other => other,
        }
    }

    pub fn collect(&self, source: &GitSource) -> Result<Vec<CollectedRoot>, AggregateError> {
        let repo = self.open_or_clone(&source.url)?;
        let matched = matched_refs(&repo, source).map_err(|message| AggregateError::Refs {
            url: source.url.clone(),
            message,
        })?;

        if matched.is_empty() {
            return Err(AggregateError::NoMatchingRef {
                url: source.url.clone(),
                branches: source.branches.clone(),
                tags: source.tags.clone(),
            });
        }

        let mut roots = Vec::new();
        for (refname, commit_id) in matched {
            let path = self.export_ref(&repo, source, &refname, commit_id)?;
            let version_override = source
                .version_from_ref
                .then(|| version_from_refname(&refname));
            roots.push(CollectedRoot {
                path,
                version_override,
                refname,
            });
        }
        Ok(roots)
    }

    fn open_or_clone(&self, url: &str) -> Result<gix::Repository, AggregateError> {
        let as_error = |e: String| AggregateError::Repo {
            url: url.to_string(),
            message: e,
        };

        // A local path (clone or worktree) is opened in place.
        let local = Path::new(url);
        if local.exists() {
            return gix::open(local).map_err(|e| as_error(e.to_string()));
        }

        // Remote: bare clone into the cache, reused across builds.
        let repo_dir = self.cache_dir.join("repos").join(cache_key(url));
        if repo_dir.is_dir() {
            let repo = gix::open(&repo_dir).map_err(|e| as_error(e.to_string()))?;
            if self.fetch {
                fetch_repo(&repo).map_err(as_error)?;
            }
            return Ok(repo);
        }

        std::fs::create_dir_all(&repo_dir).map_err(|e| as_error(e.to_string()))?;
        let interrupt = AtomicBool::new(false);
        let (repo, _outcome) = gix::prepare_clone_bare(url, &repo_dir)
            .map_err(|e| as_error(e.to_string()))?
            .fetch_only(gix::progress::Discard, &interrupt)
            .map_err(|e| as_error(e.to_string()))?;
        Ok(repo)
    }

    /// Exports `start_path` of the ref's tree into the cache, keyed by
    /// commit id so unchanged refs are reused as-is.
    fn export_ref(
        &self,
        repo: &gix::Repository,
        source: &GitSource,
        refname: &str,
        commit_id: gix::ObjectId,
    ) -> Result<PathBuf, AggregateError> {
        let as_error = |message: String| AggregateError::Export {
            url: source.url.clone(),
            refname: refname.to_string(),
            message,
        };

        // The export's identity is (commit, exact start path). The readable
        // sanitized name alone is lossy (`docs/a` and `docs_a` would
        // collide), so the exact path's hash is part of the key.
        let dest = self
            .cache_dir
            .join("exports")
            .join(commit_id.to_string())
            .join(format!(
                "{}-{:08x}",
                sanitize(&source.start_path),
                fnv1a64(source.start_path.as_bytes()) as u32
            ));
        let marker = dest.join(".bokfell-export-complete");
        if marker.is_file() {
            return Ok(dest);
        }

        let commit = repo
            .find_object(commit_id)
            .map_err(|e| as_error(e.to_string()))?
            .try_into_commit()
            .map_err(|e| as_error(e.to_string()))?;
        let mut tree = commit.tree().map_err(|e| as_error(e.to_string()))?;

        // Navigate to the start path within the tree.
        if !source.start_path.is_empty() {
            let entry = tree
                .lookup_entry_by_path(&source.start_path)
                .map_err(|e| as_error(e.to_string()))?
                .ok_or_else(|| AggregateError::StartPathMissing {
                    url: source.url.clone(),
                    refname: refname.to_string(),
                    start_path: source.start_path.clone(),
                })?;
            tree = entry
                .object()
                .map_err(|e| as_error(e.to_string()))?
                .try_into_tree()
                .map_err(|e| as_error(e.to_string()))?;
        }

        // Publish atomically so concurrent builds sharing the cache never
        // observe a partial export: build the whole tree (marker included)
        // in a process-unique staging directory, then rename it into
        // place. Losing the rename race to a completed export is success.
        let staging = dest.with_file_name(format!(
            ".staging-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&staging).map_err(|e| as_error(e.to_string()))?;
        let result = export_tree(&tree, &staging).and_then(|()| {
            std::fs::write(
                staging.join(".bokfell-export-complete"),
                commit_id.to_string(),
            )
            .map_err(|e| e.to_string())
        });
        if let Err(message) = result {
            std::fs::remove_dir_all(&staging).ok();
            return Err(as_error(message));
        }

        for attempt in 0..2 {
            match std::fs::rename(&staging, &dest) {
                Ok(()) => return Ok(dest),
                Err(_) if marker.is_file() => {
                    // Another build published this export first.
                    std::fs::remove_dir_all(&staging).ok();
                    return Ok(dest);
                }
                Err(e) if attempt == 0 => {
                    // A stale partial from a crashed build: clear and retry.
                    std::fs::remove_dir_all(&dest).ok();
                    let _ = e;
                }
                Err(e) => {
                    std::fs::remove_dir_all(&staging).ok();
                    return Err(as_error(e.to_string()));
                }
            }
        }
        unreachable!("the retry loop returns on every path");
    }
}

/// Enumerates the refs matching the source's patterns as
/// `(short name, commit id)`.
fn matched_refs(
    repo: &gix::Repository,
    source: &GitSource,
) -> Result<Vec<(String, gix::ObjectId)>, String> {
    let mut branches = source.branches.clone();
    if branches.is_empty() && source.tags.is_empty() {
        branches.push("HEAD".to_string());
    }

    // Refs are deduplicated by *short name*: `refs/heads/x` and
    // `refs/remotes/origin/x` are the same logical branch (the local one
    // wins by iteration order), while a tag and a branch pointing at the
    // same commit are still two distinct versions.
    let mut matched: Vec<(String, gix::ObjectId)> = Vec::new();
    let mut seen_branches = std::collections::HashSet::new();

    // `HEAD` names the repository's current branch — but a negative
    // pattern still excludes that branch (`[HEAD, '!main']` with HEAD on
    // `main` selects nothing).
    if branches.iter().any(|p| p == "HEAD") {
        if let Ok(Some(mut head)) = repo.head_ref() {
            let name = head.name().shorten().to_string();
            if !excluded_by_negation(&name, &source.branches) {
                let id = head.peel_to_commit().map_err(|e| e.to_string())?.id;
                if seen_branches.insert(name.clone()) {
                    matched.push((name, id));
                }
            }
        }
    }

    let platform = repo.references().map_err(|e| e.to_string())?;

    // Branches may live under `refs/heads/` (local repos, mirror clones)
    // or `refs/remotes/origin/` (cache clones); consult both. When a
    // branch exists under both, prefer the remote-tracking ref — for a
    // repository used as a content *source*, `origin/x` is the published
    // state of `x`, while a local `refs/heads/x` may lag behind it.
    for prefix in ["refs/remotes/origin/", "refs/heads/"] {
        let iter = platform.prefixed(prefix).map_err(|e| e.to_string())?;
        for reference in iter.flatten() {
            let mut reference = reference;
            let full = reference.name().as_bstr().to_string();
            let short = full.trim_start_matches(prefix).to_string();
            if short == "HEAD" || !matches_patterns(&short, &source.branches) {
                continue;
            }
            if !seen_branches.contains(&short) {
                if let Ok(commit) = reference.peel_to_commit() {
                    seen_branches.insert(short.clone());
                    matched.push((short, commit.id));
                }
            }
        }
    }

    let mut seen_tags = std::collections::HashSet::new();
    for reference in platform
        .prefixed("refs/tags/")
        .map_err(|e| e.to_string())?
        .flatten()
    {
        let mut reference = reference;
        let full = reference.name().as_bstr().to_string();
        let short = full.trim_start_matches("refs/tags/").to_string();
        if !matches_patterns(&short, &source.tags) {
            continue;
        }
        if !seen_tags.contains(&short) {
            if let Ok(commit) = reference.peel_to_commit() {
                seen_tags.insert(short.clone());
                matched.push((short, commit.id));
            }
        }
    }

    Ok(matched)
}

fn fetch_repo(repo: &gix::Repository) -> Result<(), String> {
    let remote = repo
        .find_default_remote(gix::remote::Direction::Fetch)
        .ok_or("repository has no fetch remote")?
        .map_err(|e| e.to_string())?;
    let interrupt = AtomicBool::new(false);
    remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(|e| e.to_string())?
        .prepare_fetch(gix::progress::Discard, Default::default())
        .map_err(|e| e.to_string())?
        .receive(gix::progress::Discard, &interrupt)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Recursively writes a tree's blobs under `dest`.
fn export_tree(tree: &gix::Tree<'_>, dest: &Path) -> Result<(), String> {
    for entry in tree.iter() {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.filename().to_string();

        // Guard against path escapes from hostile tree entries.
        if name.contains('/') || name.contains('\\') || name == ".." || name.is_empty() {
            continue;
        }
        let target = dest.join(&name);

        match entry.mode().kind() {
            gix::object::tree::EntryKind::Tree => {
                let subtree = entry
                    .object()
                    .map_err(|e| e.to_string())?
                    .try_into_tree()
                    .map_err(|e| e.to_string())?;
                std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
                export_tree(&subtree, &target)?;
            }
            gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable => {
                let object = entry.object().map_err(|e| e.to_string())?;
                std::fs::write(&target, &object.data).map_err(|e| e.to_string())?;
            }
            // Symlinks and submodules are not content.
            _ => {}
        }
    }
    Ok(())
}

/// Whether any negative (`!`-prefixed) pattern matches the name.
fn excluded_by_negation(name: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .filter_map(|p| p.strip_prefix('!'))
        .any(|negated| glob_match(negated, name))
}

/// Matches a ref short-name against glob patterns (`*` wildcards), where a
/// leading `!` negates: the name must match at least one positive pattern
/// and no negative one.
fn matches_patterns(name: &str, patterns: &[String]) -> bool {
    let mut matched = false;
    for pattern in patterns {
        if let Some(negated) = pattern.strip_prefix('!') {
            if glob_match(negated, name) {
                return false;
            }
        } else if pattern == "HEAD" {
            // Handled separately by the caller.
        } else if glob_match(pattern, name) {
            matched = true;
        }
    }
    matched
}

/// Minimal `*` glob matching (no character classes).
fn glob_match(pattern: &str, name: &str) -> bool {
    fn inner(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => inner(&p[1..], n) || (!n.is_empty() && inner(p, &n[1..])),
            (Some(pc), Some(nc)) if pc == nc => inner(&p[1..], &n[1..]),
            _ => false,
        }
    }
    inner(pattern.as_bytes(), name.as_bytes())
}

/// A stable, readable cache directory name for a repository URL.
fn cache_key(url: &str) -> String {
    let tail: String = url
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repo")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    format!("{tail}-{:016x}", fnv1a64(url.as_bytes()))
}

/// FNV-1a, 64-bit: a tiny hash with a *stable* definition, safe to bake
/// into on-disk cache paths (`DefaultHasher` is explicitly not stable
/// across Rust releases).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn sanitize(path: &str) -> String {
    if path.is_empty() {
        "_root".to_string()
    } else {
        path.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_and_negation_matching() {
        assert!(glob_match("v*", "v1.2.3"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("release/*", "release/2.0"));
        assert!(!glob_match("v*", "main"));

        let patterns = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(matches_patterns("v1.2", &patterns(&["v*"])));
        assert!(!matches_patterns("v1.2", &patterns(&["v*", "!v1.*"])));
        assert!(matches_patterns("v2.0", &patterns(&["v*", "!v1.*"])));
        assert!(!matches_patterns("main", &patterns(&["v*"])));
    }

    #[test]
    fn negation_applies_to_head_resolution() {
        let patterns: Vec<String> = vec!["HEAD".to_string(), "!main".to_string()];
        assert!(excluded_by_negation("main", &patterns));
        assert!(!excluded_by_negation("develop", &patterns));
    }

    #[test]
    fn export_keys_distinguish_lossy_start_paths() {
        assert_ne!(fnv1a64(b"docs/a"), fnv1a64(b"docs_a"));
        assert_eq!(sanitize("docs/a"), sanitize("docs_a"));
    }

    #[test]
    fn cache_keys_are_stable_and_distinct() {
        let a = cache_key("https://github.com/asciidoc-rs/asciidoc-html5");
        let b = cache_key("https://github.com/asciidoc-rs/asciidoc-parser");
        assert_ne!(a, b);
        assert_eq!(
            a,
            cache_key("https://github.com/asciidoc-rs/asciidoc-html5")
        );
        assert!(a.starts_with("asciidoc-html5-"));
    }
}
