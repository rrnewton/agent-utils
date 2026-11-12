// Tests for web/src/view/collapse.ts (`#1387 discuss-collapsible-sections`):
// heading sections fold, all collapsed; Expand/Collapse all flips every fold.
import { test } from "node:test";
import assert from "node:assert/strict";
import { moduleForTests } from "../../../web/build.mjs";

const { foldSectionsByHeading, setAllSections } = await moduleForTests("src/view/collapse.ts");

function el(tag) {
  return {
    tagName: tag.toUpperCase(), children: [], open: undefined,
    appendChild(c) { this.children.push(c); },
    insertBefore(c, before) {
      const i = before === null ? -1 : this.children.indexOf(before);
      if (i < 0) this.children.push(c); else this.children.splice(i, 0, c);
    },
    removeChild(c) { this.children.splice(this.children.indexOf(c), 1); },
  };
}
const create = (tag) => el(tag);

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
