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

test("`#624 voice-page-modularity`: the fold model lays a text out and counts what a fold hides at each width, from the module alone", () => {
  const { foldPieces, foldShows, elisionLabels, FOLD_WIDTHS_EM, forgetElisionCounts } = ROWS;
  const text = Array(200).fill("nnnnn").join(" ");
  const laid = foldPieces(text);
  assert.equal(laid.words, 200);
  assert.equal(laid.chars, 1000);
  // More lines shown, more words shown; a wider line, more words shown.
  const at = (width, lines) => foldShows(laid, width, lines).words;
  assert.ok(at(28.5, 3) < at(28.5, 8) && at(28.5, 3) < at(45, 3));
  const labels = elisionLabels(text, 3);
  assert.equal(labels.length, FOLD_WIDTHS_EM.length);
  assert.match(labels[0], /^\(~\d+ more words\)$/);
  assert.equal(elisionLabels(text, 3), labels, "a count held for the same text and lines was made again");
  forgetElisionCounts();
  assert.notEqual(elisionLabels(text, 3), labels, "a forgotten count was still held");
  assert.deepStrictEqual([...elisionLabels(null, 3)], FOLD_WIDTHS_EM.map(() => ""));
  // A table's words are a block drawn whole: shown, all of them, when it starts inside the shown lines.
  assert.deepStrictEqual(foldShows(foldPieces([["w", "a b c"]]), 28.5, 3), { words: 3, chars: 3 });
});

test("`#624 voice-page-modularity`: which messages are drawn as one row, from the module alone", () => {
  const { joinsGroup, isNoise, GLOM_WINDOW_MS } = ROWS;
  const at = (ms) => new Date(Date.UTC(2026, 9, 12, 12, 0, 0) + ms).toISOString();
  const first = { id: "1", author_id: "a", timestamp: at(0), content: "half" };
  const second = { id: "2", author_id: "a", timestamp: at(GLOM_WINDOW_MS), content: "of it" };
  assert.equal(joinsGroup(first, second, true), true);
  assert.equal(joinsGroup(first, second, false), false, "joined with combining turned off");
  assert.equal(joinsGroup(first, { ...second, timestamp: at(GLOM_WINDOW_MS + 1) }, true), false);
  assert.equal(joinsGroup(first, { ...second, author_id: "b" }, true), false);
  assert.equal(joinsGroup(first, { ...second, timestamp: "" }, true), false, "joined on no evidence of when");
  assert.equal(joinsGroup({ ...first, noise: true }, second, true), false, "a placeholder joined its answer");
  assert.equal(joinsGroup(null, second, true), false);
  assert.deepStrictEqual([isNoise({ noise: true }), isNoise({}), isNoise(null)], [true, false, false]);
});
