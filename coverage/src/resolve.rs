//! Resolution: claims and sidecar entries → block states (RFC 0001 §3–§5).
//!
//! Every measured page's blocks start from the structural default; sidecar
//! entries and claims target blocks by page path plus excerpt (or section
//! anchor), and the precedence rules of RFC 0001 §3 turn the two into one
//! state per block. Anything that fails to resolve is a diagnostic with
//! provenance, never a silent skip.

use std::collections::{BTreeMap, HashMap};

use bokfell_model::{Coords, ResourceRef};
use serde::{Deserialize, Serialize};

use crate::{
    model::{
        BlockCoverage, BlockState, Claim, ClaimTarget, MeasuredPage, PageCoverage, PageRecord,
        StateCounts, StructuralKind,
    },
    sidecar::{Sidecar, SidecarEntry, SidecarKind},
    text::normalize_whitespace,
};

/// What kind of problem a diagnostic reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosticKind {
    /// A claim or sidecar path matches no measured page in its scope.
    UnknownPage,
    /// A path matches more than one page in its scope.
    AmbiguousPage,
    /// A `#anchor` names no section of any measured version.
    NoSection,
    /// An excerpt matches no block of any measured version.
    NoMatch,
    /// An excerpt matches several blocks within one version's page.
    Ambiguous,
    /// A claim targets a block the sidecar calls non-normative or out of
    /// scope.
    Contradiction,
    /// A block is targeted by more than one sidecar list.
    SidecarConflict,
    /// A claim resolves to a `planned` block: the entry is stale.
    StalePlanned,
    /// The structural heuristic disagrees with a sidecar entry or claim.
    HeuristicDisagreement,
    /// A block nobody has classified.
    Unclassified,
}

impl DiagnosticKind {
    /// Whether diagnostics of this kind are hard errors (as opposed to
    /// lint findings).
    pub fn is_error(self) -> bool {
        matches!(
            self,
            DiagnosticKind::UnknownPage
                | DiagnosticKind::AmbiguousPage
                | DiagnosticKind::NoSection
                | DiagnosticKind::NoMatch
                | DiagnosticKind::Ambiguous
                | DiagnosticKind::Contradiction
                | DiagnosticKind::SidecarConflict
        )
    }
}

/// One resolution problem, with provenance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The kind of problem.
    pub kind: DiagnosticKind,
    /// Where it was found: `file:line` of a claim, or a sidecar file, or
    /// a page.
    pub at: String,
    /// What is wrong.
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.at, self.message)
    }
}

/// The coverage database: every measured page's resolved coverage, the
/// claims they reference, and the diagnostics resolution produced.
/// Ephemeral — recomputed by every scan, never committed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CoverageDatabase {
    /// Every measured page, in catalog order.
    pub pages: Vec<PageRecord>,
    /// Every scanned claim; block records index into this list.
    pub claims: Vec<Claim>,
    /// Hard errors: unresolved or ambiguous targets, contradictions.
    pub errors: Vec<Diagnostic>,
    /// Review findings (`bokfell coverage lint`).
    pub lint: Vec<Diagnostic>,
}

impl CoverageDatabase {
    /// The record of one page, by coordinates.
    pub fn page(&self, coords: &Coords) -> Option<&PageRecord> {
        self.pages.iter().find(|page| &page.coords == coords)
    }

    /// Per-component rollups, keyed by `(component, version)` in page
    /// order, plus the site total.
    pub fn rollups(&self) -> (Vec<ComponentRollup>, StateCounts) {
        let mut components: Vec<ComponentRollup> = Vec::new();
        let mut total = StateCounts::default();
        for page in &self.pages {
            let key = (page.coords.component.clone(), page.coords.version.clone());
            match components.iter_mut().find(|(k, _)| *k == key) {
                Some((_, counts)) => counts.add_counts(&page.coverage.counts),
                None => components.push((key, page.coverage.counts)),
            }
            total.add_counts(&page.coverage.counts);
        }
        (components, total)
    }

    /// Serializes the database as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("database serializes")
    }

    /// Reads a database written by [`to_json`](Self::to_json).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
}

/// One component version's rolled-up counts: `((component, version),
/// counts)`.
pub type ComponentRollup = ((String, Option<String>), StateCounts);

/// How a target (`#anchor` plus optional excerpt) resolved against one
/// page version.
#[derive(Debug, PartialEq)]
enum TargetHit {
    /// The blocks the target names.
    Blocks(Vec<usize>),
    /// The anchor names no section of the page.
    NoSection,
    /// The excerpt matches no block (within the anchor's section).
    NoMatch,
    /// The excerpt matches several blocks.
    Ambiguous(Vec<usize>),
}

/// Resolves a target against one page: the anchor scopes the candidate
/// blocks (or selects them all when there is no excerpt), and the
/// excerpt must match exactly one of them.
fn resolve_target(
    page: &MeasuredPage,
    normalized: &[String],
    anchor: Option<&str>,
    excerpt: Option<&str>,
) -> TargetHit {
    if let Some(anchor) = anchor {
        if !page.section_ids.iter().any(|id| id == anchor) {
            return TargetHit::NoSection;
        }
    }
    let candidates = page
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| anchor.is_none_or(|a| block.sections.iter().any(|s| s == a)))
        .map(|(index, _)| index);

    match excerpt {
        None => TargetHit::Blocks(candidates.collect()),
        Some(excerpt) => {
            let needle = normalize_whitespace(excerpt);
            let hits: Vec<usize> = candidates
                .filter(|&index| !needle.is_empty() && normalized[index].contains(&needle))
                .collect();
            match hits.len() {
                0 => TargetHit::NoMatch,
                1 => TargetHit::Blocks(hits),
                _ => TargetHit::Ambiguous(hits),
            }
        }
    }
}

/// Whether a claim path names a repository path: equal, or a suffix on a
/// `/` boundary.
pub fn path_matches(repo_path: &str, claim_path: &str) -> bool {
    let claim_path = claim_path.trim_start_matches("./").trim_start_matches('/');
    repo_path == claim_path
        || repo_path
            .strip_suffix(claim_path)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

/// The pages a path-form target names within `scope`, grouped so that
/// versions of one page stay together: `Err` names the distinct pages
/// when more than one matches.
fn pages_for_path<'a>(
    pages: &'a [MeasuredPage],
    scope: usize,
    path: &str,
) -> Result<Vec<&'a MeasuredPage>, Vec<String>> {
    let hits: Vec<&MeasuredPage> = pages
        .iter()
        .filter(|page| page.scope == scope && path_matches(&page.repo_path, path))
        .collect();
    let mut identities: Vec<String> = hits
        .iter()
        .map(|page| {
            format!(
                "{}:{}:{}",
                page.coords.component, page.coords.module, page.coords.path
            )
        })
        .collect();
    identities.sort();
    identities.dedup();
    if identities.len() > 1 {
        Err(identities)
    } else {
        Ok(hits)
    }
}

/// The pages a resource-ID target names, across every scope: every
/// measured version of that page (or the one version named).
fn pages_for_resource_id<'a>(pages: &'a [MeasuredPage], id: &str) -> Vec<&'a MeasuredPage> {
    let Some(reference) = ResourceRef::parse(id) else {
        return Vec::new();
    };
    let Some(component) = reference.component else {
        return Vec::new();
    };
    let module = reference.module.unwrap_or_else(|| "ROOT".to_string());
    pages
        .iter()
        .filter(|page| {
            page.coords.component == component
                && page.coords.module == module
                && page.coords.path == reference.path
                && reference
                    .version
                    .as_ref()
                    .is_none_or(|v| page.coords.version.as_ref() == Some(v))
        })
        .collect()
}

/// A sidecar entry applied to a block.
#[derive(Clone, Copy)]
struct EntryHit {
    sidecar: usize,
    entry: usize,
}

/// A claim applied to a block.
#[derive(Clone, Copy)]
struct ClaimHit {
    claim: usize,
    whole_section: bool,
}

/// Resolves claims and sidecars against the measured pages into a
/// coverage database.
pub fn resolve(
    pages: &[MeasuredPage],
    claims: Vec<Claim>,
    sidecars: &[Sidecar],
) -> CoverageDatabase {
    let normalized: Vec<Vec<String>> = pages
        .iter()
        .map(|page| {
            page.blocks
                .iter()
                .map(|block| normalize_whitespace(&block.text))
                .collect()
        })
        .collect();

    let mut errors = Vec::new();
    let mut lint = Vec::new();

    // Per page: which sidecar entry classifies each block, and whether
    // any sidecar marks the page reviewed.
    let mut entry_hits: Vec<Vec<Option<EntryHit>>> = pages
        .iter()
        .map(|page| vec![None; page.blocks.len()])
        .collect();
    let mut reviewed = vec![false; pages.len()];

    for (sidecar_index, sidecar) in sidecars.iter().enumerate() {
        let targets = match pages_for_path(pages, sidecar.scope, &sidecar.spec_path) {
            Ok(targets) => targets,
            Err(identities) => {
                errors.push(Diagnostic {
                    kind: DiagnosticKind::AmbiguousPage,
                    at: sidecar.file.clone(),
                    message: format!(
                        "spec path {:?} matches several pages: {}",
                        sidecar.spec_path,
                        identities.join(", ")
                    ),
                });
                continue;
            }
        };
        if targets.is_empty() {
            errors.push(Diagnostic {
                kind: DiagnosticKind::UnknownPage,
                at: sidecar.file.clone(),
                message: format!("spec path {:?} matches no measured page", sidecar.spec_path),
            });
            continue;
        }

        // `reviewed = true` is per page and needs no entries.
        if sidecar.reviewed {
            for page in &targets {
                reviewed[index_of(pages, page)] = true;
            }
        }

        for (entry_index, entry) in sidecar.entries.iter().enumerate() {
            let mut matched_any = false;
            let mut no_section_everywhere = true;
            for page in &targets {
                let page_index = index_of(pages, page);
                let hit = resolve_target(
                    page,
                    &normalized[page_index],
                    entry.section.as_deref(),
                    entry.excerpt.as_deref(),
                );
                match hit {
                    TargetHit::Blocks(blocks) => {
                        matched_any = true;
                        no_section_everywhere = false;
                        for block in blocks {
                            apply_entry(
                                &mut entry_hits[page_index][block],
                                EntryHit {
                                    sidecar: sidecar_index,
                                    entry: entry_index,
                                },
                                sidecars,
                                page,
                                block,
                                &mut errors,
                            );
                        }
                    }
                    TargetHit::NoSection => {}
                    TargetHit::NoMatch => no_section_everywhere = false,
                    TargetHit::Ambiguous(blocks) => {
                        matched_any = true;
                        no_section_everywhere = false;
                        errors.push(ambiguous(&sidecar.file, &entry.display(), page, &blocks));
                    }
                }
            }
            if !matched_any {
                errors.push(unmatched(
                    &sidecar.file,
                    &entry.display(),
                    entry.section.as_deref(),
                    no_section_everywhere,
                    &targets,
                ));
            }
        }
    }

    // Per page: the claims resolving to each block.
    let mut claim_hits: Vec<Vec<Vec<ClaimHit>>> = pages
        .iter()
        .map(|page| vec![Vec::new(); page.blocks.len()])
        .collect();

    for (claim_index, claim) in claims.iter().enumerate() {
        let at = format!("{}:{}", claim.site.file, claim.site.line);
        let targets = match &claim.target {
            ClaimTarget::Path(path) => match pages_for_path(pages, claim.site.scope, path) {
                Ok(targets) => targets,
                Err(identities) => {
                    errors.push(Diagnostic {
                        kind: DiagnosticKind::AmbiguousPage,
                        at,
                        message: format!(
                            "path {path:?} matches several pages: {}",
                            identities.join(", ")
                        ),
                    });
                    continue;
                }
            },
            ClaimTarget::ResourceId(id) => pages_for_resource_id(pages, id),
        };
        if targets.is_empty() {
            errors.push(Diagnostic {
                kind: DiagnosticKind::UnknownPage,
                at,
                message: format!("{:?} matches no measured page", claim.target_display()),
            });
            continue;
        }

        let mut matched_any = false;
        let mut no_section_everywhere = true;
        for page in &targets {
            let page_index = index_of(pages, page);
            let hit = resolve_target(
                page,
                &normalized[page_index],
                claim.anchor.as_deref(),
                claim.excerpt.as_deref(),
            );
            match hit {
                TargetHit::Blocks(blocks) => {
                    matched_any = true;
                    no_section_everywhere = false;
                    for block in blocks {
                        claim_hits[page_index][block].push(ClaimHit {
                            claim: claim_index,
                            whole_section: claim.excerpt.is_none(),
                        });
                    }
                }
                TargetHit::NoSection => {}
                TargetHit::NoMatch => no_section_everywhere = false,
                TargetHit::Ambiguous(blocks) => {
                    matched_any = true;
                    no_section_everywhere = false;
                    errors.push(ambiguous(&at, &claim.target_display(), page, &blocks));
                }
            }
        }
        if !matched_any {
            errors.push(unmatched(
                &at,
                &claim.target_display(),
                claim.anchor.as_deref(),
                no_section_everywhere,
                &targets,
            ));
        }
    }

    // Precedence (RFC 0001 §3): one state per block.
    let mut records = Vec::new();
    for (page_index, page) in pages.iter().enumerate() {
        let mut blocks = Vec::new();
        for (block_index, block) in page.blocks.iter().enumerate() {
            let entry = entry_hits[page_index][block_index].map(|hit| {
                (
                    &sidecars[hit.sidecar],
                    &sidecars[hit.sidecar].entries[hit.entry],
                )
            });
            let hits = &claim_hits[page_index][block_index];
            let location = format!("{} block {}", page.url, block_index + 1);
            let excerpt = short_excerpt(&normalized[page_index][block_index]);

            // A whole-section claim covers the section's normative blocks
            // only; it never contradicts a sidecar entry or a structural
            // default inside the section.
            let sidecar_state = entry.map(|(_, e)| e.kind.state());
            let excluded = matches!(
                sidecar_state,
                Some(BlockState::NonNormative | BlockState::OutOfScope)
            ) || (sidecar_state.is_none()
                && block.kind == StructuralKind::NonNormative);
            let mut claim_indexes: Vec<usize> = hits
                .iter()
                .filter(|hit| !(hit.whole_section && excluded))
                .map(|hit| hit.claim)
                .collect();
            claim_indexes.sort_unstable();
            claim_indexes.dedup();

            let state = if claim_indexes.is_empty() {
                match (entry, block.kind, reviewed[page_index]) {
                    (Some((_, e)), _, _) => e.kind.state(),
                    (None, StructuralKind::NonNormative, _) => BlockState::NonNormative,
                    (None, StructuralKind::Prose, true) => BlockState::Uncovered,
                    (None, StructuralKind::Prose, false) => BlockState::Unclassified,
                }
            } else {
                match entry {
                    Some((sidecar, e))
                        if matches!(
                            e.kind,
                            SidecarKind::NonNormative | SidecarKind::OutOfScope
                        ) =>
                    {
                        for &claim in &claim_indexes {
                            let site = &claims[claim].site;
                            errors.push(Diagnostic {
                                kind: DiagnosticKind::Contradiction,
                                at: format!("{}:{}", site.file, site.line),
                                message: format!(
                                    "claim {:?} verifies a block {} marks {}: {} ({location}: \
                                     {excerpt:?})",
                                    claims[claim].target_display(),
                                    sidecar.file,
                                    e.kind.list_name(),
                                    e.display()
                                ),
                            });
                        }
                    }
                    Some((sidecar, e)) => {
                        // A claim on a planned block: the work shipped.
                        lint.push(Diagnostic {
                            kind: DiagnosticKind::StalePlanned,
                            at: sidecar.file.clone(),
                            message: format!(
                                "{} is verified by {}; remove the stale planned entry \
                                 ({location}: {excerpt:?})",
                                e.display(),
                                claim_labels(&claims, &claim_indexes)
                            ),
                        });
                    }
                    None if block.kind == StructuralKind::NonNormative => {
                        lint.push(Diagnostic {
                            kind: DiagnosticKind::HeuristicDisagreement,
                            at: claim_site(&claims, claim_indexes[0]),
                            message: format!(
                                "claim verifies a {} block the heuristic reads as \
                                 non-normative ({location}: {excerpt:?})",
                                block.context
                            ),
                        });
                    }
                    None => {}
                }
                BlockState::Verified
            };

            if state == BlockState::Unclassified {
                lint.push(Diagnostic {
                    kind: DiagnosticKind::Unclassified,
                    at: location.clone(),
                    message: format!("unclassified {} {excerpt:?}", block.context),
                });
            }
            if let Some((sidecar, e)) = entry {
                if e.kind == SidecarKind::NonNormative && block.kind == StructuralKind::Prose {
                    lint.push(Diagnostic {
                        kind: DiagnosticKind::HeuristicDisagreement,
                        at: sidecar.file.clone(),
                        message: format!(
                            "{} marks a {} block the heuristic reads as normative prose — \
                             non-normative, or out of scope? ({location}: {excerpt:?})",
                            e.display(),
                            block.context
                        ),
                    });
                }
            }

            let (reason, tracking) = match (state, entry) {
                (BlockState::OutOfScope, Some((_, e))) => (e.reason.clone(), None),
                (BlockState::Planned, Some((_, e))) => (None, e.tracking.clone()),
                _ => (None, None),
            };
            blocks.push(BlockCoverage {
                state,
                context: block.context.clone(),
                page_lines: block.page_lines,
                reason,
                tracking,
                claims: claim_indexes,
            });
        }
        records.push(PageRecord {
            coords: page.coords.clone(),
            url: page.url.clone(),
            repo_path: page.repo_path.clone(),
            coverage: PageCoverage::from_blocks(blocks),
        });
    }

    CoverageDatabase {
        pages: records,
        claims,
        errors,
        lint,
    }
}

/// Records a sidecar entry on a block, or the conflict when a different
/// list already targets it.
fn apply_entry(
    slot: &mut Option<EntryHit>,
    hit: EntryHit,
    sidecars: &[Sidecar],
    page: &MeasuredPage,
    block: usize,
    errors: &mut Vec<Diagnostic>,
) {
    let entry = |hit: EntryHit| -> &SidecarEntry { &sidecars[hit.sidecar].entries[hit.entry] };
    match slot {
        None => *slot = Some(hit),
        Some(existing) if entry(*existing).kind == entry(hit).kind => {}
        Some(existing) => errors.push(Diagnostic {
            kind: DiagnosticKind::SidecarConflict,
            at: sidecars[hit.sidecar].file.clone(),
            message: format!(
                "{} and {} both target {} block {} ({:?})",
                entry(*existing).display(),
                entry(hit).display(),
                page.url,
                block + 1,
                short_excerpt(&normalize_whitespace(&page.blocks[block].text))
            ),
        }),
    }
}

fn ambiguous(at: &str, target: &str, page: &MeasuredPage, blocks: &[usize]) -> Diagnostic {
    let listed: Vec<String> = blocks
        .iter()
        .map(|&b| {
            format!(
                "block {} {:?}",
                b + 1,
                short_excerpt(&normalize_whitespace(&page.blocks[b].text))
            )
        })
        .collect();
    Diagnostic {
        kind: DiagnosticKind::Ambiguous,
        at: at.to_string(),
        message: format!(
            "{target:?} matches {} blocks of {}: {} — quote a longer excerpt or add a #anchor",
            blocks.len(),
            page.url,
            listed.join("; ")
        ),
    }
}

fn unmatched(
    at: &str,
    target: &str,
    anchor: Option<&str>,
    no_section: bool,
    targets: &[&MeasuredPage],
) -> Diagnostic {
    let versions = targets.len();
    if no_section {
        Diagnostic {
            kind: DiagnosticKind::NoSection,
            at: at.to_string(),
            message: format!(
                "{target:?}: no section {:?} in any of the {versions} measured version(s) of {}",
                anchor.unwrap_or(""),
                targets[0].url
            ),
        }
    } else {
        Diagnostic {
            kind: DiagnosticKind::NoMatch,
            at: at.to_string(),
            message: format!(
                "{target:?}: excerpt matches no block in any of the {versions} measured \
                 version(s) of {} (the spec text may have changed)",
                targets[0].url
            ),
        }
    }
}

fn index_of(pages: &[MeasuredPage], page: &MeasuredPage) -> usize {
    pages
        .iter()
        .position(|p| std::ptr::eq(p, page))
        .expect("page comes from the slice")
}

fn claim_site(claims: &[Claim], index: usize) -> String {
    format!("{}:{}", claims[index].site.file, claims[index].site.line)
}

fn claim_labels(claims: &[Claim], indexes: &[usize]) -> String {
    indexes
        .iter()
        .map(|&i| claims[i].label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first few words of a block, for messages.
fn short_excerpt(normalized: &str) -> String {
    const MAX: usize = 60;
    if normalized.chars().count() <= MAX {
        return normalized.to_string();
    }
    let cut: String = normalized.chars().take(MAX).collect();
    match cut.rfind(' ') {
        Some(space) if space > MAX / 2 => format!("{}…", &cut[..space]),
        _ => format!("{cut}…"),
    }
}

/// Groups the database's pages by `(component, version)`, preserving page
/// order — the dashboard's and table report's grouping.
pub fn pages_by_component(db: &CoverageDatabase) -> BTreeMap<(String, String), Vec<&PageRecord>> {
    let mut groups: BTreeMap<(String, String), Vec<&PageRecord>> = BTreeMap::new();
    for page in &db.pages {
        groups
            .entry((
                page.coords.component.clone(),
                page.coords.version.clone().unwrap_or_default(),
            ))
            .or_default()
            .push(page);
    }
    groups
}

/// Every claim's index keyed by its site, for callers that need to look
/// claims up by provenance.
pub fn claims_by_site(db: &CoverageDatabase) -> HashMap<(String, u32), usize> {
    db.claims
        .iter()
        .enumerate()
        .map(|(i, c)| ((c.site.file.clone(), c.site.line), i))
        .collect()
}

#[cfg(test)]
mod tests {
    use bokfell_model::Family;

    use super::*;
    use crate::model::{ClaimSite, SpecBlock};

    fn block(text: &str, kind: StructuralKind, sections: &[&str], line: u32) -> SpecBlock {
        SpecBlock {
            context: if kind == StructuralKind::Prose {
                "paragraph".into()
            } else {
                "listing".into()
            },
            text: text.to_string(),
            start_line: line,
            line_count: 2,
            page_lines: Some((line, 2)),
            sections: sections.iter().map(|s| s.to_string()).collect(),
            kind,
        }
    }

    fn page(version: Option<&str>, scope: usize, blocks: Vec<SpecBlock>) -> MeasuredPage {
        MeasuredPage {
            coords: Coords {
                component: "spec".into(),
                version: version.map(str::to_string),
                module: "lists".into(),
                family: Family::Page,
                path: "ordered.adoc".into(),
            },
            url: match version {
                Some(v) => format!("spec/{v}/lists/ordered.html"),
                None => "spec/lists/ordered.html".into(),
            },
            repo_path: "docs/modules/lists/pages/ordered.adoc".into(),
            scope,
            section_ids: blocks
                .iter()
                .flat_map(|b| b.sections.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            blocks,
        }
    }

    fn sample_page(version: Option<&str>) -> MeasuredPage {
        page(
            version,
            0,
            vec![
                block("Intro prose about lists.", StructuralKind::Prose, &[], 3),
                block(
                    "To nest an ordered list, add a marker character for each level\nof nesting.",
                    StructuralKind::Prose,
                    &["_nesting"],
                    7,
                ),
                block(
                    "....\n. one\n.. two\n....",
                    StructuralKind::NonNormative,
                    &["_nesting"],
                    10,
                ),
                block(
                    "Asciidoctor also supports the DocBook converter emits things.",
                    StructuralKind::Prose,
                    &["_notes"],
                    15,
                ),
                block(
                    "Footnotes may be defined once and reused.",
                    StructuralKind::Prose,
                    &["_notes"],
                    18,
                ),
            ],
        )
    }

    fn claim(target: &str, excerpt: Option<&str>, line: u32) -> Claim {
        let (page, anchor) = match target.split_once('#') {
            Some((p, a)) => (p.to_string(), Some(a.to_string())),
            None => (target.to_string(), None),
        };
        Claim {
            target: if page.contains(':') {
                ClaimTarget::ResourceId(page)
            } else {
                ClaimTarget::Path(page)
            },
            anchor,
            excerpt: excerpt.map(str::to_string),
            site: ClaimSite {
                file: "parser/src/tests/lists.rs".into(),
                local_path: None,
                line,
                test_fn: Some("t".into()),
                krate: Some("parser".into()),
                repo: None,
                rev: None,
                scope: 0,
            },
        }
    }

    fn sidecar(text: &str) -> Sidecar {
        Sidecar::parse(
            text,
            "spec-map/docs/modules/lists/pages/ordered.adoc.toml",
            "docs/modules/lists/pages/ordered.adoc",
            0,
        )
        .unwrap()
    }

    fn states(db: &CoverageDatabase, page: usize) -> Vec<BlockState> {
        db.pages[page]
            .coverage
            .blocks
            .iter()
            .map(|b| b.state)
            .collect()
    }

    #[test]
    fn structural_defaults_without_a_sidecar() {
        let db = resolve(&[sample_page(None)], Vec::new(), &[]);
        assert!(db.errors.is_empty(), "{:?}", db.errors);
        assert_eq!(
            states(&db, 0),
            vec![
                BlockState::Unclassified,
                BlockState::Unclassified,
                BlockState::NonNormative,
                BlockState::Unclassified,
                BlockState::Unclassified,
            ]
        );
        assert_eq!(db.pages[0].coverage.percent_verified(), 0);
        assert_eq!(
            db.lint
                .iter()
                .filter(|d| d.kind == DiagnosticKind::Unclassified)
                .count(),
            4
        );
    }

    #[test]
    fn reviewed_sidecar_and_claims_resolve_to_states() {
        let sidecar = sidecar(
            r#"
reviewed = true
[[non-normative]]
excerpt = "Intro prose"
[[out-of-scope]]
excerpt = "the DocBook converter emits"
reason = "HTML5 only"
[[planned]]
excerpt = "Footnotes may be defined once"
tracking = "o/r#1"
"#,
        );
        let claims = vec![claim(
            "lists/pages/ordered.adoc",
            Some("add a marker character for each level of nesting"),
            12,
        )];
        let db = resolve(&[sample_page(None)], claims, &[sidecar]);
        assert!(db.errors.is_empty(), "{:?}", db.errors);
        assert_eq!(
            states(&db, 0),
            vec![
                BlockState::NonNormative,
                BlockState::Verified,
                BlockState::NonNormative,
                BlockState::OutOfScope,
                BlockState::Planned,
            ]
        );
        let blocks = &db.pages[0].coverage.blocks;
        assert_eq!(blocks[1].claims, vec![0]);
        assert_eq!(blocks[3].reason.as_deref(), Some("HTML5 only"));
        assert_eq!(
            blocks[4].tracking.as_ref().unwrap().url,
            "https://github.com/o/r/issues/1"
        );

        // verified / (verified + planned + uncovered + unclassified) = 1/2.
        assert_eq!(db.pages[0].coverage.counts.denominator(), 2);
        assert_eq!(db.pages[0].coverage.percent_verified(), 50);

        // The non-normative entry on prose is a heuristic disagreement.
        assert!(db
            .lint
            .iter()
            .any(|d| d.kind == DiagnosticKind::HeuristicDisagreement));
    }

    #[test]
    fn whole_section_claims_skip_excluded_blocks() {
        let claims = vec![claim(
            "docs/modules/lists/pages/ordered.adoc#_nesting",
            None,
            1,
        )];
        let db = resolve(&[sample_page(None)], claims, &[]);
        assert!(db.errors.is_empty(), "{:?}", db.errors);
        let states = states(&db, 0);
        assert_eq!(states[1], BlockState::Verified);
        assert_eq!(states[2], BlockState::NonNormative);
        assert!(db.pages[0].coverage.blocks[2].claims.is_empty());
    }

    #[test]
    fn contradictions_stale_planned_and_conflicts() {
        let sidecar = sidecar(
            r#"
[[non-normative]]
excerpt = "Intro prose"
[[planned]]
excerpt = "Footnotes may be defined once"
tracking = "o/r#1"
"#,
        );
        let claims = vec![
            claim("ordered.adoc", Some("Intro prose"), 5),
            claim("ordered.adoc", Some("Footnotes may be defined"), 9),
        ];
        let db = resolve(&[sample_page(None)], claims, &[sidecar]);
        assert_eq!(db.errors.len(), 1, "{:?}", db.errors);
        assert_eq!(db.errors[0].kind, DiagnosticKind::Contradiction);
        assert_eq!(db.errors[0].at, "parser/src/tests/lists.rs:5");
        assert!(db.errors[0].message.contains("ordered.adoc.toml"));

        // The planned block is verified now and the entry flagged stale.
        assert_eq!(states(&db, 0)[4], BlockState::Verified);
        assert!(db
            .lint
            .iter()
            .any(|d| d.kind == DiagnosticKind::StalePlanned));

        let conflicting = super::super::sidecar::Sidecar::parse(
            "[[non-normative]]\nsection = \"_notes\"\n[[planned]]\nexcerpt = \"Footnotes\"\ntracking = \"o/r#2\"\n",
            "spec-map/ordered.adoc.toml",
            "ordered.adoc",
            0,
        )
        .unwrap();
        let db = resolve(&[sample_page(None)], Vec::new(), &[conflicting]);
        assert!(
            db.errors
                .iter()
                .any(|d| d.kind == DiagnosticKind::SidecarConflict),
            "{:?}",
            db.errors
        );
    }

    #[test]
    fn unresolved_and_ambiguous_targets_are_errors() {
        let claims = vec![
            claim("nowhere.adoc", Some("x"), 1),
            claim("ordered.adoc", Some("text that drifted"), 2),
            claim("ordered.adoc#_missing", Some("Intro"), 3),
            // "of nesting" matches two blocks? No — make one that does:
            claim("ordered.adoc", Some("a"), 4),
        ];
        let db = resolve(&[sample_page(None)], claims, &[]);
        let kinds: Vec<DiagnosticKind> = db.errors.iter().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DiagnosticKind::UnknownPage,
                DiagnosticKind::NoMatch,
                DiagnosticKind::NoSection,
                DiagnosticKind::Ambiguous,
            ],
            "{:?}",
            db.errors
        );
        assert_eq!(db.errors[1].at, "parser/src/tests/lists.rs:2");
        assert!(db.errors[3].message.contains("longer excerpt"));
    }

    #[test]
    fn claims_resolve_per_version_and_drift_is_not_an_error() {
        let mut old = sample_page(Some("1.0"));
        old.blocks[1].text = "Old wording of the nesting rule.".into();
        let new = sample_page(Some("2.0"));
        let claims = vec![
            claim("ordered.adoc", Some("add a marker character"), 1),
            claim("ordered.adoc", Some("Intro prose"), 2),
        ];
        let db = resolve(&[old, new], claims, &[]);
        assert!(db.errors.is_empty(), "{:?}", db.errors);
        assert_eq!(states(&db, 0)[1], BlockState::Unclassified);
        assert_eq!(states(&db, 1)[1], BlockState::Verified);
        assert_eq!(states(&db, 0)[0], BlockState::Verified);
        assert_eq!(states(&db, 1)[0], BlockState::Verified);
    }

    #[test]
    fn scopes_and_resource_ids() {
        let mut other = sample_page(None);
        other.scope = 1;
        other.coords.component = "other".into();
        other.url = "other/lists/ordered.html".into();
        let pages = [sample_page(None), other];

        // A path claim from scope 0 never reaches scope 1's identical
        // path; a resource ID does.
        let mut cross = claim("other:lists:ordered.adoc", Some("Intro prose"), 2);
        cross.site.scope = 0;
        let claims = vec![claim("ordered.adoc", Some("Intro prose"), 1), cross];
        let db = resolve(&pages, claims, &[]);
        assert!(db.errors.is_empty(), "{:?}", db.errors);
        assert_eq!(db.pages[0].coverage.blocks[0].claims, vec![0]);
        assert_eq!(db.pages[1].coverage.blocks[0].claims, vec![1]);

        // Two distinct pages under one scope matching one suffix is
        // ambiguous.
        let mut twin = sample_page(None);
        twin.coords.module = "other".into();
        twin.repo_path = "docs/modules/other/pages/ordered.adoc".into();
        let db = resolve(
            &[sample_page(None), twin],
            vec![claim("pages/ordered.adoc", Some("Intro prose"), 1)],
            &[],
        );
        assert_eq!(db.errors[0].kind, DiagnosticKind::AmbiguousPage);
    }

    #[test]
    fn path_suffix_matching_respects_boundaries() {
        assert!(path_matches(
            "docs/modules/ROOT/pages/x.adoc",
            "pages/x.adoc"
        ));
        assert!(path_matches(
            "docs/modules/ROOT/pages/x.adoc",
            "docs/modules/ROOT/pages/x.adoc"
        ));
        assert!(path_matches(
            "docs/modules/ROOT/pages/x.adoc",
            "./docs/modules/ROOT/pages/x.adoc"
        ));
        assert!(!path_matches("docs/modules/ROOT/pages/x.adoc", "s/x.adoc"));
        assert!(!path_matches("docs/modules/ROOT/pages/x.adoc", "y.adoc"));
    }

    #[test]
    fn rollups_and_json_round_trip() {
        let db = resolve(
            &[sample_page(Some("1.0")), sample_page(Some("2.0"))],
            vec![claim("ordered.adoc", Some("Intro prose"), 1)],
            &[],
        );
        let (components, total) = db.rollups();
        assert_eq!(components.len(), 2);
        assert_eq!(total.verified, 2);
        assert_eq!(total.unclassified, 6);
        assert_eq!(total.non_normative, 2);
        assert_eq!(total.percent_verified(), 25);

        let json = db.to_json();
        let back = CoverageDatabase::from_json(&json).unwrap();
        assert_eq!(back.pages.len(), 2);
        assert_eq!(back.claims.len(), 1);
        assert_eq!(
            back.page(&db.pages[1].coords).unwrap().coverage,
            db.pages[1].coverage
        );
    }
}
