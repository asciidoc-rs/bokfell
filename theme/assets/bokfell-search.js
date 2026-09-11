// Bokfell client-side search (PLAN.md §12, resolved at M7).
//
// The build emits _/search-index.json: a {title, url, text} lead entry
// per page plus one per anchored section, whose url carries the
// #fragment and whose `page` names the parent page. This script
// lazy-loads the index on first focus and scores entries with plain
// token matching — every term must match (title, parent page title, or
// body), title hits weigh more — rendering the top results as links. No
// library, no network beyond the one index fetch.

// Scores one index entry against the lowercased query terms. Every term
// must appear in the entry's title, parent page title, or body text or
// the entry scores 0; otherwise hits accumulate weight (own title
// highest, parent page title next, body least). Defined outside the DOM
// closure (and exported below) so it is unit-testable under Node.
function bokfellScoreEntry(entry, terms) {
  var title = entry.title.toLowerCase();
  var page = (entry.page || "").toLowerCase();
  var text = entry.text.toLowerCase();
  var total = 0;
  for (var i = 0; i < terms.length; i++) {
    var inTitle = title.indexOf(terms[i]) !== -1;
    var inPage = page.indexOf(terms[i]) !== -1;
    var inText = text.indexOf(terms[i]) !== -1;
    if (!inTitle && !inPage && !inText) return 0;
    if (inTitle) total += 10;
    if (title === terms[i]) total += 20;
    if (inPage) total += 5;
    if (inText) total += 1;
  }
  return total;
}

(function () {
  "use strict";

  if (typeof document === "undefined") return;

  var input = document.getElementById("bokfell-search");
  var results = document.getElementById("bokfell-search-results");
  if (!input || !results) return;

  // The site root, derived from this script's own URL so pages at any
  // depth resolve the index and result links correctly.
  var script = document.querySelector('script[src$="bokfell-search.js"]');
  var root = script ? script.src.replace(/_\/bokfell-search\.js$/, "") : "/";

  var index = null;
  var loading = false;

  function load() {
    if (index || loading) return;
    loading = true;
    fetch(root + "_/search-index.json").then(
      function (response) { return response.json(); }
    ).then(
      function (data) { index = data; run(); },
      function () { loading = false; }
    );
  }

  function snippet(entry, terms) {
    var text = entry.text;
    var at = -1;
    for (var i = 0; i < terms.length && at === -1; i++) {
      at = text.toLowerCase().indexOf(terms[i]);
    }
    if (at === -1) return text.slice(0, 120);
    var from = Math.max(0, at - 50);
    return (from > 0 ? "…" : "") + text.slice(from, at + 90) + "…";
  }

  function setExpanded(open) {
    results.hidden = !open;
    input.setAttribute("aria-expanded", open ? "true" : "false");
  }

  function run() {
    var query = input.value.trim().toLowerCase();
    if (!query) {
      setExpanded(false);
      results.textContent = "";
      return;
    }
    if (!index) {
      load();
      return;
    }

    var terms = query.split(/\s+/);
    var scored = [];
    for (var i = 0; i < index.length; i++) {
      var s = bokfellScoreEntry(index[i], terms);
      if (s > 0) scored.push([s, index[i]]);
    }
    scored.sort(function (a, b) { return b[0] - a[0]; });

    results.textContent = "";
    var top = scored.slice(0, 10);
    for (var j = 0; j < top.length; j++) {
      var entry = top[j][1];
      var link = document.createElement("a");
      link.href = root + entry.url;
      var title = document.createElement("strong");
      title.textContent = entry.title;
      link.appendChild(title);
      if (entry.page) {
        var page = document.createElement("em");
        page.className = "result-page";
        page.textContent = entry.page;
        link.appendChild(page);
      }
      var body = document.createElement("span");
      body.textContent = snippet(entry, terms);
      link.appendChild(body);
      results.appendChild(link);
    }
    if (!top.length) {
      var none = document.createElement("span");
      none.className = "no-results";
      none.textContent = "No matches";
      results.appendChild(none);
    }
    setExpanded(true);
  }

  input.addEventListener("input", run);
  input.addEventListener("focus", function () {
    load();
    if (input.value.trim()) run();
  });
  input.addEventListener("keydown", function (ev) {
    if (ev.key === "Escape") {
      setExpanded(false);
      input.blur();
    }
  });
  document.addEventListener("click", function (ev) {
    if (ev.target !== input && !results.contains(ev.target)) {
      setExpanded(false);
    }
  });
})();

if (typeof module !== "undefined" && module.exports) {
  module.exports = { scoreEntry: bokfellScoreEntry };
}
