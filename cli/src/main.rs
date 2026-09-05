//! Bokfell command-line entry point.
//!
//! The real CLI (`build`, `serve`, `diff`, `coverage`, `init`) arrives
//! with milestone M1; see `PLAN.md` at the workspace root.

fn main() {
    eprintln!(
        "bokfell {}: nothing is implemented yet — see PLAN.md for the roadmap",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(1);
}
