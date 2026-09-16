//! End-to-end spec-coverage scans through the binary (RFC 0001 §7): a
//! git repository scanned at a ref — test roots and spec map exported
//! from the clone, the crate name read from an ancestor manifest — and a
//! plain worktree whose claims no longer resolve.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn bokfell(cwd: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_bokfell"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run bokfell");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bokfell-covscan-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A repository holding a two-page component, a crate whose tests claim
/// both pages, and a spec map; `v1` predates the second page's rules.
fn fixture_repo(base: &Path) -> PathBuf {
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);

    write(
        &repo.join("docs/antora.yml"),
        "name: demo\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
    );
    write(
        &repo.join("docs/modules/ROOT/nav.adoc"),
        "* xref:index.adoc[]\n* xref:guide.adoc[]\n",
    );
    write(
        &repo.join("docs/modules/ROOT/pages/index.adoc"),
        "= Demo\n\nNested lists take one marker per level.\n",
    );
    write(
        &repo.join("docs/modules/ROOT/pages/guide.adoc"),
        "= Guide\n\n== Setup\n\nInstall it.\n\n== Unreviewed\n\nLeft alone.\n",
    );
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    git(&repo, &["tag", "v1"]);

    // The rules grow, and the tests and spec map arrive with them. The
    // crate manifest sits two levels above the test root, behind a
    // directory without one.
    write(
        &repo.join("docs/modules/ROOT/pages/index.adoc"),
        "= Demo\n\nNested lists take one marker per level.\n\n\
         A paragraph nobody has classified yet.\n\n----\n. one\n.. two\n----\n",
    );
    write(
        &repo.join("Cargo.toml"),
        "[workspace]\nmembers = [\"fixture\"]\n",
    );
    write(
        &repo.join("fixture/Cargo.toml"),
        "[package]\nname = \"fixture-crate\"\nversion = \"0.1.0\"\n",
    );
    write(
        &repo.join("fixture/src/tests/rules.rs"),
        "macro_rules! verifies { ($($t:tt)*) => {}; }\n\n\
         #[test]\nfn nesting_rule() {\n    \
         verifies!(\"docs/modules/ROOT/pages/index.adoc\", \"one marker per level\");\n}\n\n\
         #[test]\nfn setup_section() {\n    \
         verifies!(\"docs/modules/ROOT/pages/guide.adoc#_setup\");\n}\n",
    );
    write(
        &repo.join("spec-map/docs/modules/ROOT/pages/index.adoc.toml"),
        "reviewed = true\n\n[[planned]]\nexcerpt = \"nobody has classified yet\"\ntracking = \"o/r#7\"\n",
    );
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "two"]);
    repo
}

#[test]
fn scans_a_git_repository_at_a_ref() {
    let base = scratch("git");
    fixture_repo(&base);

    // The playbook names the repository by a relative path, both as the
    // content source and as the scan scope. A scope naming some other
    // repository (nothing to export from it) does not claim the pages.
    let site = base.join("site");
    write(
        &site.join("bokfell.yml"),
        "site:\n  title: Fixture\n\
         content:\n  sources:\n    - url: ../repo\n      branches: [main]\n      start_path: docs\n\
         output:\n  dir: out\n\
         runtime:\n  cache_dir: cache\n\
         coverage:\n  scan:\n    - repo: ../other\n    - repo: ../repo\n      ref: main\n      \
         tests: [fixture/src/tests]\n      spec_map: spec-map\n  \
         link_template: \"https://example.test/{repo_url}@{rev}/{path}#L{line}\"\n",
    );

    let (ok, stdout, stderr) = bokfell(&site, &["coverage", "scan"]);
    assert!(ok, "scan failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("Scanned 2 page(s)"), "{stdout}");
    let database = site.join("build/coverage.json");
    let db: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&database).unwrap()).unwrap();
    assert_eq!(db["errors"].as_array().unwrap().len(), 0, "{db:#}");

    // Both claims resolved, with the crate name from the ancestor
    // manifest and the export's commit as the revision.
    let claims = db["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 2, "{claims:#?}");
    for claim in claims {
        assert_eq!(claim["site"]["krate"], "fixture-crate");
        assert_eq!(claim["site"]["rev"].as_str().unwrap().len(), 40);
        assert!(claim["site"]["local_path"].is_null());
        assert!(claim["site"]["file"]
            .as_str()
            .unwrap()
            .starts_with("fixture/src/tests/rules.rs"));
    }
    let states = |page: &str| -> Vec<String> {
        db["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["coords"]["path"] == page)
            .unwrap_or_else(|| panic!("{page} measured"))["coverage"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["state"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        states("index.adoc"),
        ["verified", "planned", "non-normative"]
    );
    assert_eq!(states("guide.adoc"), ["verified", "unclassified"]);

    // Report, lint, and the dashboard: two pages of one component roll
    // up with a subtotal, and the one unclassified block is the review
    // queue.
    let (ok, table, _) = bokfell(&site, &["coverage", "report"]);
    assert!(ok);
    assert!(table.contains("demo/index.html"), "{table}");
    assert!(table.contains("  = demo "), "{table}");
    let (ok, json, _) = bokfell(&site, &["coverage", "report", "--format", "json"]);
    assert!(ok);
    assert!(json.trim_start().starts_with('{'), "{json}");
    let (ok, lint, _) = bokfell(&site, &["coverage", "lint"]);
    assert!(ok);
    assert!(lint.contains("1 unclassified finding(s):"), "{lint}");
    assert!(lint.contains("Left alone"), "{lint}");

    let (ok, stdout, stderr) = bokfell(&site, &["build"]);
    assert!(ok, "build failed:\n{stdout}\n{stderr}");
    let dashboard = std::fs::read_to_string(site.join("out/coverage.html")).unwrap();
    assert!(
        dashboard.contains("<span class=\"review-queue\">1</span>"),
        "{dashboard}"
    );
    assert_eq!(dashboard.matches("<tr class=\"component\">").count(), 1);
    assert!(dashboard.contains("demo/guide.html\""), "{dashboard}");
    assert!(dashboard.contains("demo/index.html\""), "{dashboard}");
    let index = std::fs::read_to_string(site.join("out/demo/index.html")).unwrap();
    assert!(index.contains("https://example.test/"), "templated link");
    assert!(index.contains("\"n\":\"nesting_rule\""), "inlined test");
    assert!(
        index.contains("https://github.com/o/r/issues/7"),
        "tracking link"
    );

    // PR-preview mode against the tag: the base catalog comes from the
    // same repository, and main's new paragraphs show as changes.
    let (ok, stdout, stderr) = bokfell(&site, &["build", "--diff-base", "v1"]);
    assert!(ok, "diff build failed:\n{stdout}\n{stderr}");
    let index = std::fs::read_to_string(site.join("out/demo/index.html")).unwrap();
    assert!(index.contains("v1"), "diff base label");

    // A plain directory source inside the clone still belongs to the
    // `repo` scope naming that clone.
    let mixed = base.join("mixed");
    write(
        &mixed.join("bokfell.yml"),
        "site:\n  title: Mixed\n\
         content:\n  sources:\n    - path: ../repo/docs\n\
         runtime:\n  cache_dir: cache\n\
         coverage:\n  scan:\n    - repo: ../repo\n      tests: [fixture/src/tests]\n      \
         spec_map: spec-map\n",
    );
    let (ok, stdout, stderr) = bokfell(&mixed, &["coverage", "scan", "--out", "nested/db.json"]);
    assert!(ok, "mixed scan failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("Scanned 2 page(s)"), "{stdout}");
    assert!(mixed.join("nested/db.json").is_file());

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn unresolved_claims_fail_scan_lint_and_build() {
    let base = scratch("local");
    write(
        &base.join("docs/antora.yml"),
        "name: loc\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
    );
    write(
        &base.join("docs/modules/ROOT/nav.adoc"),
        "* xref:index.adoc[]\n",
    );
    write(
        &base.join("docs/modules/ROOT/pages/index.adoc"),
        "= Local\n\nSome rule here.\n",
    );
    write(
        &base.join("tests/bad.rs"),
        "macro_rules! verifies { ($($t:tt)*) => {}; }\n\
         fn drifted() { verifies!(\"docs/modules/ROOT/pages/index.adoc\", \"wording that moved on\"); }\n\
         fn gone() { verifies!(\"docs/modules/ROOT/pages/missing.adoc\", \"x\"); }\n",
    );
    write(
        &base.join("bokfell.yml"),
        "site:\n  title: Local\n\
         content:\n  sources:\n    - path: docs\n\
         coverage:\n  scan:\n    - path: .\n      tests: [tests]\n",
    );

    // The database is written before the errors fail the command.
    let (ok, stdout, stderr) = bokfell(&base, &["coverage", "scan"]);
    assert!(!ok, "scan should fail:\n{stdout}");
    assert!(stderr.contains("error[coverage/no-match]"), "{stderr}");
    assert!(stderr.contains("error[coverage/unknown-page]"), "{stderr}");
    assert!(stderr.contains("2 spec-coverage error(s)"), "{stderr}");
    assert!(base.join("build/coverage.json").is_file());

    let (ok, stdout, stderr) = bokfell(&base, &["coverage", "lint"]);
    assert!(!ok, "lint should fail:\n{stdout}");
    assert!(stdout.contains("2 error(s):"), "{stdout}");
    assert!(stdout.contains("tests/bad.rs:2"), "{stdout}");
    assert!(stderr.contains("2 spec-coverage error(s)"), "{stderr}");

    // Reports still read the database that was written.
    let (ok, table, _) = bokfell(&base, &["coverage", "report"]);
    assert!(ok, "{table}");
    assert!(table.contains("loc/index.html"), "{table}");

    let (ok, stdout, stderr) = bokfell(&base, &["build"]);
    assert!(!ok, "build should fail:\n{stdout}");
    assert!(
        stderr.contains("fix the claims or coverage maps above"),
        "{stderr}"
    );

    // Without scan entries there is nothing to scan.
    write(
        &base.join("plain.yml"),
        "site:\n  title: Plain\ncontent:\n  sources:\n    - path: docs\n",
    );
    let (ok, _, stderr) = bokfell(&base, &["coverage", "scan", "--playbook", "plain.yml"]);
    assert!(!ok);
    assert!(stderr.contains("no `coverage.scan` entries"), "{stderr}");

    std::fs::remove_dir_all(&base).ok();
}

/// Sends one HTTP/1.0 request to the dev server and returns the response
/// body, or `None` while the server is not accepting connections yet.
fn http_get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};

    let mut stream = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(500),
    )
    .ok()?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes())
        .ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    let response = String::from_utf8_lossy(&response).into_owned();
    let (_, body) = response.split_once("\r\n\r\n")?;
    Some(body.to_string())
}

#[test]
fn serve_mode_warns_on_errors_and_exposes_local_tests_for_editing() {
    let base = scratch("serve");
    write(
        &base.join("docs/antora.yml"),
        "name: live\nversion: ~\nnav:\n- modules/ROOT/nav.adoc\n",
    );
    write(
        &base.join("docs/modules/ROOT/nav.adoc"),
        "* xref:index.adoc[]\n",
    );
    write(
        &base.join("docs/modules/ROOT/pages/index.adoc"),
        "= Live\n\nA rule under test.\n\nA rule that drifted.\n",
    );
    write(
        &base.join("tests/live.rs"),
        "macro_rules! verifies { ($($t:tt)*) => {}; }\n\n\
         #[test]\nfn holds() {\n    \
         verifies!(\"docs/modules/ROOT/pages/index.adoc\", \"A rule under test.\");\n}\n\n\
         #[test]\nfn stale() {\n    \
         verifies!(\"docs/modules/ROOT/pages/index.adoc\", \"the old wording\");\n}\n",
    );
    write(
        &base.join("bokfell.yml"),
        "site:\n  title: Live\n\
         content:\n  sources:\n    - path: docs\n\
         coverage:\n  scan:\n    - path: .\n      tests: [tests]\n",
    );

    // A port nobody is listening on right now.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bokfell"))
        .args([
            "serve",
            "--port",
            &port.to_string(),
            "--editor",
            "true {file}",
        ])
        .current_dir(&base)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn serve");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let page = loop {
        if let Some(body) = http_get(port, "/live/index.html") {
            break body;
        }
        if let Some(status) = child.try_wait().unwrap() {
            let mut stderr = String::new();
            std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).ok();
            panic!("serve exited early ({status}):\n{stderr}");
        }
        assert!(
            std::time::Instant::now() < deadline,
            "serve did not come up"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    child.kill().ok();
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The unresolved claim is reported but does not stop the server, and
    // the resolved one ships its local file as an edit target — for the
    // claim line and for the inlined test function alike.
    assert!(stderr.contains("error[coverage/no-match]"), "{stderr}");
    assert!(page.contains("\"s\":\"verified\""), "{page}");
    assert!(page.contains("\"n\":\"holds\""), "{page}");
    let edit_targets = page.matches("\"e\":[").count();
    assert_eq!(edit_targets, 2, "{page}");
    assert!(page.contains("tests/live.rs"), "{page}");

    std::fs::remove_dir_all(&base).ok();
}
