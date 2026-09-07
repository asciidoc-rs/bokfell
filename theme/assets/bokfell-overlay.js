// Bokfell page overlays: spec coverage (PLAN.md §9.2) and diff
// highlighting (PLAN.md §9.1).
//
// The page carries a JSON payload (#bokfell-overlay-data) with the block
// pairing selector plus per-block arrays for whichever overlays the page
// has, enumerated server-side from the AST in document order. This
// script collects the article's matching elements outermost-only —
// mirroring the server's "never descend into an emitted block" rule —
// and wires each overlay's toggle only when its array length matches the
// element count, so a mismatch degrades to no overlay instead of marking
// the wrong blocks.
(function () {
  "use strict";

  var dataEl = document.getElementById("bokfell-overlay-data");
  var article = document.querySelector("main.doc article");
  if (!dataEl || !article) return;

  var data;
  try {
    data = JSON.parse(dataEl.textContent);
  } catch (e) {
    return;
  }
  if (!data || typeof data.selector !== "string") return;

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

  // Wires one overlay: verifies the count, applies the block marks, and
  // binds its toggle to a body class persisted across pages.
  function wire(toggle, expected, bodyClass, apply) {
    if (!Array.isArray(expected)) return;
    if (!toggle) return;
    if (blocks.length !== expected.length) {
      toggle.disabled = true;
      toggle.title =
        "Overlay unavailable: the rendered blocks do not line up with " +
        "the server's block map (" + blocks.length + " vs " +
        expected.length + ")";
      return;
    }
    apply();

    var storageKey = "bokfell-" + bodyClass;
    function setOverlay(on) {
      document.body.classList.toggle(bodyClass, on);
      toggle.setAttribute("aria-pressed", on ? "true" : "false");
      try {
        window.localStorage.setItem(storageKey, on ? "1" : "0");
      } catch (e) {
        // Storage unavailable; the toggle still works within this page.
      }
    }
    toggle.addEventListener("click", function () {
      setOverlay(!document.body.classList.contains(bodyClass));
    });
    try {
      if (window.localStorage.getItem(storageKey) === "1") {
        setOverlay(true);
      }
    } catch (e) {
      // Ignore; default to off.
    }
  }

  wire(
    document.getElementById("bokfell-cov-toggle"),
    data.coverage,
    "coverage-overlay",
    function () {
      for (var i = 0; i < blocks.length; i++) {
        if (data.coverage[i]) {
          blocks[i].setAttribute("data-coverage", data.coverage[i]);
        }
      }
    }
  );

  wire(
    document.getElementById("bokfell-diff-toggle"),
    data.diff,
    "diff-overlay",
    function () {
      for (var i = 0; i < blocks.length; i++) {
        var change = data.diff[i];
        if (!change) continue;
        var token = typeof change === "string" ? change : change[0];
        blocks[i].setAttribute("data-diff", token);
        if (token === "edited" && change[1]) {
          attachDetail(blocks[i], change[1]);
        }
      }
    }
  );

  // An edited block toggles a word-diff panel of its source on click
  // (while the diff overlay is on). The HTML is server-generated:
  // escaped source text with <ins>/<del> markers only.
  function attachDetail(block, html) {
    block.classList.add("has-diff-detail");
    block.addEventListener("click", function () {
      if (!document.body.classList.contains("diff-overlay")) return;
      var next = block.nextElementSibling;
      if (next && next.classList.contains("bokfell-diff-detail")) {
        next.remove();
        return;
      }
      var pre = document.createElement("pre");
      pre.className = "bokfell-diff-detail";
      pre.innerHTML = html;
      block.parentNode.insertBefore(pre, block.nextSibling);
    });
  }
})();
