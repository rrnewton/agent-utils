// Tests for web/src/present/rows/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the same
// module, as its rows draw it, is tested in ../page/rows.mjs and its neighbours.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** How the page words and arranges its rows of messages (web/src/present/rows). */
const ROWS = await moduleForTests("src/present/rows/index.ts");

test("`#624 voice-page-modularity`: what a fold hides, and the lines for gaps and runs, from the module alone", () => {
  const { elisionLabel, gapWords, readRunText, cardGapWords, cardRunText, lineLabel } = ROWS;
  assert.deepStrictEqual([elisionLabel({ words: 9, chars: 54 }), elisionLabel({ words: 243, chars: 1400 }),
    elisionLabel({ words: 1, chars: 400 })], ["", "(~240 more words)", "(~400 more characters)"]);
  assert.equal(gapWords(1), "… 1 unmatched message …");
  // Each side of the dot held together by no-break spaces, and a word joiner after the range's dash.
  assert.equal(readRunText(12, "Oct 3 – 5"),
    "…\u00a012\u00a0read\u00a0messages\u00a0· Oct\u00a03\u00a0–\u2060\u00a05\u00a0…");
  assert.equal(lineLabel(readRunText(12, "Oct 3 – 5")), "12 read messages · Oct 3 – 5");
  assert.equal(cardGapWords(3, 2, false, true), "… at least 3 unmatched messages in 2 threads …");
  assert.equal(cardRunText(1, 2), "… 1 read thread and 2 read messages");
});

test("`#624 voice-page-modularity`: the Undo chip's words and a reaction's face, from the module alone", () => {
  const { undoName, undoLabel, UNDO_NAME_CHARS, reactionKey, reactionText } = ROWS;
  assert.equal(undoName("  two words  "), "two words");
  assert.equal(undoName("x".repeat(UNDO_NAME_CHARS + 5)), `${"x".repeat(UNDO_NAME_CHARS)}…`);
  assert.deepStrictEqual([undoLabel(null), undoLabel({ name: "Ship it", messages: [1] }), undoLabel({ messages: [1, 2] })],
    ["Undo", "Undo “Ship it”", "Undo Done (2)"]);
  assert.deepStrictEqual([reactionKey({ emoji: "👍" }), reactionKey({ emoji: "party", custom: true, custom_id: "9" })], ["u:👍", "c:9"]);
  assert.deepStrictEqual([reactionText({ emoji: "👍" }), reactionText({ emoji: "party", custom: true })], ["👍", ":party:"]);
});
