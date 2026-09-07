// Bokfell page overlays: spec coverage (PLAN.md §9.2), diff
// highlighting (PLAN.md §9.1), and click-to-source editing (PLAN.md
// §9.3, serve mode only).
//
// The page carries a JSON payload (#bokfell-overlay-data) with the block
// pairing selector plus per-block arrays for whichever overlays the page
// has, enumerated server-side from the AST in document order. Blocks
// anchor exactly by their data-source-line attributes when present
// (asciidoc-html5 0.2.2); otherwise this script collects the article's
// matching elements outermost-only — mirroring the server's "never
// descend into an emitted block" rule — and each overlay's toggle wires
// only when its array length matches the element count, so a mismatch
// degrades to no overlay instead of marking the wrong blocks.

// Pairs payload block lines with annotated elements. `annotatedLines` is
// every data-source-line value in document order; `targetLines` is the
// payload's per-block start lines. Returns, per target, the index of the
// matched element — or null overall when any target has no candidate
// left (the caller then falls back to the selector walk). Candidates
// sharing a line are consumed in document order, so a container and a
// same-line inner block cannot collide. Defined outside the DOM closure
// (and exported below) so it is unit-testable under Node.
function bokfellPairByLine(annotatedLines, targetLines) {
  var byLine = {};
  for (var i = 0; i < annotatedLines.length; i++) {
    var key = String(annotatedLines[i]);
    (byLine[key] || (byLine[key] = [])).push(i);
  }
  var indexes = [];
  for (var t = 0; t < targetLines.length; t++) {
    var candidates = byLine[String(targetLines[t])];
    if (!candidates || !candidates.length) return null;
    indexes.push(candidates.shift());
  }
  return indexes;
}

(function () {
  "use strict";

  // Under Node (tests), there is no DOM to wire.
  if (typeof document === "undefined") return;

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

  // Exact anchoring first: the payload's per-block source lines match
  // the containers' data-source-line attributes (asciidoc-html5 0.2.2),
  // consumed in document order so a container and its same-line inner
  // block cannot collide. Falls back to the shared outermost-only
  // selector walk when the annotations are missing or incomplete.
  var blocks = null;
  if (Array.isArray(data.lines) && data.lines.length) {
    var annotated = article.querySelectorAll("[data-source-line]");
    var annotatedLines = [];
    for (var a = 0; a < annotated.length; a++) {
      annotatedLines.push(annotated[a].getAttribute("data-source-line"));
    }
    var indexes = bokfellPairByLine(annotatedLines, data.lines);
    if (indexes) {
      blocks = [];
      for (var l = 0; l < indexes.length; l++) {
        blocks.push(annotated[indexes[l]]);
      }
    }
  }

  if (!blocks) {
    // Outermost-only collection: drop any match nested inside another
    // match.
    var matches = article.querySelectorAll(data.selector);
    blocks = [];
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

  // Click-to-source editing (serve mode): each block with a known
  // source location gets a small button that asks the dev server to
  // open that file and line in the configured editor; the watcher and
  // livereload complete the loop.
  if (Array.isArray(data.edit) && blocks.length === data.edit.length) {
    for (var k = 0; k < blocks.length; k++) {
      if (data.edit[k]) {
        addEditButton(blocks[k], data.edit[k][0], data.edit[k][1]);
      }
    }
  }

  function addEditButton(block, file, line) {
    var button = document.createElement("button");
    button.className = "bokfell-edit";
    button.type = "button";
    button.title = "Edit " + file + ":" + line;
    button.textContent = "\u270e";
    button.addEventListener("click", function (ev) {
      ev.stopPropagation();
      var params = new URLSearchParams({ file: file, line: String(line) });
      fetch("/__bokfell/edit?" + params.toString(), { method: "POST" }).then(
        function (response) {
          if (!response.ok) {
            response.text().then(function (text) {
              window.alert(text || "Cannot open the editor.");
            });
          }
        },
        function () {
          window.alert("Cannot reach the dev server.");
        }
      );
    });

    // Inserted before the block (a button can't live inside a <table>)
    // and floated to its top right by the stylesheet.
    block.parentNode.insertBefore(button, block);
  }

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

// Node test hook; browsers never define `module`.
if (typeof module !== "undefined" && module.exports) {
  module.exports = { pairByLine: bokfellPairByLine };
}
