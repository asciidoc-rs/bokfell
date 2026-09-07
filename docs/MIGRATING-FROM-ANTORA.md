# Migrating from Antora

Bokfell reads Antora's content layout directly, so most migrations are a
playbook translation — the content repositories usually need no changes
at all.

## What carries over unchanged

- **The content layout**: `antora.yml` at each content root and the
  `modules/<module>/{pages,partials,images,attachments,examples}` tree.
  Bokfell reads `name`, `title`, `version` (`~` for versionless),
  `display_version`, `prerelease`, `nav`, and `asciidoc.attributes`.
- **Resource IDs**: `version@component:module:family$path` references in
  xrefs and includes (`partial$note.adoc`, `example$config.yml`,
  `image::name.png[]` through the module's images family).
- **Navigation**: `nav.adoc` files listed in `antora.yml`, rendered as
  nested lists.
- **URL rules**: the `ROOT` component and module segments drop out of
  URLs, versionless components omit the version segment, images publish
  under `_images/`, and the latest version is the first non-prerelease in
  Antora's version ordering.

## Translating the playbook

```yaml
# antora-playbook.yml                # bokfell.yml
site:                                site:
  title: My Docs                       title: My Docs
  start_page: comp::index.adoc         start_page: comp::index.adoc
content:                             content:
  sources:                             sources:
  - url: https://example.com/r.git     - url: https://example.com/r.git
    branches: [main, v2.*]               branches: [main, v2.*]
    tags: [v1.*, '!v1.0.*']              tags: [v1.*, '!v1.0.*']
    start_path: docs                     start_path: docs
                                         version_from_ref: true  # optional
asciidoc:                            asciidoc:
  attributes:                          attributes:
    page-pagination: ''                  page-pagination: ''
output:                              output:
  dir: build/site                      dir: build/site
runtime:                             runtime:
  cache_dir: .cache/bokfell            cache_dir: cache
```

Local directories use `- path: ../repo/docs` instead of `url`. Branch and
tag patterns support `*` globs, `!` negation, and `HEAD`.

## What Bokfell adds

- `version_from_ref: true` derives a source's component version from the
  matched git ref name instead of `antora.yml`.
- `coverage:` / `coverage_prefix:` per source attach spec-coverage JSON
  (verified-percentage badges, block shading, a `/coverage.html`
  dashboard).
- Version-to-version diffs by default, `--diff-base <ref>` for PR
  previews, and a `/whats-changed.html` index.
- `bokfell serve` with live reload and click-to-source editing.
- Built-in client-side search — no site extension needed.
- One static binary; no Node.js installation.

## Not (yet) supported

- Antora **extensions** and Asciidoctor extensions (Bokfell has no
  JavaScript runtime; renderer features come from `asciidoc-html5`).
- **UI bundles**: theming is a directory of file overrides
  (see [THEMING.md](THEMING.md)), not a zipped UI project.
- `edit_url`, per-component `start_page`, multiple `start_paths` on one
  source, git credential configuration, redirects, and sitemap
  generation (redirects and sitemap are on the roadmap — `PLAN.md` §10).
- AsciiDoc constructs the `asciidoc-html5` renderer does not cover yet
  render as visible `<!-- unsupported -->` comments rather than silently
  disappearing.
