//! Manual smoke test: aggregate the asciidoc-html5 repo's docs from two refs.

fn main() {
    let aggregator =
        bokfell_aggregate::Aggregator::new(std::env::temp_dir().join("bokfell-smoke-cache"), false);
    let source = bokfell_aggregate::GitSource {
        url: "/home/user/asciidoc-html5".to_string(),
        branches: vec!["HEAD".to_string()],
        tags: vec!["asciidoc-html5-v0.2.1".to_string()],
        start_path: "docs".to_string(),
        version_from_ref: true,
    };
    match aggregator.collect(&source) {
        Ok(roots) => {
            for root in roots {
                println!(
                    "ref={} version={:?} path={} antora.yml={}",
                    root.refname,
                    root.version_override,
                    root.path.display(),
                    root.path.join("antora.yml").is_file()
                );
            }
        }
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}
