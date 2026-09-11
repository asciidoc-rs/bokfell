# Theming

The built-in theme compiles into the `bokfell` binary. Pass a theme
directory with `--theme <dir>` (on `build` and `serve`) to replace the
built-in `layout.html` — that is the one file a theme overrides today.
There are no zipped UI bundles; link any custom stylesheets or scripts
from your replacement layout.

## Overriding the layout

A `layout.html` in the theme directory replaces the built-in page
template ([`theme/templates/layout.html`](../theme/templates/layout.html)
is the reference). Templates use [minijinja] — Jinja2 syntax.

[minijinja]: https://docs.rs/minijinja

Every page renders through the template with this context:

| Variable | Contents |
|----------|----------|
| `site.title` | The playbook's site title. |
| `page.title_text` | Page title as plain text (for `<title>`). |
| `page.title_html` | Page title as inline HTML (for the `<h1>`); absent for untitled pages. |
| `page.contents` | The rendered page body (embedded HTML) — emit with `\| safe`. |
| `page.url` | The page's site-root-relative URL. |
| `bokfell_version` | The generator version. |
| `nav_html` | The component version's navigation tree as nested `<ul>` lists, links relativized to the page (`\| safe`). |
| `versions_html` | The page-version selector, empty for single-version components (`\| safe`). |
| `css_href` / `home_href` | Page-relative links to the stylesheet and the site root. |
| `coverage` | Spec-coverage badge data when the page has coverage: `percent`, `verified`, `uncovered`, `level` (`high`/`mid`/`low`), `dashboard_href`. Absent otherwise. |
| `diff` | Change-badge data when the page changed against its diff base: `base_label`, `new_page`, `added`, `removed`, `edited`, `changes_href`. Absent otherwise. |
| `overlay_json` / `overlay_script_href` | The overlay payload and script for coverage/diff shading and click-to-source editing. Emit both exactly as the built-in layout does to keep those features. |
| `search_script_href` | The search client script (page-relative). |

Pre-relativized `*_href` values and the `*_html` fragments are already
escaped or generated — emit them with `| safe`.

## Keeping the built-in features

The overlay and search clients find their elements by ID, so a custom
layout keeps the features by preserving these hooks:

- `<input id="bokfell-search">` and `<div id="bokfell-search-results">`
  for search, plus the `search_script_href` script tag.
- `<button id="bokfell-cov-toggle">` and
  `<button id="bokfell-diff-toggle">` for the coverage/diff badges, plus
  the `overlay_json` payload block and `overlay_script_href` script tag,
  emitted exactly as in the built-in layout.
- An `<article>` inside `<main class="doc">` wrapping `page.contents` —
  the overlay pairs rendered blocks inside it.

Omit any of them and the corresponding feature simply switches off for
your theme; nothing else breaks.

## Styling

The built-in stylesheet publishes as `_/bokfell.css`
([`theme/assets/bokfell.css`](../theme/assets/bokfell.css) is the
reference). It keys colors off CSS custom properties (`--bf-*`) with a
`prefers-color-scheme: dark` override, so a lightweight restyle can be a
`layout.html` that links your own stylesheet after `css_href` and
redefines the variables.
