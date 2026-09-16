//! Rollup reports (RFC 0001 §7): the table, the JSON database, and the
//! Codecov projection.

use std::collections::BTreeMap;

use crate::{
    model::{BlockState, StateCounts},
    resolve::CoverageDatabase,
};

/// Projects block states onto the pages' own source lines and emits the
/// interim tools' Codecov schema: every line of a `verified` block is
/// `1`; every line of a `planned`, `uncovered`, or `unclassified` block
/// is `0`; lines of `out-of-scope` and `non-normative` blocks, and lines
/// belonging to no block, are omitted. Keys are repository-relative page
/// paths; when several measured versions share a path, a verified line
/// wins.
pub fn codecov_json(db: &CoverageDatabase) -> String {
    let mut files: BTreeMap<&str, BTreeMap<u32, u8>> = BTreeMap::new();
    for page in &db.pages {
        let lines = files.entry(page.repo_path.as_str()).or_default();
        for block in &page.coverage.blocks {
            let Some((start, count)) = block.page_lines else {
                continue;
            };
            let hit = match block.state {
                BlockState::Verified => 1,
                BlockState::Planned | BlockState::Uncovered | BlockState::Unclassified => 0,
                BlockState::OutOfScope | BlockState::NonNormative => continue,
            };
            for line in start..start + count.max(1) {
                let slot = lines.entry(line).or_insert(hit);
                *slot = (*slot).max(hit);
            }
        }
    }

    let mut coverage = serde_json::Map::new();
    for (path, lines) in files {
        let map: serde_json::Map<String, serde_json::Value> = lines
            .into_iter()
            .map(|(line, hit)| (line.to_string(), serde_json::json!(hit)))
            .collect();
        coverage.insert(path.to_string(), serde_json::Value::Object(map));
    }
    let mut root = serde_json::Map::new();
    root.insert("coverage".to_string(), serde_json::Value::Object(coverage));
    serde_json::to_string_pretty(&serde_json::Value::Object(root)).expect("serializes")
}

/// The plain-text rollup table: one row per page grouped by component
/// version, a subtotal per component, and the site total. Out-of-scope
/// counts sit beside the percentage, never inside it.
pub fn table(db: &CoverageDatabase) -> String {
    const HEADER: [&str; 8] = [
        "PAGE",
        "VERIFIED",
        "PLANNED",
        "UNCOVERED",
        "UNCLASSIFIED",
        "COVERAGE",
        "OUT-OF-SCOPE",
        "NON-NORMATIVE",
    ];

    let mut rows: Vec<[String; 8]> = Vec::new();
    let groups = crate::resolve::pages_by_component(db);
    let (_, total) = db.rollups();
    for ((component, version), pages) in &groups {
        let label = if version.is_empty() {
            component.clone()
        } else {
            format!("{component} {version}")
        };
        rows.push([
            format!("[{label}]"),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ]);
        let mut subtotal = StateCounts::default();
        for page in pages {
            subtotal.add_counts(&page.coverage.counts);
            rows.push(row(&format!("  {}", page.url), &page.coverage.counts));
        }
        if pages.len() > 1 {
            rows.push(row(&format!("  = {label}"), &subtotal));
        }
    }
    if groups.len() > 1 {
        rows.push(row("= all pages", &total));
    }

    let mut widths: Vec<usize> = HEADER.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    let mut out = String::new();
    let emit = |out: &mut String, cells: &[String]| {
        for (i, cell) in cells.iter().enumerate() {
            if i > 0 {
                out.push_str("  ");
            }
            if i == 0 {
                out.push_str(&format!("{cell:<width$}", width = widths[i]));
            } else {
                out.push_str(&format!("{cell:>width$}", width = widths[i]));
            }
        }
        out.push('\n');
    };
    emit(
        &mut out,
        &HEADER.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
    );
    for row in &rows {
        emit(&mut out, row);
    }
    out
}

fn row(label: &str, counts: &StateCounts) -> [String; 8] {
    [
        label.to_string(),
        counts.verified.to_string(),
        counts.planned.to_string(),
        counts.uncovered.to_string(),
        counts.unclassified.to_string(),
        format!("{}%", counts.percent_verified()),
        counts.out_of_scope.to_string(),
        counts.non_normative.to_string(),
    ]
}

/// The lint report: every hard error, then every review finding, grouped
/// by kind. Empty when there is nothing to report.
pub fn lint(db: &CoverageDatabase) -> String {
    let mut out = String::new();
    if !db.errors.is_empty() {
        out.push_str(&format!("{} error(s):\n", db.errors.len()));
        for error in &db.errors {
            out.push_str(&format!("  error[{}]: {error}\n", error.kind.token()));
        }
    }
    let mut by_kind: BTreeMap<&str, Vec<&crate::resolve::Diagnostic>> = BTreeMap::new();
    for finding in &db.lint {
        by_kind
            .entry(finding.kind.token())
            .or_default()
            .push(finding);
    }
    for (kind, findings) in by_kind {
        out.push_str(&format!("{} {kind} finding(s):\n", findings.len()));
        for finding in findings {
            out.push_str(&format!("  {finding}\n"));
        }
    }
    out
}

impl crate::resolve::DiagnosticKind {
    /// The kind's token for reports (`no-match`, `unclassified`, …).
    pub fn token(self) -> &'static str {
        match self {
            Self::UnknownPage => "unknown-page",
            Self::AmbiguousPage => "ambiguous-page",
            Self::NoSection => "no-section",
            Self::NoMatch => "no-match",
            Self::Ambiguous => "ambiguous",
            Self::Contradiction => "contradiction",
            Self::SidecarConflict => "sidecar-conflict",
            Self::StalePlanned => "stale-planned",
            Self::HeuristicDisagreement => "heuristic-disagreement",
            Self::Unclassified => "unclassified",
        }
    }
}

#[cfg(test)]
mod tests {
    use bokfell_model::{Coords, Family};

    use super::*;
    use crate::model::{BlockCoverage, PageCoverage, PageRecord};

    fn record(version: Option<&str>, states: &[(BlockState, Option<(u32, u32)>)]) -> PageRecord {
        PageRecord {
            coords: Coords {
                component: "c".into(),
                version: version.map(str::to_string),
                module: "ROOT".into(),
                family: Family::Page,
                path: "p.adoc".into(),
            },
            url: "c/p.html".into(),
            repo_path: "docs/modules/ROOT/pages/p.adoc".into(),
            coverage: PageCoverage::from_blocks(
                states
                    .iter()
                    .map(|(state, page_lines)| BlockCoverage {
                        state: *state,
                        context: "paragraph".into(),
                        page_lines: *page_lines,
                        reason: None,
                        tracking: None,
                        claims: Vec::new(),
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn codecov_projection_follows_the_rfc_rule() {
        let db = CoverageDatabase {
            pages: vec![record(
                None,
                &[
                    (BlockState::Verified, Some((3, 2))),
                    (BlockState::Planned, Some((6, 1))),
                    (BlockState::Uncovered, Some((8, 1))),
                    (BlockState::Unclassified, Some((10, 1))),
                    (BlockState::OutOfScope, Some((12, 1))),
                    (BlockState::NonNormative, Some((14, 3))),
                    (BlockState::Verified, None),
                ],
            )],
            ..Default::default()
        };
        let json = codecov_json(&db);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let lines = &value["coverage"]["docs/modules/ROOT/pages/p.adoc"];
        assert_eq!(lines["3"], 1);
        assert_eq!(lines["4"], 1);
        assert_eq!(lines["6"], 0);
        assert_eq!(lines["8"], 0);
        assert_eq!(lines["10"], 0);
        assert!(lines.get("12").is_none());
        assert!(lines.get("14").is_none());
        assert!(lines.get("5").is_none());

        // The legacy reader consumes the export unchanged.
        let mut data = crate::CoverageData::new();
        data.load_str(&json).unwrap();
        assert!(!data.is_empty());
    }

    #[test]
    fn codecov_merges_versions_verified_wins() {
        let db = CoverageDatabase {
            pages: vec![
                record(Some("1"), &[(BlockState::Uncovered, Some((1, 1)))]),
                record(Some("2"), &[(BlockState::Verified, Some((1, 1)))]),
            ],
            ..Default::default()
        };
        let value: serde_json::Value = serde_json::from_str(&codecov_json(&db)).unwrap();
        assert_eq!(value["coverage"]["docs/modules/ROOT/pages/p.adoc"]["1"], 1);
    }

    #[test]
    fn table_lists_pages_and_totals() {
        let db = CoverageDatabase {
            pages: vec![
                record(
                    Some("1"),
                    &[
                        (BlockState::Verified, None),
                        (BlockState::Uncovered, None),
                        (BlockState::OutOfScope, None),
                    ],
                ),
                record(Some("2"), &[(BlockState::Verified, None)]),
            ],
            ..Default::default()
        };
        let text = table(&db);
        assert!(text.starts_with("PAGE"), "{text}");
        assert!(text.contains("[c 1]"));
        assert!(text.contains("50%"));
        assert!(text.contains("= all pages"));
        let total_line = text.lines().last().unwrap();
        assert!(total_line.contains("66%"), "{total_line}");

        // Several pages of one component version get a subtotal row.
        let mut second = record(Some("1"), &[(BlockState::Verified, None)]);
        second.url = "c/q.html".into();
        let db = CoverageDatabase {
            pages: vec![record(Some("1"), &[(BlockState::Uncovered, None)]), second],
            ..Default::default()
        };
        let text = table(&db);
        assert!(text.contains("  = c 1 "), "{text}");
        assert!(!text.contains("= all pages"), "{text}");
    }

    #[test]
    fn lint_lists_errors_then_findings_by_kind() {
        use crate::resolve::{Diagnostic, DiagnosticKind};

        assert!(lint(&CoverageDatabase::default()).is_empty());

        let diagnostic = |kind, at: &str, message: &str| Diagnostic {
            kind,
            at: at.into(),
            message: message.into(),
        };
        let db = CoverageDatabase {
            errors: vec![diagnostic(
                DiagnosticKind::NoMatch,
                "t.rs:3",
                "excerpt not found",
            )],
            lint: vec![
                diagnostic(DiagnosticKind::Unclassified, "p.adoc:9", "review me"),
                diagnostic(DiagnosticKind::StalePlanned, "m.toml", "verified now"),
                diagnostic(DiagnosticKind::Unclassified, "p.adoc:12", "and me"),
            ],
            ..Default::default()
        };
        let text = lint(&db);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "1 error(s):");
        assert_eq!(lines[1], "  error[no-match]: t.rs:3: excerpt not found");
        assert_eq!(lines[2], "1 stale-planned finding(s):");
        assert_eq!(lines[3], "  m.toml: verified now");
        assert_eq!(lines[4], "2 unclassified finding(s):");
        assert_eq!(lines[5], "  p.adoc:9: review me");
        assert_eq!(lines[6], "  p.adoc:12: and me");

        let tokens: Vec<&str> = [
            DiagnosticKind::UnknownPage,
            DiagnosticKind::AmbiguousPage,
            DiagnosticKind::NoSection,
            DiagnosticKind::NoMatch,
            DiagnosticKind::Ambiguous,
            DiagnosticKind::Contradiction,
            DiagnosticKind::SidecarConflict,
            DiagnosticKind::StalePlanned,
            DiagnosticKind::HeuristicDisagreement,
            DiagnosticKind::Unclassified,
        ]
        .iter()
        .map(|k| k.token())
        .collect();
        assert_eq!(
            tokens,
            [
                "unknown-page",
                "ambiguous-page",
                "no-section",
                "no-match",
                "ambiguous",
                "contradiction",
                "sidecar-conflict",
                "stale-planned",
                "heuristic-disagreement",
                "unclassified"
            ]
        );
    }
}
