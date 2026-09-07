// Bokfell spec-coverage overlay (PLAN.md §9.2).
//
// The page carries a JSON payload (#bokfell-cov-data) with the block
// pairing selector and one status token per block in document order,
// enumerated server-side from the AST. This script collects the article's
// matching elements outermost-only — mirroring the server's "never
// descend into an emitted block" rule — and applies the statuses only
// when the two counts agree, so a mismatch degrades to no overlay
// instead of shading the wrong blocks.
(function () {
  "use strict";

  var dataEl = document.getElementById("bokfell-cov-data");
  var toggle = document.getElementById("bokfell-cov-toggle");
  var article = document.querySelector("main.doc article");
  if (!dataEl || !toggle || !article) return;

  var data;
  try {
    data = JSON.parse(dataEl.textContent);
  } catch (e) {
    return;
  }
  if (!data || typeof data.selector !== "string" || !Array.isArray(data.blocks)) {
    return;
  }

  // Outermost-only collection: drop any match nested inside another match.
  var matches = article.querySelectorAll(data.selector);
  var blocks = [];
  for (var i = 0; i < matches.length; i++) {
    var el = matches[i];
    var ancestor = el.parentElement;
    var nested = false;
    while (ancestor && ancestor !== article) {
      if (ancestor.matches(data.selector)) {
        nested = true;
        break;
      }
      ancestor = ancestor.parentElement;
    }
    if (!nested) blocks.push(el);
  }

  if (blocks.length !== data.blocks.length) {
    toggle.disabled = true;
    toggle.title =
      "Coverage overlay unavailable: the rendered blocks do not line up " +
      "with the coverage map (" + blocks.length + " vs " +
      data.blocks.length + ")";
    return;
  }

  for (var j = 0; j < blocks.length; j++) {
    if (data.blocks[j]) {
      blocks[j].setAttribute("data-coverage", data.blocks[j]);
    }
  }

  // The toggle shades blocks via a body class; the choice sticks across
  // pages when storage is available.
  var STORAGE_KEY = "bokfell-coverage-overlay";
  function setOverlay(on) {
    document.body.classList.toggle("coverage-overlay", on);
    toggle.setAttribute("aria-pressed", on ? "true" : "false");
    try {
      window.localStorage.setItem(STORAGE_KEY, on ? "1" : "0");
    } catch (e) {
      // Storage unavailable; the toggle still works within this page.
    }
  }
  toggle.addEventListener("click", function () {
    setOverlay(!document.body.classList.contains("coverage-overlay"));
  });
  try {
    if (window.localStorage.getItem(STORAGE_KEY) === "1") {
      setOverlay(true);
    }
  } catch (e) {
    // Ignore; default to off.
  }
})();
