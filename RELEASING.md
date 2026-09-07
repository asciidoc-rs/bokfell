# Releasing Bokfell

## Versioning

All workspace crates share one version, set in `[workspace.package]` in the
root `Cargo.toml`. Bump it there (and the `bokfell-*` path-dependency
`version` fields, which must match) in a normal PR.

## Binaries

Release binaries build automatically: push a `v*` tag (e.g. `v0.1.0`) on a
main-branch commit and the [Release workflow](.github/workflows/release.yml)
attaches `bokfell` archives for Linux (x86_64), macOS (arm64 and x86_64),
and Windows (x86_64), each with a SHA-256 checksum, to the GitHub release
for that tag.

```sh
git tag v0.1.0
git push origin v0.1.0
```

## Crates

Publish to crates.io in dependency order — each crate must be on crates.io
before its dependents can be published:

```sh
cargo publish -p bokfell-model      # no in-workspace dependencies
cargo publish -p bokfell-diff       # no in-workspace dependencies
cargo publish -p bokfell-serve      # no in-workspace dependencies
cargo publish -p bokfell-search     # no in-workspace dependencies
cargo publish -p bokfell-aggregate  # depends on model
cargo publish -p bokfell-coverage   # depends on model
cargo publish -p bokfell-theme      # depends on model
cargo publish -p bokfell-render     # depends on model, coverage, diff
cargo publish -p bokfell            # the CLI; depends on all of the above
```
