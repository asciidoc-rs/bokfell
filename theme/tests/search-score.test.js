// Unit tests for the search client's pure scoring function
// (bokfellScoreEntry in assets/bokfell-search.js), run with `node --test`.
const test = require("node:test");
const assert = require("node:assert/strict");
const { scoreEntry } = require("../assets/bokfell-search.js");

const lead = { title: "Table column widths", url: "t.html", text: "Columns share width evenly." };
const section = {
  title: "Proportional widths",
  url: "t.html#_proportional_widths",
  text: "Numbers set relative shares.",
  page: "Table column widths",
};

test("every term must match somewhere or the entry scores zero", () => {
  assert.equal(scoreEntry(lead, ["widths", "missingterm"]), 0);
  assert.ok(scoreEntry(lead, ["widths", "evenly"]) > 0);
});

test("a term matching only the parent page title still matches", () => {
  // "table" appears in neither the section title nor its text.
  assert.ok(scoreEntry(section, ["table", "proportional"]) > 0);
});

test("own-title hits outweigh parent-page hits, which outweigh body hits", () => {
  const inTitle = scoreEntry(section, ["proportional"]);
  const inPage = scoreEntry(section, ["table"]);
  const inText = scoreEntry(section, ["relative"]);
  assert.ok(inTitle > inPage, `${inTitle} > ${inPage}`);
  assert.ok(inPage > inText, `${inPage} > ${inText}`);
});

test("an exact title match gets an extra boost", () => {
  const exact = { title: "widths", url: "w.html", text: "" };
  assert.ok(scoreEntry(exact, ["widths"]) > scoreEntry(lead, ["widths"]));
});

test("entries without a page field score on title and text alone", () => {
  assert.ok(scoreEntry(lead, ["column"]) > 0);
  assert.equal(scoreEntry(lead, ["table", "share", "nothere"]), 0);
});

test("matching is case-insensitive against the entry", () => {
  // Terms arrive lowercased; entry fields may be mixed case.
  assert.ok(scoreEntry(section, ["proportional", "numbers"]) > 0);
});
