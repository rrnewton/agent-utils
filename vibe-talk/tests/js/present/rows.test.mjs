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

test("`#624 voice-page-modularity`: runs of read messages, and the ones the reader opened, from the module alone", () => {
  const { createReadRuns, READ_RUN_MIN_ROWS } = ROWS;
  assert.equal(READ_RUN_MIN_ROWS, 2);
  const runs = createReadRuns();
  // Rows a, b, c read; d unread; e, f read; g last. Each row stands for its own id.
  const rows = ["a", "b", "c", "d", "e", "f", "g"];
  const read = new Set(["a", "b", "c", "e", "f", "g"]);
  const plan = () => runs.plan(rows, "k", (row) => read.has(row), (row) => [row]);
  const roles = (placed) => rows.map((row) => (placed.get(row) || { role: "-" }).role.slice(0, 1)).join("");
  // The last row never joins, so e and f are a run of two and g stands alone.
  assert.equal(roles(plan()), "hmm-hm-");
  assert.equal(plan().get("a").count, 3);
  runs.open("k", ["b"]);
  assert.equal(roles(plan()), "----hm-", "an opened run collapsed");
  assert.deepStrictEqual([...runs.opened("k")].sort(), ["a", "b", "c"], "what joined an opened run was not remembered");
  runs.close("k", ["a", "b", "c"]);
  assert.equal(roles(plan()), "hmm-hm-");
  runs.open("k", ["e"]);
  runs.forget("k");
  assert.equal(runs.opened("k"), undefined);
  // One folded away by hand collapses on its own, even last.
  assert.equal(roles(runs.plan(rows, "k", (row) => read.has(row), (row) => [row], new Set(["g"]))), "hmm-hmm");
  runs.open("k", ["a"]);
  runs.clear();
  assert.equal(runs.opened("k"), undefined);
});

test("`#624 voice-page-modularity`: a card's lines by the server's proxy, and a thread's facts, from the module alone", () => {
  const { wrapCardLine, cardProxyLines, proxyLinesOf, foldedCardText, firstLineOf, threadChoice, threadFacts, CARD_LINE_CHARS } = ROWS;
  assert.equal(CARD_LINE_CHARS, 36);
  assert.deepStrictEqual(wrapCardLine("x".repeat(73)).map(({ text, cut }) => [text.length, cut]), [[36, true], [36, true], [1, false]]);
  assert.equal(cardProxyLines(proxyLinesOf("one\n\n  two   words  \n")), 2);
  assert.equal(foldedCardText(" a  b \n\n c "), "a b\nc");
  assert.deepStrictEqual([firstLineOf("\n  first line \nsecond", 40), firstLineOf("abcdef", 4)], ["first line", "abc…"]);
  const choice = threadChoice(7, { id: "7", reply_count: 3, updated_at: new Date(Date.now() - 5 * 60e3).toISOString(),
    root: { id: "r", timestamp: new Date(Date.now() - 3600e3).toISOString() } });
  assert.deepStrictEqual([choice.id, choice.count, choice.root.id], ["7", 3, "r"]);
  assert.equal(threadFacts(choice), "5m · 3");
  assert.equal(threadFacts({ ...choice, count: 0 }), "5m", "a count of nought was printed");
});

test("`#624 voice-page-modularity`: a new thread's colour stands clear of its neighbours, and a message's text, from the module alone", () => {
  const { hueApart, spreadThreadHue, newestThreadHues, THREAD_HUE_APART, THREAD_HUE_BESIDE, rawTextOf, renderedHtmlOf, messageLook, BLANK_TEXT } = ROWS;
  assert.deepStrictEqual([hueApart(10, 350), hueApart(0, 180), hueApart(90, 90)], [20, 180, 0]);
  // The widest arc's middle when it clears; else the re-roll, which does.
  assert.equal(spreadThreadHue(200, [0, 100], { near: [0], beside: [100] }), 200);
  const rolled = spreadThreadHue(20, [0, 120, 240], { near: [0, 120, 240], beside: [0] });
  assert.ok(hueApart(rolled, 0) >= THREAD_HUE_BESIDE && [120, 240].every((hue) => hueApart(rolled, hue) >= THREAD_HUE_APART), `${rolled}`);
  assert.deepStrictEqual(Object.keys(newestThreadHues([["a", [1, 10]], ["b", [2, 30]], ["c", [3, 20]]], 2)), ["b", "c"]);
  assert.deepStrictEqual([rawTextOf({ content: null }), rawTextOf({ content: "x" }), renderedHtmlOf({}), renderedHtmlOf({ content_html: "<p>x</p>" })],
    ["", "x", "", "<p>x</p>"]);
  assert.equal(messageLook({ id: 9, content: "abc", content_html: "<p>abc</p>" }), "9|3:10||null");
  assert.ok(BLANK_TEXT.test(" ​\n") && !BLANK_TEXT.test(" x "));
});

test("`#624 voice-page-modularity`: a row's text, last edit and reactions over the messages it stands for, from the module alone", () => {
  const { combinedContent, rowEdited, editedDrawn, rowReactions, reactionsDrawn } = ROWS;
  assert.equal(combinedContent([{ content: "one" }, { content: null }, { content: "three" }]), "one\n\nthree");
  assert.equal(rowEdited([{}, {}]), null);
  assert.deepStrictEqual(rowEdited([{ edited_timestamp: "garbage" }]), { at: null });
  assert.equal(rowEdited([{ edited_timestamp: "2026-10-01T00:00:00Z" }, { edited_timestamp: "2026-10-03T00:00:00Z" }]).at.toISOString(),
    "2026-10-03T00:00:00.000Z");
  assert.deepStrictEqual([editedDrawn({}), editedDrawn({ edited_timestamp: "t" })], [null, "t"]);
  const tallies = rowReactions([
    { reactions: [{ emoji: "👍", count: 2 }, { emoji: "party", custom: true, custom_id: "9", count: 1, owner_reacted: true }] },
    { reactions: [{ emoji: "👍", count: 1, owner_reacted: true }, { emoji: "", count: 3 }, { emoji: "x", count: 0 }] },
  ]);
  assert.deepStrictEqual(tallies, [{ emoji: "👍", custom: false, count: 3, own: true }, { emoji: "party", custom: true, count: 1, own: true }]);
  assert.deepStrictEqual(reactionsDrawn({ reactions: [{ emoji: "party", custom: true, count: 2 }] }), [[":party:", 2, false]]);
});

test("`#624 voice-page-modularity`: a quoted stretch as the head of a reply, from the module alone", () => {
  const { quoteBlock, QUOTE_MAX_CHARS } = ROWS;
  assert.equal(quoteBlock("  one  \r\n\n\n\ntwo\t three "), "> one\n>\n> two three\n\n");
  assert.equal(quoteBlock(" \n "), "");
  const long = quoteBlock(`${"word ".repeat(400)}`);
  const said = long.slice(2, -2);
  assert.ok(said.length <= QUOTE_MAX_CHARS && said.endsWith("word…"), "a long quote was not cut at a word with an ellipsis");
});

test("`#624 voice-page-modularity`: what replies are called, from the module alone", () => {
  const { countOf, replyMarkerName } = ROWS;
  assert.deepStrictEqual([countOf(1), countOf(3)], ["1 reply", "3 replies"]);
  assert.deepStrictEqual([replyMarkerName(1, 1), replyMarkerName(2, 5)],
    ["Jump to the reply to this message", "Jump to reply 2 of 5 to this message"]);
});

test("`#624 voice-page-modularity`: Main's messages cut into the runs before each card, from the module alone", () => {
  const { mainRunsAmong } = ROWS;
  const at = (minute) => ({ id: String(minute), timestamp: new Date(Date.UTC(2026, 9, 12, 12, minute)).toISOString() });
  const cards = [Date.UTC(2026, 9, 12, 12, 10), Date.UTC(2026, 9, 12, 12, 20)];
  assert.deepStrictEqual(mainRunsAmong([at(1), at(10), at(15), at(30)], cards).map((run) => run.map(({ id }) => id)),
    [["1"], ["10", "15"], ["30"]]);
  assert.deepStrictEqual(mainRunsAmong([], []), [[]]);
});

test("`#728 vt-quote-read-threshold`: how much of a message a reply's opening quote covers, and whether the reply marks it read", () => {
  const { quotedHead, quoteShare, quoteMarksRead, quoteBlock } = ROWS;
  assert.equal(quotedHead("> one line\n> two\n\nthe answer"), "one line two");
  assert.equal(quotedHead(">\n> after a blank\nanswer"), "after a blank");
  assert.equal(quotedHead("no quote\n> later"), "", "a quote further down is not the reply's opening quote");
  assert.equal(quotedHead(`${quoteBlock(`${"word ".repeat(400)}`)}answer`).endsWith("…"), false, "a cut quote's … was counted");
  const whole = "a".repeat(95) + " " + "b".repeat(4);
  assert.equal(quoteShare("> bbbb\n\nok", whole), 0.04);
  assert.equal(quoteShare("> anything\n\nok", "   "), null, "a message with no words was covered");
  assert.equal(quoteShare("no quote", whole), null);
  assert.equal(quoteShare(`> ${whole} and more\n\nok`, whole), 1, "a share past the whole is the whole");
  assert.equal(quoteShare(">   spaced \n>  out\n\nok", "spaced out entirely"), "spaced out".length / "spaced out entirely".length,
    "white space was counted as written rather than as words");
  assert.equal(quoteMarksRead(null, 5), true, "a reply quoting nothing left its message unread");
  assert.equal(quoteMarksRead(0.04, 5), false);
  assert.equal(quoteMarksRead(0.05, 5), true, "a quote of exactly the share left its message unread");
  assert.equal(quoteMarksRead(0, 0), true, "at 0% a reply left its message unread");
});
