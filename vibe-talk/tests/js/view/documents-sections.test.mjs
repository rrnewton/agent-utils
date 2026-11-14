// Tests for the Documents navigator's section model (`#1508
// documents-scrubber-multilevel` for the tree and numbers, `#1541
// documents-nav-outline-tree` for the scrubber rows): the normalized
// section tree over the ONE block model (`blocks.ts`), its numbers
// (title rule, skipped levels, Introduction, the author's numbers
// winning), and the scrubber's you-are-here unfolding — chunk order
// (a table between its neighbours), one block per chunk, path-only
// expansion, and "…" windowing within the width cap. Pure over the
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

test("scrubber: cell cap comes from the width (24px target)", () => {
  assert.equal(SECTIONS.scrubberMaxCells(360), 15);
  assert.equal(SECTIONS.scrubberMaxCells(390), 16);
  assert.equal(SECTIONS.scrubberMaxCells(720), 30);
  assert.equal(SECTIONS.scrubberMaxCells(96), 5);
});

test("scrubber: windowing keeps the focus, one gap per hidden run, within cap", () => {
  for (const count of [1, 5, 16, 40, 119]) {
    for (const cap of [3, 5, 9, 16]) {
      for (const focus of [0, 1, Math.floor(count / 2), count - 1]) {
        const seq = SECTIONS.windowIndices(count, focus, cap);
        assert.ok(seq.length <= Math.max(cap, 1) || count <= cap,
          `count ${count} cap ${cap} focus ${focus}: ${seq.length} cells`);
        if (count > cap) assert.ok(seq.length <= cap, `count ${count} cap ${cap}: ${seq.length} cells over cap`);
        const kept = seq.filter((e) => "index" in e).map((e) => e.index);
        assert.ok(kept.includes(Math.min(focus, count - 1)), "the focus is kept");
        assert.deepEqual(kept, [...kept].sort((a, b) => a - b), "kept indices stay in order");
        for (let i = 1; i < seq.length; i += 1) {
          assert.ok(!("gap" in seq[i] && "gap" in seq[i - 1]), "never two gaps in a row");
        }
        for (const entry of seq) {
          if ("gap" in entry) assert.ok(!kept.includes(entry.gap), "a gap names a hidden index");
        }
      }
    }
  }
  // Everything fits: no gaps at all.
  assert.deepEqual(SECTIONS.windowIndices(4, 2, 9), [{ index: 0 }, { index: 1 }, { index: 2 }, { index: 3 }]);
});

function flattenSections(cells) {
  const out = [];
  for (const cell of cells) {
    out.push(cell);
    if (cell.kind === "section" && cell.children !== null) out.push(...flattenSections(cell.children));
  }
  return out;
}

test("scrubber: you-are-here unfolds only the path to the current section", () => {
  const { blocks, model: built } = model(bigDocument());
  const inDeep = built.sections.find((s) => s.title === "Section 3.2");
  const nav = SECTIONS.buildScrubberNav(blocks, built, inDeep.start + 5, 390);
  assert.equal(nav.address.startsWith("3.2 ¶"), true, `address is ${nav.address}`);
  assert.equal(built.sections[nav.currentSection].number, "3.2");
  const flat = flattenSections(nav.roots);
  const nodes = flat.filter((c) => c.kind === "section");
  // Exactly one current, and it is 3.2.
  assert.deepEqual(nodes.filter((n) => n.current).map((n) => n.number), ["3.2"]);
  // Expanded (children drawn) exactly where on the path with children:
  // section 3 is an ancestor; 3.2 itself is a leaf.
  assert.deepEqual(nodes.filter((n) => n.children !== null).map((n) => n.number), ["3"]);
  assert.deepEqual(nodes.filter((n) => n.ancestor).map((n) => n.number), ["3"]);
  // The siblings unfolding showed are present around the current section.
  const parent3 = nodes.find((n) => n.number === "3");
  assert.deepEqual(
    parent3.children.filter((c) => c.kind === "section").map((c) => c.number),
    ["3.1", "3.2", "3.3"]);
  // A non-ancestor section stays folded.
  const folded = nodes.find((n) => n.number === "2");
  if (folded !== undefined) assert.equal(folded.children, null);
  // Document order: section starts strictly increase down the flattening,
  // and every label names its exact tap target.
  const starts = nodes.map((n) => n.start);
  assert.deepEqual(starts, [...starts].sort((a, b) => a - b));
  assert.equal(new Set(starts).size, starts.length);
  for (const node of nodes) assert.equal(node.start, built.sections[node.sectionIndex].start);
  // The whole section row fits the width cap (labels plus gap cells).
  assert.ok(flat.length <= nav.maxCells, `section row holds ${flat.length} cells, cap ${nav.maxCells}`);
  assert.ok(nav.chunks.length <= nav.maxCells, "chunk row fits the cap");
});

test("scrubber: a huge leaf's chunks window with gaps, current kept, ends kept", () => {
  const { blocks, model: built } = model(bigDocument());
  const huge = built.sections.find((s) => s.title === "Section 3.2");
  const at = huge.start + 60;
  const nav = SECTIONS.buildScrubberNav(blocks, built, at, 390);
  const range = SECTIONS.sectionChunkRange(built, built.sections.indexOf(huge));
  assert.equal(range.start, huge.start);
  assert.equal(range.end, huge.end);
  assert.ok(range.end - range.start > 100, "the huge leaf holds over a hundred chunks");
  assert.ok(nav.chunks.length <= nav.maxCells, `chunk row holds ${nav.chunks.length} cells`);
  assert.ok(nav.chunks.some((c) => c.kind === "gap"), "hidden chunks stand as gap cells");
  const shown = nav.chunks.filter((c) => c.kind === "chunk");
  // One chunk is exactly one block, labelled by its local place.
  for (const cell of shown) assert.equal(cell.local, cell.block - range.start + 1);
  assert.deepEqual(shown.map((c) => c.block), shown.map((c) => c.block).sort((a, b) => a - b));
  const current = shown.filter((c) => c.current);
  assert.deepEqual(current.map((c) => c.block), [at], "the current chunk is shown, once");
  assert.equal(shown[0].block, range.start, "the first chunk is kept");
  assert.equal(shown[shown.length - 1].block, range.end - 1, "the last chunk is kept");
  // A gap taps to its first hidden block, never to a shown one.
  for (const cell of nav.chunks) {
    if (cell.kind === "gap") assert.ok(!shown.some((c) => c.block === cell.block));
  }
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
  const nav = SECTIONS.buildScrubberNav(blocks, built, tableAt, 390);
  const shown = nav.chunks.filter((c) => c.kind === "chunk");
  assert.deepEqual(shown.map((c) => c.block), shown.map((c) => c.block).sort((a, b) => a - b),
    "chunks stay in document order");
  const tableCell = shown.find((c) => c.block === tableAt);
  assert.ok(tableCell !== undefined && tableCell.current, "the table chunk is shown and current");
  const place = shown.indexOf(tableCell);
  assert.equal(shown[place - 1].block, tableAt - 1, "the chunk before the table is the paragraph before it");
  assert.equal(shown[place + 1].block, tableAt + 1, "the chunk after the table is the paragraph after it");
});

test("scrubber: a parent section's own chunks stop at its first subsection", () => {
  const { blocks, model: built } = model(bigDocument());
  const parent = built.sections.find((s) => s.title === "Section 3");
  const range = SECTIONS.sectionChunkRange(built, built.sections.indexOf(parent));
  const firstChild = built.sections[parent.children[0]];
  assert.equal(range.start, parent.start);
  assert.equal(range.end, firstChild.start);
  // Standing on the parent's heading, the chunk row is that own range.
  const nav = SECTIONS.buildScrubberNav(blocks, built, parent.start, 390);
  for (const cell of nav.chunks) {
    const block = cell.block;
    assert.ok(block >= range.start && block < range.end, `chunk ${block} inside the parent's own range`);
  }
});

test("scrubber: a document without headings gets a chunk row alone", () => {
  const markdown = Array.from({ length: 80 }, (_, i) => `Paragraph number ${i} carrying enough words to weigh in the balance of the scrubber row.`).join("\n\n");
  const { blocks, model: built } = model(markdown);
  const nav = SECTIONS.buildScrubberNav(blocks, built, 40, 360);
  assert.deepEqual(nav.roots.map((c) => c.number), ["0"], "the lone Introduction is the section row");
  assert.equal(nav.currentSection, 0, "the Introduction holds the position");
  assert.ok(nav.chunks.length <= nav.maxCells, "the chunk row still fits the cap");
  assert.ok(nav.chunks.some((c) => c.kind === "chunk" && c.current && c.block === 40));
});
