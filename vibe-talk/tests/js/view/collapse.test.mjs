// Tests for web/src/view/dom/collapse.ts (`#1387 discuss-collapsible-sections`,
// `#1497 discuss-fold-state-persist`): heading sections fold, all collapsed;
// Expand/Collapse all flips every fold; a summary's open sections restore
// from the set kept for its record and a new summary starts collapsed.
import { test } from "node:test";
import assert from "node:assert/strict";
import { moduleForTests } from "../../../web/build.mjs";

const { foldSectionsByHeading, setAllSections, foldSectionKeys, openSectionKeys } =
  await moduleForTests("src/view/dom/collapse.ts");

function el(tag, text = "") {
  return {
    tagName: tag.toUpperCase(), children: [], open: undefined, textContent: text,
    appendChild(c) { this.children.push(c); },
    insertBefore(c, before) {
      const i = before === null ? -1 : this.children.indexOf(before);
      if (i < 0) this.children.push(c); else this.children.splice(i, 0, c);
    },
    removeChild(c) { this.children.splice(this.children.indexOf(c), 1); },
  };
}
const create = (tag) => el(tag);

function summaryRoot(sections = [["Alpha", "a"], ["Beta", "b"]]) {
  const root = el("div");
  for (const [heading, body] of sections) {
    root.appendChild(el("h2", heading));
    root.appendChild(el("p", body));
  }
  return root;
}

function foldsOf(root) {
  return root.children.filter((kid) => kid.tagName === "DETAILS");
}

test("collapse: sections fold collapsed and flip together", () => {
  const root = el("div");
  const h1 = el("h2"); const p1 = el("p"); const h2 = el("h2"); const p2 = el("p");
  for (const kid of [h1, p1, h2, p2]) root.appendChild(kid);
  assert.equal(foldSectionsByHeading(root, create), 2);
  const folds = root.children.filter((kid) => kid.tagName === "DETAILS");
  assert.equal(folds.length, 2);
  assert.ok(folds.every((fold) => fold.open === false), "sections did not start collapsed");
  assert.equal(folds[0].children[1].children[0], p1, "first section lost its body");
  setAllSections(root, true);
  assert.ok(folds.every((fold) => fold.open === true));
  setAllSections(root, false);
  assert.ok(folds.every((fold) => fold.open === false));
});

test("collapse: no headings, no folds", () => {
  const root = el("div");
  root.appendChild(el("p"));
  assert.equal(foldSectionsByHeading(root, create), 0);
});

test("collapse: a kept open set restores on a refold of the same summary (#1497)", () => {
  const root = summaryRoot();
  assert.equal(foldSectionsByHeading(root, create, new Set(["Beta"])), 2);
  const folds = foldsOf(root);
  assert.deepEqual(folds.map((fold) => fold.open), [false, true], "the kept section did not reopen");
  assert.deepEqual(foldSectionKeys(root), ["Alpha", "Beta"]);
  assert.deepEqual(openSectionKeys(root), ["Beta"]);
});

test("collapse: Expand/Collapse all reads back as the set to keep (#1497)", () => {
  const root = summaryRoot();
  foldSectionsByHeading(root, create);
  setAllSections(root, true);
  assert.deepEqual(openSectionKeys(root), ["Alpha", "Beta"], "Expand all did not read back whole");
  const reopened = summaryRoot();
  foldSectionsByHeading(reopened, create, new Set(openSectionKeys(root)));
  assert.ok(foldsOf(reopened).every((fold) => fold.open === true), "Expand all did not survive a refold");
  setAllSections(root, false);
  assert.deepEqual(openSectionKeys(root), [], "Collapse all did not read back empty");
});

test("collapse: a new summary starts collapsed; unknown kept keys are ignored (#1497)", () => {
  const fresh = summaryRoot([["Gamma", "g"], ["Delta", "d"]]);
  foldSectionsByHeading(fresh, create, new Set(["Beta", "No such section"]));
  assert.ok(foldsOf(fresh).every((fold) => fold.open === false), "a new summary inherited folds");
  assert.deepEqual(openSectionKeys(fresh), []);
  const same = summaryRoot();
  foldSectionsByHeading(same, create, new Set(["No such section"]));
  assert.ok(foldsOf(same).every((fold) => fold.open === false), "an unknown key opened a section");
});

test("collapse: duplicate and blank headings still key apart (#1497)", () => {
  const root = summaryRoot([["Notes", "a"], ["Notes", "b"], ["", "c"]]);
  foldSectionsByHeading(root, create, new Set(["Notes#2"]));
  assert.deepEqual(foldSectionKeys(root), ["Notes", "Notes#2", "section"]);
  assert.deepEqual(foldsOf(root).map((fold) => fold.open), [false, true, false]);
});
