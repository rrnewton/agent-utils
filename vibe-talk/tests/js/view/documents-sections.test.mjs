// Tests for the Documents navigator's section model (`#1508
// documents-scrubber-multilevel` for the tree and numbers, `#1541
// documents-nav-outline-tree` for the scrubber rows): the normalized
// section tree over the ONE block model (`blocks.ts`), its numbers
// (title rule, skipped levels, Introduction, the author's numbers
// winning), and the scrubber's flat rows (`#2266`) — every section in order,
// chunk order (a table between its neighbours), one block per chunk.
// Pure over the
// blocks, held here without a reader.

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

test("sections: winning author numbers leave bare titles (`#2266`)", () => {
  const dotted = model("# T\n\n## 3. Alpha\n\nx\n\n## 4. Beta\n\ny\n");
  assert.deepEqual(dotted.model.sections.map((s) => [s.number, s.title]), [["3", "Alpha"], ["4", "Beta"]]);
  const deep = model("# T\n\n## One\n\n### 3.1 Deep\n\nx\n\n### 3.2 Deep two\n\ny\n");
  assert.deepEqual(deep.model.sections.map((s) => [s.number, s.title]),
    [["1", "One"], ["3.1", "Deep"], ["3.2", "Deep two"]], "a winning dotted number is not said twice");
  // A number that did not win its level stays part of the heading's words.
  const mixed = model("# T\n\n## 3. Alpha\n\nx\n\n## Beta\n\ny\n");
  assert.deepEqual(mixed.model.sections.map((s) => [s.number, s.title]), [["1", "3. Alpha"], ["2", "Beta"]]);
  assert.equal(SECTIONS.bareHeadingTitle("Plain heading"), "Plain heading");
  assert.equal(SECTIONS.bareHeadingTitle("2024 in review"), "2024 in review");
});

test("sections: a heading addresses as its section, its first block as paragraph 1 (`#2266`)", () => {
  const { model: built } = model("# T\n\n## Alpha\n\nFirst.\n\nSecond.\n\n## Beta\n\nThird.\n");
  const beta = built.sections.find((s) => s.title === "Beta");
  assert.equal(SECTIONS.blockAddress(built, beta.start), "2", "the heading itself is the section");
  assert.equal(SECTIONS.blockAddress(built, beta.start + 1), "2 ¶ 1", "the first body block is paragraph 1");
});

test("scrubber: every section stands flat in document order, one current (`#2266`)", () => {
  const { blocks, model: built } = model(bigDocument());
  const inDeep = built.sections.find((s) => s.title === "Section 3.2");
  const nav = SECTIONS.buildScrubberNav(blocks, built, inDeep.start + 5);
  assert.equal(nav.address.startsWith("3.2 ¶"), true, `address is ${nav.address}`);
  assert.equal(built.sections[nav.currentSection].number, "3.2");
  // Nothing folded, nothing hidden: the row is the section list itself.
  assert.deepEqual(nav.sections.map((c) => c.number), built.sections.map((s) => s.number));
  assert.deepEqual(nav.sections.map((c) => c.start), built.sections.map((s) => s.start));
  assert.deepEqual(nav.sections.filter((c) => c.current).map((c) => c.number), ["3.2"],
    "exactly one current section");
  for (const cell of nav.sections) {
    assert.equal(cell.start, built.sections[cell.sectionIndex].start, "a label names its exact tap target");
    assert.equal(cell.title, built.sections[cell.sectionIndex].title);
  }
});

test("scrubber: a huge leaf's chunks are all present, one tick a block (`#2266`)", () => {
  const { blocks, model: built } = model(bigDocument());
  const huge = built.sections.find((s) => s.title === "Section 3.2");
  const at = huge.start + 60;
  const nav = SECTIONS.buildScrubberNav(blocks, built, at);
  const range = SECTIONS.sectionChunkRange(built, built.sections.indexOf(huge));
  assert.equal(range.start, huge.start);
  assert.equal(range.end, huge.end);
  assert.ok(range.end - range.start > 100, "the huge leaf holds over a hundred chunks");
  assert.equal(nav.chunks.length, range.end - range.start, "every chunk stands in the row");
  assert.deepEqual(nav.chunks.map((c) => c.block),
    Array.from({ length: range.end - range.start }, (_unused, i) => range.start + i),
    "chunks stay in document order, one a block");
  assert.deepEqual(nav.chunks.filter((c) => c.current).map((c) => c.block), [at],
    "the current chunk is present, once");
  assert.equal(nav.chunks[0].address, "3.2", "the heading chunk addresses as the section");
  assert.equal(nav.chunks[1].address, "3.2 ¶ 1");
});

test("scrubber: a table's chunk lies between the chunks before and after it", () => {
  const markdown = [
    "# Field notes", "",
    "## Results", "",
    "Before the table there is a paragraph with plain words to read aloud.", "",
    "| Lever | Cost |", "| --- | --- |", "| Alpha | 10 |", "| Beta | 20 |", "",
    "After the table there is a paragraph with plain words to read aloud.", "",
  ].join("\n");
  const { blocks, model: built } = model(markdown);
  const tableAt = blocks.findIndex((b) => b.kind === "table");
  assert.ok(tableAt > 0, "the table is one block");
  assert.equal(blocks[tableAt - 1].text.startsWith("Before the table"), true);
  assert.equal(blocks[tableAt + 1].text.startsWith("After the table"), true);
  const nav = SECTIONS.buildScrubberNav(blocks, built, tableAt);
  const place = nav.chunks.findIndex((c) => c.block === tableAt);
  assert.ok(place > 0 && nav.chunks[place].current, "the table chunk is present and current");
  assert.equal(nav.chunks[place - 1].block, tableAt - 1, "the chunk before the table is the paragraph before it");
  assert.equal(nav.chunks[place + 1].block, tableAt + 1, "the chunk after the table is the paragraph after it");
});

test("scrubber: a parent section's own chunks stop at its first subsection", () => {
  const { blocks, model: built } = model(bigDocument());
  const parent = built.sections.find((s) => s.title === "Section 3");
  const range = SECTIONS.sectionChunkRange(built, built.sections.indexOf(parent));
  const firstChild = built.sections[parent.children[0]];
  assert.equal(range.start, parent.start);
  assert.equal(range.end, firstChild.start);
  const nav = SECTIONS.buildScrubberNav(blocks, built, parent.start);
  assert.equal(nav.chunks.length, range.end - range.start);
  for (const cell of nav.chunks) {
    assert.ok(cell.block >= range.start && cell.block < range.end,
      `chunk ${cell.block} inside the parent's own range`);
  }
});

test("scrubber: a document without headings is one Introduction over all its chunks", () => {
  const markdown = Array.from({ length: 80 }, (_, i) => `Paragraph number ${i} carrying enough words to weigh in the balance of the scrubber row.`).join("\n\n");
  const { blocks, model: built } = model(markdown);
  const nav = SECTIONS.buildScrubberNav(blocks, built, 40);
  assert.deepEqual(nav.sections.map((c) => c.number), ["0"], "the lone Introduction is the section row");
  assert.equal(nav.currentSection, 0, "the Introduction holds the position");
  assert.equal(nav.chunks.length, 80, "every chunk stands in the row");
  assert.ok(nav.chunks.some((c) => c.current && c.block === 40));
});
