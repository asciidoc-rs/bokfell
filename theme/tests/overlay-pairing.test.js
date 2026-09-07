// Unit tests for the overlay client's exact-anchor pairing
// (theme/assets/bokfell-overlay.js), run by `node --test theme/tests`.

"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");

const { pairByLine } = require("../assets/bokfell-overlay.js");

test("pairs each payload line with its annotated element", () => {
  assert.deepEqual(pairByLine(["3", "7", "12"], [3, 7, 12]), [0, 1, 2]);
});

test("extra annotated elements (nested blocks) are simply unused", () => {
  // Element 1 is a nested block (line 4) the payload does not target.
  assert.deepEqual(pairByLine(["3", "4", "9"], [3, 9]), [0, 2]);
});

test("duplicate lines consume candidates in document order", () => {
  // A container and its inner block share line 5: the first (outermost,
  // document order) element wins the first slot; a second target on the
  // same line would take the inner one.
  assert.deepEqual(pairByLine(["5", "5", "8"], [5, 8]), [0, 2]);
  assert.deepEqual(pairByLine(["5", "5"], [5, 5]), [0, 1]);
});

test("a target with no candidate rejects the whole pairing", () => {
  // The caller falls back to the selector walk on null, never decorating
  // a partial match.
  assert.equal(pairByLine(["3"], [3, 9]), null);
  assert.equal(pairByLine([], [1]), null);
});

test("a line with fewer candidates than targets rejects the pairing", () => {
  assert.equal(pairByLine(["5", "8"], [5, 5]), null);
});

test("string and number lines compare equal", () => {
  assert.deepEqual(pairByLine(["10"], ["10"]), [0]);
  assert.deepEqual(pairByLine([10], ["10"]), [0]);
});

test("empty targets pair trivially", () => {
  assert.deepEqual(pairByLine(["1"], []), []);
});
