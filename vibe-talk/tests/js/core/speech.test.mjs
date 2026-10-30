// Tests for web/src/core/speech/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The speech module: what a read plays and how a block is cut (web/src/core/speech). */
const SPEECH_PIECES = await moduleForTests("src/core/speech/index.ts");

test("`#624 voice-page-modularity`: prepared read-aloud tickets end on either clock, from the module alone", () => {
  const { createPreparedSpeech, ticketLeftMs } = SPEECH_PIECES;
  let now = { mono: 100, wall: 5000 };
  const prepared = createPreparedSpeech({ now: () => now });
  const entry = { url: "/speech/a", until: { mono: 200, wall: 5100 }, blocks: null };
  prepared.keep("7", entry);
  assert.deepStrictEqual([prepared.has(7), prepared.size(), prepared.entry(7)], [true, 1, entry], "an id was not read as its string");
  assert.equal(ticketLeftMs(entry, now), 100);
  assert.equal(ticketLeftMs(entry, { mono: 150, wall: 5080 }), 20, "not the nearer of the two deadlines");
  // The phone slept: the monotonic clock stood still while the wall clock, and the ticket, went on.
  now = { mono: 150, wall: 5100 };
  assert.equal(prepared.entry("7"), null, "a ticket past its wall-clock deadline was played");
  assert.equal(prepared.has("7"), false, "an ended ticket was kept");
  prepared.keep("8", { ...entry, until: { mono: 150, wall: 9999 } });
  assert.equal(prepared.entry("8"), null, "a ticket past its monotonic deadline was played");
  // `get` keeps what is past its deadline; `forget` and `clear` drop it.
  prepared.keep("9", entry);
  assert.equal(prepared.get("9"), entry);
  prepared.forget("9");
  assert.equal(prepared.get(9), undefined);
  prepared.keep("10", entry);
  prepared.clear();
  assert.equal(prepared.size(), 0);
});

test("`#624 voice-page-modularity`: when a cut-off read tries again, and when a ticket can no longer carry it, from the module alone", () => {
  const { recoveryWait, ticketEnded, ticketTooShortForBlocks, READ_RECOVERY_WAITS_MS, BLOCK_READ_TICKET_MARGIN_MS } = SPEECH_PIECES;
  assert.deepStrictEqual(READ_RECOVERY_WAITS_MS.map((_, at) => recoveryWait(at)), READ_RECOVERY_WAITS_MS);
  assert.equal(recoveryWait(READ_RECOVERY_WAITS_MS.length), null, "a read tried again after its last try");
  const ticket = { until: { mono: 100, wall: 1000 } };
  assert.deepStrictEqual([ticketEnded(ticket, { mono: 99, wall: 999 }), ticketEnded(ticket, { mono: 100, wall: 0 }), ticketEnded(ticket, { mono: 0, wall: 1000 })],
    [false, true, true]);
  const entry = { url: "/u", until: { mono: BLOCK_READ_TICKET_MARGIN_MS, wall: 10 * BLOCK_READ_TICKET_MARGIN_MS }, blocks: null };
  assert.equal(ticketTooShortForBlocks(entry, { mono: 0, wall: 0 }), false);
  assert.equal(ticketTooShortForBlocks(entry, { mono: 1, wall: 0 }), true, "a ticket with less than the margin left started a read in blocks");
});

test("`read-chunk-balance`: a paragraph's sentences are cut into balanced pieces, as src/render.rs cuts them", () => {
  // The owner's paragraph: sentences of 5, 18, 30 and 8 words, read greedily as 53 and 8. The same
  // examples src/render.rs's `balance_sentences` test holds the server to.
  // The module's own functions (`#624 voice-page-modularity`): the page uses them through readPieces.
  const balance = (counts, alone) => SPEECH_PIECES.balanceSentences(counts, alone);
  const passes = SPEECH_PIECES.pieceSkewPasses;
  assert.equal(passes([53, 8]), false, "the greedy cut passes the balance metric");
  assert.equal(passes([23, 38]), true);
  assert.ok(passes([30, 60]) && !passes([29, 60]));
  assert.deepEqual(balance([5, 18, 30, 8], false), [2, 2]);
  assert.deepEqual(balance([5, 18, 30, 8], true), [2, 2]);
  assert.deepEqual(balance([15, 16, 16], false), [3]);
  assert.deepEqual(balance([15, 16, 16], true), [1, 1, 1]);
  assert.deepEqual(balance([6, 7], true), [2]);
  assert.deepEqual(balance([40, 1, 40], false), [1, 2]);
  assert.deepEqual(balance([60, 1, 60], false), [1, 1, 1]);
  assert.deepEqual(balance([], true), []);
});
