//! Claim extraction: the `syn`-based static scan of test roots (RFC 0001
//! §4, §6).
//!
//! Every `.rs` file under a test root is parsed properly and every
//! `verifies!` invocation — any nesting, any formatting — becomes a
//! [`Claim`] carrying the span of its enclosing `fn`. Scanned repositories
//! take no dependency: `verifies!` is a no-op `macro_rules!` marker there.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use syn::{punctuated::Punctuated, spanned::Spanned, visit::Visit, LitStr, Token};

use crate::model::{Claim, ClaimSite, ClaimTarget};

/// Errors from scanning a test root.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// A directory or file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The path being read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A Rust file failed to parse.
    #[error("{file}: cannot parse: {message}")]
    Parse {
        /// The file.
        file: String,
        /// The parser's message.
        message: String,
    },

    /// A `verifies!` invocation does not follow the claim grammar.
    #[error("{file}:{line}: invalid verifies! invocation: {message}")]
    Invocation {
        /// The file.
        file: String,
        /// The invocation's line.
        line: u32,
        /// What was wrong.
        message: String,
    },
}

/// One test root to scan.
#[derive(Clone, Debug)]
pub struct TestRoot {
    /// The directory holding the `.rs` files.
    pub dir: PathBuf,
    /// The directory's path within its repository (`/`-separated; empty
    /// for the repository root) — claim files are reported relative to
    /// the repository.
    pub repo_prefix: String,
    /// Whether the directory is a local source (a worktree), so claims
    /// may carry the file's absolute path for the editor.
    pub local: bool,
    /// The repository the root came from, for provenance.
    pub repo: Option<String>,
    /// The revision the root was read at, for provenance.
    pub rev: Option<String>,
    /// The `coverage.scan` entry index the root belongs to.
    pub scope: usize,
    /// The crate name to record when no `Cargo.toml` is found at or
    /// above a file within the root — a remote root is exported as a
    /// bare subtree, so the caller supplies the enclosing package.
    pub krate: Option<String>,
}

/// Scans one test root for `verifies!` claims, in file order.
pub fn scan_test_root(root: &TestRoot) -> Result<Vec<Claim>, ScanError> {
    let mut files = Vec::new();
    walk(&root.dir, &mut files)?;
    files.sort();

    let mut crates: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut claims = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(&root.dir)
            .expect("walked file is under the root")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        let file = if root.repo_prefix.is_empty() {
            rel
        } else {
            format!("{}/{rel}", root.repo_prefix.trim_matches('/'))
        };

        let text = std::fs::read_to_string(&path).map_err(|source| ScanError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let krate = crate_name_for(&path, &mut crates).or_else(|| root.krate.clone());
        let site = ClaimSite {
            file,
            local_path: root.local.then(|| path.clone()),
            line: 0,
            test_fn: None,
            fn_line: None,
            fn_source: None,
            krate,
            repo: root.repo.clone(),
            rev: root.rev.clone(),
            scope: root.scope,
        };
        claims.extend(scan_source(&text, &site)?);
    }
    Ok(claims)
}

/// Scans one Rust source text for `verifies!` claims; `site` supplies
/// every provenance field but the line and function.
pub fn scan_source(text: &str, site: &ClaimSite) -> Result<Vec<Claim>, ScanError> {
    let ast = syn::parse_file(text).map_err(|e| ScanError::Parse {
        file: site.file.clone(),
        message: e.to_string(),
    })?;
    let mut visitor = Visitor {
        site,
        lines: text.lines().collect(),
        fn_stack: Vec::new(),
        claims: Vec::new(),
        error: None,
    };
    visitor.visit_file(&ast);
    match visitor.error {
        Some(error) => Err(error),
        None => Ok(visitor.claims),
    }
}

/// An enclosing function: its name and 1-based line span (attributes
/// included).
struct EnclosingFn {
    name: String,
    start: u32,
    end: u32,
}

struct Visitor<'a> {
    site: &'a ClaimSite,
    /// The source, by line, for extracting enclosing functions.
    lines: Vec<&'a str>,
    fn_stack: Vec<EnclosingFn>,
    claims: Vec<Claim>,
    error: Option<ScanError>,
}

impl Visitor<'_> {
    /// The source text of `lines[start..=end]` (1-based), with the
    /// indentation every non-blank line shares removed.
    fn extract(&self, start: u32, end: u32) -> String {
        let start = (start.max(1) - 1) as usize;
        let end = (end as usize).min(self.lines.len());
        let block = &self.lines[start.min(end)..end];
        let indent = block
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);
        block
            .iter()
            .map(|line| {
                if line.len() >= indent {
                    &line[indent..]
                } else {
                    line.trim_start()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The 1-based line span of a function item: from its first attribute
/// (or its signature) to the closing brace of its body.
fn fn_span(attrs: &[syn::Attribute], sig: &syn::Signature, block: &syn::Block) -> (u32, u32) {
    let start = attrs
        .first()
        .map(|attr| attr.span().start().line)
        .unwrap_or_else(|| sig.span().start().line)
        .min(sig.fn_token.span.start().line);
    let end = block.brace_token.span.close().end().line;
    (start as u32, end as u32)
}

impl<'ast> Visit<'ast> for Visitor<'_> {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let (start, end) = fn_span(&item.attrs, &item.sig, &item.block);
        self.fn_stack.push(EnclosingFn {
            name: item.sig.ident.to_string(),
            start,
            end,
        });
        syn::visit::visit_item_fn(self, item);
        self.fn_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let (start, end) = fn_span(&item.attrs, &item.sig, &item.block);
        self.fn_stack.push(EnclosingFn {
            name: item.sig.ident.to_string(),
            start,
            end,
        });
        syn::visit::visit_impl_item_fn(self, item);
        self.fn_stack.pop();
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let is_verifies = mac
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "verifies");
        if is_verifies && self.error.is_none() {
            let line = mac.path.span().start().line as u32;
            match parse_invocation(mac) {
                Ok((target, anchor, excerpt)) => {
                    let enclosing = self.fn_stack.last();
                    self.claims.push(Claim {
                        target,
                        anchor,
                        excerpt,
                        site: ClaimSite {
                            line,
                            test_fn: enclosing.map(|f| f.name.clone()),
                            fn_line: enclosing.map(|f| f.start),
                            fn_source: enclosing.map(|f| self.extract(f.start, f.end)),
                            ..self.site.clone()
                        },
                    })
                }
                Err(message) => {
                    self.error = Some(ScanError::Invocation {
                        file: self.site.file.clone(),
                        line,
                        message,
                    })
                }
            }
        }
        syn::visit::visit_macro(self, mac);
    }
}

type Invocation = (ClaimTarget, Option<String>, Option<String>);

/// Parses the three claim forms: `("<path>", "<excerpt>")`,
/// `("<path>#<anchor>", "<excerpt>")`, and `("<path>#<anchor>")`.
fn parse_invocation(mac: &syn::Macro) -> Result<Invocation, String> {
    let args = mac
        .parse_body_with(Punctuated::<LitStr, Token![,]>::parse_terminated)
        .map_err(|_| "expected one or two string literals".to_string())?;
    let mut args = args.into_iter().map(|lit| lit.value());
    let Some(first) = args.next() else {
        return Err("expected a page path".to_string());
    };
    let excerpt = args.next();
    if args.next().is_some() {
        return Err("expected at most two string literals".to_string());
    }

    let (page, anchor) = match first.split_once('#') {
        Some((page, anchor)) => (page.to_string(), Some(anchor.to_string())),
        None => (first, None),
    };
    if page.is_empty() {
        return Err("empty page path".to_string());
    }
    if anchor.as_deref().is_some_and(str::is_empty) {
        return Err("empty #anchor".to_string());
    }
    if excerpt.is_none() && anchor.is_none() {
        return Err("a claim without an excerpt must name a #anchor section".to_string());
    }
    if excerpt.as_deref().is_some_and(|e| e.trim().is_empty()) {
        return Err("empty excerpt".to_string());
    }

    // A resource ID carries coordinate separators; a repository path
    // never does.
    let target = if page.contains(':') {
        ClaimTarget::ResourceId(page)
    } else {
        ClaimTarget::Path(page.trim_start_matches("./").to_string())
    };
    Ok((target, anchor, excerpt))
}

/// The package name of the crate enclosing `file`: the nearest ancestor
/// `Cargo.toml` with a `[package]` table.
fn crate_name_for(file: &Path, cache: &mut HashMap<PathBuf, Option<String>>) -> Option<String> {
    let mut dir = file.parent()?;
    let mut visited: Vec<PathBuf> = Vec::new();
    let found = loop {
        if let Some(cached) = cache.get(dir) {
            break cached.clone();
        }
        visited.push(dir.to_path_buf());
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            if let Some(name) = package_name(&manifest) {
                break Some(name);
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break None,
        }
    };
    for dir in visited {
        cache.insert(dir, found.clone());
    }
    found
}

fn package_name(manifest: &Path) -> Option<String> {
    manifest_package_name(&std::fs::read_to_string(manifest).ok()?)
}

/// The `[package] name` of a `Cargo.toml`'s text, if it declares one.
pub fn manifest_package_name(text: &str) -> Option<String> {
    let value: toml::Value = toml::from_str(text).ok()?;
    value
        .get("package")?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ScanError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ScanError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry in entries {
        let path = entry
            .map_err(|source| ScanError::Io {
                path: dir.display().to_string(),
                source,
            })?
            .path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if name != "target" {
                walk(&path, out)?;
            }
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> ClaimSite {
        ClaimSite {
            file: "html5/src/tests/lists.rs".to_string(),
            local_path: None,
            line: 0,
            test_fn: None,
            fn_line: None,
            fn_source: None,
            krate: Some("asciidoc-html5".to_string()),
            repo: None,
            rev: None,
            scope: 0,
        }
    }

    #[test]
    fn finds_claims_anywhere_with_their_enclosing_fn() {
        let source = r##"
macro_rules! verifies { ($($t:tt)*) => {}; }

mod nested {
    #[test]
    fn nested_ordered_markers() {
        verifies!(
            "ref/asciidoc-lang/docs/modules/lists/pages/ordered.adoc",
            r#"To nest an ordered list, add a marker character for each level
of nesting."#
        );
        let _ = 1;
        if true {
            crate::verifies!("docs/modules/ROOT/pages/x.adoc#_intro", "scoped excerpt");
        }
    }
}

struct S;
impl S {
    fn method() {
        verifies!("docs/modules/ROOT/pages/x.adoc#_whole");
    }
}

verifies!("component:module:page.adoc", "cross repo");
"##;
        let claims = scan_source(source, &site()).unwrap();
        assert_eq!(claims.len(), 4, "{claims:#?}");

        assert_eq!(
            claims[0].target,
            ClaimTarget::Path("ref/asciidoc-lang/docs/modules/lists/pages/ordered.adoc".into())
        );
        assert_eq!(claims[0].anchor, None);
        assert!(claims[0]
            .excerpt
            .as_deref()
            .unwrap()
            .starts_with("To nest an ordered list"));
        assert_eq!(claims[0].site.line, 7);
        assert_eq!(
            claims[0].site.test_fn.as_deref(),
            Some("nested_ordered_markers")
        );
        assert_eq!(claims[0].label(), "asciidoc-html5::nested_ordered_markers");

        // The enclosing function is captured whole (attribute to closing
        // brace) with the module's indentation stripped.
        assert_eq!(claims[0].site.fn_line, Some(5));
        let source = claims[0].site.fn_source.as_deref().unwrap();
        assert!(
            source.starts_with("#[test]\nfn nested_ordered_markers() {"),
            "{source}"
        );
        assert!(source.ends_with("    }\n}"), "{source}");
        assert!(source.contains("\n    verifies!(\n"), "{source}");
        assert_eq!(claims[1].site.fn_source, claims[0].site.fn_source);
        assert_eq!(claims[2].site.fn_line, Some(21));
        assert!(claims[2]
            .site
            .fn_source
            .as_deref()
            .unwrap()
            .starts_with("fn method() {"));
        assert_eq!(claims[3].site.fn_source, None);

        assert_eq!(claims[1].anchor.as_deref(), Some("_intro"));
        assert_eq!(claims[1].excerpt.as_deref(), Some("scoped excerpt"));

        assert_eq!(claims[2].anchor.as_deref(), Some("_whole"));
        assert_eq!(claims[2].excerpt, None);
        assert_eq!(claims[2].site.test_fn.as_deref(), Some("method"));

        assert_eq!(
            claims[3].target,
            ClaimTarget::ResourceId("component:module:page.adoc".into())
        );
        assert_eq!(claims[3].site.test_fn, None);
    }

    #[test]
    fn rejects_malformed_invocations() {
        let bad = |src: &str| scan_source(src, &site()).unwrap_err().to_string();
        assert!(bad("fn t() { verifies!(); }").contains("expected a page path"));
        assert!(bad("fn t() { verifies!(\"p.adoc\"); }").contains("#anchor"));
        assert!(bad("fn t() { verifies!(\"p.adoc\", x); }").contains("string literal"));
        assert!(bad("fn t() { verifies!(\"p.adoc\", \"a\", \"b\"); }").contains("at most two"));
        assert!(bad("fn t() { verifies!(\"p.adoc#\", \"a\"); }").contains("empty #anchor"));
        assert!(bad("fn t() { verifies!(\"p.adoc\", \"  \"); }").contains("empty excerpt"));

        let err = bad("fn t() {\n\n  verifies!(\"p.adoc\", 1);\n}");
        assert!(err.starts_with("html5/src/tests/lists.rs:3:"), "{err}");
    }

    #[test]
    fn parse_failures_name_the_file() {
        let err = scan_source("fn t( {", &site()).unwrap_err().to_string();
        assert!(
            err.starts_with("html5/src/tests/lists.rs: cannot parse"),
            "{err}"
        );
    }

    #[test]
    fn scans_a_root_with_crate_names() {
        let dir = std::env::temp_dir().join(format!("bokfell-claims-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("mycrate/src/tests")).unwrap();
        std::fs::write(
            dir.join("mycrate/Cargo.toml"),
            "[package]\nname = \"mycrate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("mycrate/src/tests/a.rs"),
            "#[test]\nfn t() { verifies!(\"docs/p.adoc\", \"excerpt\"); }\n",
        )
        .unwrap();
        std::fs::write(dir.join("mycrate/src/tests/notes.txt"), "ignored").unwrap();

        let claims = scan_test_root(&TestRoot {
            dir: dir.join("mycrate/src/tests"),
            repo_prefix: "mycrate/src/tests".to_string(),
            local: true,
            repo: Some("https://github.com/o/r".to_string()),
            rev: Some("abc".to_string()),
            scope: 1,
            krate: Some("fallback".to_string()),
        })
        .unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].site.file, "mycrate/src/tests/a.rs");
        assert_eq!(claims[0].site.krate.as_deref(), Some("mycrate"));
        assert_eq!(claims[0].site.line, 2);
        assert_eq!(claims[0].site.scope, 1);
        assert_eq!(claims[0].site.rev.as_deref(), Some("abc"));
        assert_eq!(
            claims[0].site.local_path.as_deref(),
            Some(dir.join("mycrate/src/tests/a.rs").as_path())
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn crate_fallback_applies_without_a_manifest() {
        let dir =
            std::env::temp_dir().join(format!("bokfell-claims-nocrate-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.rs"),
            "fn t() { verifies!(\"p.adoc\", \"x\"); }\n",
        )
        .unwrap();
        let claims = scan_test_root(&TestRoot {
            dir: dir.clone(),
            repo_prefix: "html5/src/tests".to_string(),
            local: false,
            repo: None,
            rev: None,
            scope: 0,
            krate: Some("asciidoc-html5".to_string()),
        })
        .unwrap();
        assert_eq!(claims[0].site.krate.as_deref(), Some("asciidoc-html5"));
        assert_eq!(
            manifest_package_name("[package]\nname = \"x\"\n").as_deref(),
            Some("x")
        );
        assert_eq!(manifest_package_name("[workspace]\nmembers = []\n"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
