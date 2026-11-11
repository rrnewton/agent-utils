// Tests for web/src/view/channel/textselection.ts on its own (`#624 voice-page-modularity`):
// a live selection read by its ends into the row's words, the kept selection across a
// rebuild (and a pointer down letting it go), and putting it back on the rebuilt row. The
// page over the same factory is in ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");

function makeSelection() {
  const target = new EventTarget();
  const ranges = [];
  const selection = {
    ranges,
    get rangeCount() { return this.ranges.length; },
    get isCollapsed() { return this.ranges.length === 0; },
    getRangeAt(index) { return this.ranges[index]; },
    removeAllRanges: () => { ranges.length = 0; },
    addRange: (range) => { ranges.splice(0, ranges.length, range); },
  };
  const doc = Object.assign(target, {
    createRange: () => ({
      startContainer: null, startOffset: 0, endContainer: null, endOffset: 0,
      setStart(node, offset) { this.startContainer = node; this.startOffset = offset; },
      setEnd(node, offset) { this.endContainer = node; this.endOffset = offset; },
      toString() { return String(this.endContainer.textContent).slice(0, this.endOffset); },
    }),
  });
  globalThis.document = doc;
  globalThis.getSelection = () => selection;

  const list = { id: "discord-log", parentNode: null, children: [] };
  const area = {
    id: "scroll-area", parentNode: null, children: [list],
    querySelectorAll: () => list.children,
  };
  list.parentNode = area;
  const rowWith = (text) => {
    const row = { nodeType: 1, parentNode: list, childNodes: [], children: [] };
    const words = { nodeType: 3, textContent: text, parentElement: row, parentNode: row, childNodes: [] };
    row.childNodes.push(words);
    list.children.push(row);
    return { row, words };
  };
  const first = rowWith("hello world");
  const owner = CHANNEL.mountTextSelection({
    el: () => area,
    idsOf: (row) => (row === first.row || row?.datasetId === "m1" ? ["m1"] : []),
  });
  const select = (from, to) => {
    selection.ranges = [{
      startContainer: first.words, startOffset: from,
      endContainer: first.words, endOffset: to,
    }];
  };
  return { owner, doc, selection, area, list, first, rowWith, select };
}

test("a live selection is read by its ends, kept as it changes, and let go by the reader's hand", () => {
  const { owner, doc, selection, area, select } = makeSelection();
  assert.equal(owner.textSelectionIn(area), null, "nothing selected, nothing kept");
  select(2, 5);
  assert.deepEqual(owner.textSelectionInNow(area), { id: "m1", start: 2, end: 5 });
  doc.dispatchEvent(new Event("selectionchange"));
  selection.ranges = [];
  assert.deepEqual(owner.textSelectionIn(area), { id: "m1", start: 2, end: 5 },
    "the kept selection answers when the live one is gone");
  doc.dispatchEvent(new Event("pointerdown"));
  assert.equal(owner.textSelectionIn(area), null, "the reader's own hand let the kept selection go");
  owner.unmount();
  select(1, 3);
  doc.dispatchEvent(new Event("selectionchange"));
  selection.ranges = [];
  assert.equal(owner.textSelectionIn(area), null, "an unmounted mount keeps nothing new");
});

test("a kept selection goes back on the rebuilt row, by its words", () => {
  const { owner, selection, area, list, rowWith } = makeSelection();
  const rebuilt = rowWith("hello world");
  rebuilt.row.datasetId = "m1";
  list.children = [rebuilt.row];
  owner.restoreTextSelectionIn(area, { id: "m1", start: 2, end: 5 });
  assert.equal(selection.ranges.length, 1);
  assert.equal(selection.ranges[0].startContainer, rebuilt.words);
  assert.deepEqual([selection.ranges[0].startOffset, selection.ranges[0].endOffset], [2, 5]);
  owner.restoreTextSelectionIn(area, { id: "m9", start: 1, end: 2 });
  assert.equal(selection.ranges.length, 1, "a row that never comes back takes nothing");
  assert.deepEqual(owner.textSelectionIn(area), { id: "m1", start: 2, end: 5 },
    "the live selection still answers first");
  selection.ranges = [];
  assert.deepEqual(owner.textSelectionIn(area), { id: "m9", start: 1, end: 2 },
    "restoring keeps the selection it was given");
  owner.unmount();
});
