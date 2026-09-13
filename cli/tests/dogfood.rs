//! Bokfell's own documentation site, scanned and built by the binary:
//! the spec-coverage engine measuring the RFC that specifies it.

use std::{path::PathBuf, process::Command};

/// The no-op claim marker (RFC 0001 §4).
macro_rules! verifies {
    ($($tt:tt)*) => {};
}

const RFC: &str = "docs/modules/rfcs/pages/0001-spec-coverage.adoc";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

fn bokfell(args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_bokfell"))
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("run bokfell");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn scans_reports_lints_and_builds_the_dogfood_site() {
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`bokfell coverage scan` -- aggregate (reusing `bokfell-aggregate`), parse measured pages, scan test roots, resolve claims and sidecar entries, write the coverage database."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "`bokfell build` / `serve` consume the database directly -- no `coverage:` file handoff needed when the scan config is in the playbook"
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "Dashboards show the full five-way breakdown (stacked bar per page and per component), with `out-of-scope` displayed adjacent to, but outside, the bar."
    );
    verifies!(
        "docs/modules/rfcs/pages/0001-spec-coverage.adoc",
        "*Dashboard.* Per-page and per-component stacked bars over the five states, `out-of-scope` beside the bar, `unclassified` called out as the review queue."
    );

    let scratch = std::env::temp_dir().join(format!("bokfell-dogfood-{}", std::process::id()));
    std::fs::remove_dir_all(&scratch).ok();
    std::fs::create_dir_all(&scratch).unwrap();
    let database = scratch.join("coverage.json");
    let database_arg = database.to_string_lossy().into_owned();
    let site = scratch.join("site");
    let site_arg = site.to_string_lossy().into_owned();

    // Scan: every claim and sidecar entry resolves, and the RFC page is
    // measured.
    let (ok, stdout, stderr) = bokfell(&["coverage", "scan", "--out", &database_arg]);
    assert!(ok, "scan failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("Scanned 1 page(s)"), "{stdout}");
    assert!(database.is_file());

    // Report: the table names the RFC page; the Codecov projection keys
    // it by repository path.
    let (ok, table, _) = bokfell(&["coverage", "report", "--database", &database_arg]);
    assert!(ok);
    assert!(
        table.contains("bokfell/rfcs/0001-spec-coverage.html"),
        "{table}"
    );
    let (ok, codecov, _) = bokfell(&[
        "coverage",
        "report",
        "--format",
        "codecov",
        "--database",
        &database_arg,
    ]);
    assert!(ok);
    let value: serde_json::Value = serde_json::from_str(&codecov).unwrap();
    assert!(value["coverage"][RFC].is_object(), "{codecov}");

    // Lint: no hard errors (findings may exist).
    let (ok, _, stderr) = bokfell(&["coverage", "lint", "--database", &database_arg]);
    assert!(ok, "lint failed:\n{stderr}");

    // Build: the page carries the badge and the five-state payload, and
    // the dashboard shows stacked bars with out-of-scope beside them.
    let (ok, stdout, stderr) = bokfell(&["build", "--out", &site_arg]);
    assert!(ok, "build failed:\n{stdout}\n{stderr}");
    let page = std::fs::read_to_string(site.join("bokfell/rfcs/0001-spec-coverage.html")).unwrap();
    assert!(page.contains("% verified</button>"), "no badge");
    assert!(page.contains("\"s\":\"verified\""), "no verified block");
    assert!(page.contains("\"s\":\"planned\""), "no planned block");
    assert!(
        page.contains("\"s\":\"non-normative\""),
        "no non-normative block"
    );
    assert!(
        page.contains("\"l\":\"bokfell-coverage::"),
        "no claim labels"
    );

    let dashboard = std::fs::read_to_string(site.join("coverage.html")).unwrap();
    assert!(dashboard.contains("class=\"cov-bar\""), "no stacked bar");
    assert!(
        dashboard.contains("<tr class=\"component\">"),
        "no component row"
    );
    assert!(
        dashboard.contains("<th>Out of scope</th>"),
        "out-of-scope column missing"
    );
    assert!(
        dashboard.contains("review queue"),
        "unclassified not called out"
    );

    std::fs::remove_dir_all(&scratch).ok();
}
