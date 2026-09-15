//! The coverage map: per-page classification sidecars (RFC 0001 §5).
//!
//! A sidecar is a TOML file in the implementation repository, one per
//! measured spec page, laid out under one root mirroring the spec path
//! (`spec-map/<path-as-claims-write-it>.toml`). It records what is
//! non-normative, what is deliberately out of scope, and what is planned —
//! targeting blocks by the same excerpt grammar claims use.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::model::{BlockState, Tracking};

/// Errors from reading coverage-map sidecars.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// A sidecar (or the spec-map directory) could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// The path being read.
        path: String,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A sidecar is not valid TOML or violates the schema.
    #[error("{file}: {message}")]
    Invalid {
        /// The sidecar file.
        file: String,
        /// What was wrong.
        message: String,
    },
}

/// Which sidecar list an entry belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SidecarKind {
    /// `[[non-normative]]`
    NonNormative,
    /// `[[out-of-scope]]`
    OutOfScope,
    /// `[[planned]]`
    Planned,
}

impl SidecarKind {
    /// The list's TOML table name.
    pub fn list_name(self) -> &'static str {
        match self {
            SidecarKind::NonNormative => "non-normative",
            SidecarKind::OutOfScope => "out-of-scope",
            SidecarKind::Planned => "planned",
        }
    }

    /// The block state an entry of this list assigns.
    pub fn state(self) -> BlockState {
        match self {
            SidecarKind::NonNormative => BlockState::NonNormative,
            SidecarKind::OutOfScope => BlockState::OutOfScope,
            SidecarKind::Planned => BlockState::Planned,
        }
    }
}

/// One sidecar entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SidecarEntry {
    /// The list the entry is in.
    pub kind: SidecarKind,
    /// The section anchor: the whole section when `excerpt` is `None`,
    /// else the scope the excerpt resolves within.
    pub section: Option<String>,
    /// The block excerpt.
    pub excerpt: Option<String>,
    /// The reason: required on `out-of-scope`; optional on
    /// `non-normative`, where it records why prose carries no rule and
    /// closes the heuristic's question in `bokfell coverage lint`.
    pub reason: Option<String>,
    /// The tracking link (required on `planned`).
    pub tracking: Option<Tracking>,
}

impl SidecarEntry {
    /// The entry as written: `[[list]] section = …` / `excerpt = …`.
    pub fn display(&self) -> String {
        let mut out = format!("[[{}]]", self.kind.list_name());
        if let Some(section) = &self.section {
            out.push_str(&format!(" section = {section:?}"));
        }
        if let Some(excerpt) = &self.excerpt {
            out.push_str(&format!(" excerpt = {excerpt:?}"));
        }
        out
    }
}

/// One page's coverage map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    /// The sidecar file, for messages.
    pub file: String,
    /// The spec page path the sidecar classifies (the file's spec-map
    /// relative path without `.toml`), resolved like a claim path.
    pub spec_path: String,
    /// The `coverage.scan` entry index the sidecar belongs to.
    pub scope: usize,
    /// Whether a human reviewed the page: prose no entry touches is
    /// normative (`uncovered` until claimed) rather than `unclassified`.
    pub reviewed: bool,
    /// The entries, in file order.
    pub entries: Vec<SidecarEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSidecar {
    #[serde(default)]
    reviewed: bool,
    #[serde(default, rename = "non-normative")]
    non_normative: Vec<RawEntry>,
    #[serde(default, rename = "out-of-scope")]
    out_of_scope: Vec<RawEntry>,
    #[serde(default)]
    planned: Vec<RawEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    section: Option<String>,
    excerpt: Option<String>,
    reason: Option<String>,
    tracking: Option<String>,
}

impl Sidecar {
    /// Parses one sidecar's TOML text.
    pub fn parse(
        text: &str,
        file: &str,
        spec_path: &str,
        scope: usize,
    ) -> Result<Self, SidecarError> {
        let invalid = |message: String| SidecarError::Invalid {
            file: file.to_string(),
            message,
        };
        let raw: RawSidecar = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;

        let mut entries = Vec::new();
        let lists = [
            (SidecarKind::NonNormative, raw.non_normative),
            (SidecarKind::OutOfScope, raw.out_of_scope),
            (SidecarKind::Planned, raw.planned),
        ];
        for (kind, list) in lists {
            for (index, entry) in list.into_iter().enumerate() {
                let at = format!("[[{}]] entry {}", kind.list_name(), index + 1);
                if entry.section.is_none() && entry.excerpt.is_none() {
                    return Err(invalid(format!("{at}: needs `section` or `excerpt`")));
                }
                if entry
                    .excerpt
                    .as_deref()
                    .is_some_and(|e| e.trim().is_empty())
                {
                    return Err(invalid(format!("{at}: `excerpt` is empty")));
                }
                if kind == SidecarKind::OutOfScope
                    && entry.reason.as_deref().unwrap_or("").trim().is_empty()
                {
                    return Err(invalid(format!(
                        "{at}: `reason` is required on out-of-scope"
                    )));
                }
                if kind == SidecarKind::Planned
                    && entry.tracking.as_deref().unwrap_or("").trim().is_empty()
                {
                    return Err(invalid(format!("{at}: `tracking` is required on planned")));
                }
                entries.push(SidecarEntry {
                    kind,
                    section: entry.section.map(|s| s.trim_start_matches('#').to_string()),
                    excerpt: entry.excerpt,
                    reason: entry.reason,
                    tracking: entry.tracking.as_deref().map(Tracking::parse),
                });
            }
        }

        Ok(Sidecar {
            file: file.to_string(),
            spec_path: spec_path.to_string(),
            scope,
            reviewed: raw.reviewed,
            entries,
        })
    }
}

/// Loads every `*.toml` sidecar under a spec-map root; each file's
/// root-relative path without the `.toml` suffix is its spec path.
/// `display_prefix` prefixes file names in messages (the root as the
/// user wrote it).
pub fn load_spec_map(
    root: &Path,
    display_prefix: &str,
    scope: usize,
) -> Result<Vec<Sidecar>, SidecarError> {
    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();

    let mut sidecars = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .expect("walked file is under the root")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        let Some(spec_path) = rel.strip_suffix(".toml") else {
            continue;
        };
        let file = if display_prefix.is_empty() {
            rel.clone()
        } else {
            format!("{}/{rel}", display_prefix.trim_end_matches('/'))
        };
        let text = std::fs::read_to_string(&path).map_err(|source| SidecarError::Io {
            path: path.display().to_string(),
            source,
        })?;
        sidecars.push(Sidecar::parse(&text, &file, spec_path, scope)?);
    }
    Ok(sidecars)
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<(), SidecarError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let entries = std::fs::read_dir(dir).map_err(|source| SidecarError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    for entry in entries {
        let path = entry
            .map_err(|source| SidecarError::Io {
                path: dir.display().to_string(),
                source,
            })?
            .path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.extension().is_some_and(|ext| ext == "toml") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
reviewed = true

[[non-normative]]
section = "_a_note_on_terminology"

[[non-normative]]
excerpt = "Asciidoctor also supports"

[[out-of-scope]]
excerpt = "the DocBook converter emits"
reason = "asciidoc-html5 targets HTML5 only; no DocBook backend planned"

[[planned]]
excerpt = "Footnotes may be defined once and reused"
tracking = "asciidoc-rs/asciidoc-html5#341"
"#;

    #[test]
    fn parses_the_rfc_example() {
        let sidecar = Sidecar::parse(SAMPLE, "spec-map/x.adoc.toml", "x.adoc", 0).unwrap();
        assert!(sidecar.reviewed);
        assert_eq!(sidecar.entries.len(), 4);
        assert_eq!(sidecar.entries[0].kind, SidecarKind::NonNormative);
        assert_eq!(
            sidecar.entries[0].section.as_deref(),
            Some("_a_note_on_terminology")
        );
        assert_eq!(sidecar.entries[2].kind, SidecarKind::OutOfScope);
        assert!(sidecar.entries[2].reason.is_some());
        let planned = &sidecar.entries[3];
        assert_eq!(planned.kind, SidecarKind::Planned);
        assert_eq!(
            planned.tracking.as_ref().unwrap().url,
            "https://github.com/asciidoc-rs/asciidoc-html5/issues/341"
        );
    }

    #[test]
    fn requires_reason_and_tracking() {
        let missing_reason = "[[out-of-scope]]\nexcerpt = \"x\"\n";
        let err = Sidecar::parse(missing_reason, "f.toml", "x.adoc", 0).unwrap_err();
        assert!(err.to_string().contains("`reason` is required"), "{err}");

        let missing_tracking = "[[planned]]\nexcerpt = \"x\"\n";
        let err = Sidecar::parse(missing_tracking, "f.toml", "x.adoc", 0).unwrap_err();
        assert!(err.to_string().contains("`tracking` is required"), "{err}");

        let no_target = "[[non-normative]]\nreason = \"x\"\n";
        let err = Sidecar::parse(no_target, "f.toml", "x.adoc", 0).unwrap_err();
        assert!(
            err.to_string().contains("needs `section` or `excerpt`"),
            "{err}"
        );

        let unknown_key = "[[non-normative]]\nexcerpt = \"x\"\nbogus = 1\n";
        assert!(Sidecar::parse(unknown_key, "f.toml", "x.adoc", 0).is_err());
    }

    #[test]
    fn tracking_references_template_to_github() {
        assert_eq!(
            Tracking::parse("asciidoc-rs/bokfell#12").url,
            "https://github.com/asciidoc-rs/bokfell/issues/12"
        );
        assert_eq!(
            Tracking::parse("https://example.org/t/1").url,
            "https://example.org/t/1"
        );
        assert_eq!(Tracking::parse("JIRA-42").url, "JIRA-42");
    }

    #[test]
    fn loads_a_spec_map_tree() {
        let dir = std::env::temp_dir().join(format!("bokfell-specmap-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("docs/modules/ROOT/pages")).unwrap();
        std::fs::write(
            dir.join("docs/modules/ROOT/pages/a.adoc.toml"),
            "reviewed = true\n",
        )
        .unwrap();
        std::fs::write(dir.join("README.md"), "not a sidecar\n").unwrap();

        let sidecars = load_spec_map(&dir, "spec-map", 3).unwrap();
        assert_eq!(sidecars.len(), 1);
        assert_eq!(sidecars[0].spec_path, "docs/modules/ROOT/pages/a.adoc");
        assert_eq!(
            sidecars[0].file,
            "spec-map/docs/modules/ROOT/pages/a.adoc.toml"
        );
        assert_eq!(sidecars[0].scope, 3);
        assert!(sidecars[0].reviewed);

        std::fs::remove_dir_all(&dir).ok();
    }
}
