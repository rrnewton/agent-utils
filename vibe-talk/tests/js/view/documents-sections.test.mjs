// Tests for the Documents navigator's section model (`#1508
// documents-scrubber-multilevel`): the normalized section tree over the
// ONE block model (`blocks.ts`), its numbers (title rule, skipped levels,
// Introduction, the author's numbers winning), aggregation to K_target
// with the minimum chunk size, range compression, and boundary marks.
// Pure over the blocks, held here without a reader.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const BLOCKS = await moduleForTests("src/view/documents/blocks.ts");
const SECTIONS = await moduleForTests("src/view/documents/sections.ts");

function model(markdown) {
  const blocks = BLOCKS.documentReadBlocks(markdown);
  return { blocks, model: SECTIONS.buildSections(blocks) };
}

function numbers(markdown) {
  return model(markdown).model.sections.map((section) => section.number);
}

test("sections: the title is not numbered and its level is dropped", () => {
  const markdown = "# Field notes\n\nOpening words.\n\n## Alpha\n\nBody.\n\n## Beta\n\nMore.\n";
  const { model: built } = model(markdown);
  assert.equal(built.titleBlock, 0);
  assert.deepEqual(numbers(markdown), ["0", "1", "2"]);
  assert.deepEqual(built.sections.map((s) => s.title), ["Introduction", "Alpha", "Beta"]);
});

test("sections: no title where two headings share the top level", () => {
  const markdown = "# Alpha\n\nBody.\n\n# Beta\n\nMore.\n";
  assert.deepEqual(numbers(markdown), ["1", "2"]);
});

test("sections: skipped source levels collapse to ancestor rank", () => {
  const markdown = "# T\n\n## Alpha\n\n#### Deep\n\n#### Deep two\n\n## Beta\n";
  const { model: built } = model(markdown);
  assert.deepEqual(built.sections.map((s) => [s.number, s.depth]), [["1", 0], ["1.1", 1], ["1.2", 1], ["2", 0]]);
});

test("sections: text before the first section is Introduction, number 0", () => {
  const { model: built } = model("# T\n\nLoose words first.\n\n## Alpha\n\nBody.\n");
  const intro = built.sections[0];
  assert.equal(intro.number, "0");
  assert.equal(intro.title, "Introduction");
  assert.equal(intro.headingBlock, null);
  assert.equal(SECTIONS.blockAddress(built, intro.start + 1), "0 ¶ 1");
});

test("sections: author numbers win (3., 3.2, III., A.)", () => {
  assert.deepEqual(numbers("# T\n\n## 3. Alpha\n\nx\n\n## 4. Beta\n\ny\n"), ["3", "4"]);
  assert.deepEqual(numbers("# T\n\n## One\n\n### 3.1 Deep\n\nx\n\n### 3.2 Deep two\n\ny\n"), ["1", "3.1", "3.2"]);
  assert.deepEqual(numbers("# T\n\n## III. Alpha\n\nx\n\n## IV. Beta\n\ny\n"), ["III", "IV"]);
  assert.deepEqual(numbers("# T\n\n## A. Alpha\n\nx\n\n## B. Beta\n\ny\n"), ["A", "B"]);
});

test("sections: author numbers win per level only", () => {
  // One unnumbered heading at the level defeats the author's numbers there.
  const markdown = "# T\n\n## 3. Alpha\n\nx\n\n## Beta\n\ny\n";
  assert.deepEqual(numbers(markdown), ["1", "2"]);
  // A computed parent still prefixes an author's dotted child coherently.
  const mixed = model("# T\n\n## Alpha\n\n### 3.1 Deep\n\nx\n");
  assert.equal(mixed.model.sections[1].number, "3.1");
});

test("sections: authorNumberOf rejects years and invalid roman", () => {
  assert.equal(SECTIONS.authorNumberOf("2024 in review"), null);
  assert.equal(SECTIONS.authorNumberOf("IIII. Wrong"), null);
  assert.equal(SECTIONS.authorNumberOf("Plain heading"), null);
  assert.equal(SECTIONS.authorNumberOf("3.2 Dotted"), "3.2");
  assert.equal(SECTIONS.authorNumberOf("A) Lettered"), "A");
});

test("sections: addresses name the section and block", () => {
  const { blocks, model: built } = model("# T\n\n## Alpha\n\nFirst.\n\nSecond.\n\n## Beta\n\nThird.\n");
  const beta = built.sections.find((s) => s.title === "Beta");
  assert.equal(SECTIONS.blockAddress(built, beta.start + 1), "2 ¶ 1");
  assert.equal(SECTIONS.sectionIndexAt(built, blocks.length - 1), built.sections.indexOf(beta));
});

function bigDocument() {
  // Hundreds of blocks, nested sections, one huge leaf section.
  const lines = ["# Big document", "", "Intro paragraph.", ""];
  for (let s = 1; s <= 12; s += 1) {
    lines.push(`## Section ${s}`, "");
    for (let ss = 1; ss <= 3; ss += 1) {
      lines.push(`### Section ${s}.${ss}`, "");
      const count = s === 3 && ss === 2 ? 118 : 6;
      for (let b = 0; b < count; b += 1) lines.push(`Paragraph ${s}.${ss} number ${b} with enough words to weigh something in the balance of the row.`, "");
    }
  }
  return lines.join("\n");
}

test("sections: a large document numbers nested sections in order", () => {
  const { blocks, model: built } = model(bigDocument());
  assert.ok(blocks.length > 300, `expected hundreds of blocks, got ${blocks.length}`);
  const deep = built.sections.find((s) => s.title === "Section 3.2");
  assert.equal(deep.number, "3.2");
  assert.ok(deep.end - deep.start > 100, "the huge leaf section holds its blocks");
});

test("navigator: K values come from the width (24px target, 14px hard)", () => {
  assert.deepEqual(SECTIONS.navigatorKs(360), { kTarget: 15, kHard: 25, maxRows: 2 });
  assert.deepEqual(SECTIONS.navigatorKs(720), { kTarget: 30, kHard: 51, maxRows: 3 });
});

test("navigator: leaf aggregation honours K_hard and the minimum chunk", () => {
  const { blocks, model: built } = model(bigDocument());
  const huge = built.sections.find((s) => s.title === "Section 3.2");
  const units = SECTIONS.aggregateLeafUnits(blocks, huge, 15, 25);
  assert.ok(units.length <= 25, `expected at most K_hard units, got ${units.length}`);
  for (const unit of units) {
    assert.ok(unit.weight >= SECTIONS.MIN_CHUNK_CHARS || units.length === 1,
      `unit of ${unit.weight} chars is below the minimum chunk size`);
  }
  // The units tile the section exactly, in order.
  assert.equal(units[0].start, huge.start);
  assert.equal(units[units.length - 1].end, huge.end);
  for (let at = 1; at < units.length; at += 1) assert.equal(units[at].start, units[at - 1].end);
});

test("navigator: list runs aggregate into single units", () => {
  const markdown = "# T\n\n## Alpha\n\n- one\n- two\n- three\n- four\n- five\n- six\n- seven\n- eight\n";
  const { blocks, model: built } = model(markdown);
  const alpha = built.sections.find((s) => s.title === "Alpha");
  const units = SECTIONS.aggregateLeafUnits(blocks, alpha, 15, 25);
  assert.equal(units.length, 1, "one list run plus its heading is one unit");
  assert.equal(units[0].end - units[0].start, alpha.end - alpha.start);
});

test("navigator: two rows on a phone, ranges compress the peers", () => {
  const { blocks, model: built } = model(bigDocument());
  const inDeep = built.sections.find((s) => s.title === "Section 3.2");
  const nav = SECTIONS.buildNavigator(blocks, built, inDeep.start + 5, 168);
  assert.ok(nav.rows.length <= 2, `phone shows at most 2 rows, got ${nav.rows.length}`);
  assert.equal(nav.address.startsWith("3.2 ¶"), true, `address is ${nav.address}`);
  const top = nav.rows[0];
  assert.ok(top.units.length <= nav.kHard, "the top row fits K_hard after compression");
  assert.ok(top.units.some((u) => u.kind === "range" && u.label.includes("…")),
    `peers compressed into a range: ${top.units.map((u) => u.label).join(",")}`);
  const numbers = nav.rows.flatMap((row) => row.units.map((u) => u.number || u.label));
  assert.ok(numbers.some((n) => n.startsWith("3")), "section numbers ride the rows");
});

test("navigator: tapping a unit starts at its first block (units tile)", () => {
  const { blocks, model: built } = model(bigDocument());
  const nav = SECTIONS.buildNavigator(blocks, built, 10, 360);
  for (const row of nav.rows) {
    for (const unit of row.units) {
      assert.ok(unit.start < unit.end && unit.end <= blocks.length);
    }
  }
  const current = nav.rows[nav.rows.length - 1];
  assert.ok(current.current >= 0, "the bottom row marks the reading unit");
});

test("navigator: boundary marks name the section a bottom-row unit enters", () => {
  const { blocks, model: built } = model(bigDocument());
  // Reading in section 1.1: the bottom row is its parent row's units where
  // the sequence crosses sections; at minimum the model exposes adjacent
  // section numbers for the edge placeholders.
  const first = built.sections.find((s) => s.title === "Section 1.1");
  const nav = SECTIONS.buildNavigator(blocks, built, first.start + 1, 360);
  assert.equal(nav.nextSection !== "", true);
  const marked = nav.rows.flatMap((r) => r.units).filter((u) => u.boundary !== "");
  assert.ok(marked.every((u) => built.sections.some((s) => s.number === u.boundary)));
});

test("navigator: a document without headings gets one aggregated row", () => {
  const markdown = Array.from({ length: 80 }, (_, i) => `Paragraph number ${i} carrying enough words to weigh in the balance of the scrubber row.`).join("\n\n");
  const { blocks, model: built } = model(markdown);
  const nav = SECTIONS.buildNavigator(blocks, built, 40, 360);
  assert.equal(nav.rows.length, 1);
  assert.ok(nav.rows[0].units.length <= 25, "the single row still fits K_hard");
});
