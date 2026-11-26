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

test("`#2056`: which read-aloud failures a bounded retry may answer, from the module alone", () => {
  const { isRecoverableReadFailure } = SPEECH_PIECES;
  assert.deepStrictEqual(
    ["lost", "malformed", "no_audio"].map((reason) => isRecoverableReadFailure(reason)),
    [true, true, true],
    "a recoverable class would not be retried",
  );
  assert.deepStrictEqual(
    ["silent", "refused", "user_stop", "superseded", "jump", ""].map((reason) => isRecoverableReadFailure(reason)),
    [false, false, false, false, false, false],
    "a non-recoverable class, or no failure at all, would be retried",
  );
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

test("`#624 voice-page-modularity`: audio as bytes, and a read cut for a device voice, from the module alone", () => {
  const { downsampleTo, floatToPcm16, joinBytes, wavLayout, browserSpeechChunks, BROWSER_SPEECH_CHUNK_CHARS } = SPEECH_PIECES;
  assert.deepStrictEqual([...downsampleTo(new Float32Array([0, 0.5, 1, 0.5]), 4, 2)], [0, 1]);
  assert.deepStrictEqual([...floatToPcm16(new Float32Array([-2, -1, 0, 1, 2]))], [-32768, -32768, 0, 32767, 32767]);
  assert.deepStrictEqual([...joinBytes(new Uint8Array([1]), new Uint8Array([2, 3]))], [1, 2, 3]);
  // A minimal 16-bit PCM WAV header: RIFF, WAVE, a fmt chunk, then data.
  const header = new Uint8Array(44);
  const view = new DataView(header.buffer);
  [..."RIFF"].forEach((c, i) => (header[i] = c.charCodeAt(0)));
  [..."WAVEfmt "].forEach((c, i) => (header[8 + i] = c.charCodeAt(0)));
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, 24000, true);
  view.setUint16(34, 16, true);
  [..."data"].forEach((c, i) => (header[36 + i] = c.charCodeAt(0)));
  assert.deepStrictEqual(wavLayout(header), { channels: 1, rate: 24000, dataAt: 44 });
  assert.equal(wavLayout(header.subarray(0, 30)), null, "a header not yet whole was read");
  const long = `${"A sentence that is long enough to stand. ".repeat(10)}`;
  const chunks = browserSpeechChunks(long);
  assert.ok(chunks.length > 1 && chunks.every((chunk) => [...chunk].length <= BROWSER_SPEECH_CHUNK_CHARS));
  assert.equal(chunks.join(" "), long.trim());
});

test("`#1800 vibe-talk-read-aloud-chunk-start-clipping`: a cold chunk start gets the pre-roll, a gapless one gets none", () => {
  const { chunkStartSilenceMs, silenceFrameCount, DEFAULT_READ_PREROLL_MS } = SPEECH_PIECES;
  assert.equal(DEFAULT_READ_PREROLL_MS, 300, "the default pre-roll is not 300 ms");
  assert.equal(chunkStartSilenceMs(true, DEFAULT_READ_PREROLL_MS), 300, "a cold start was given no silence");
  assert.equal(chunkStartSilenceMs(false, DEFAULT_READ_PREROLL_MS), 0, "a gapless start was padded");
  assert.equal(chunkStartSilenceMs(true, 0), 0, "a pre-roll of 0 did not turn the padding off");
  assert.equal(silenceFrameCount(300, 48000), 14400);
  assert.equal(silenceFrameCount(0, 48000), 0);
  assert.equal(silenceFrameCount(300, 1000), 300);
});

test("`#624 voice-page-modularity`: the speech hold reads the server's level and cap, keeps its defaults for anything unusable, and measures a buffer in dBFS", () => {
  const { speechHoldFrom, bufferDbfs, SPEECH_HOLD_DBFS_DEFAULT, SPEECH_HOLD_CAP_MS_DEFAULT } = SPEECH_PIECES;
  const defaults = { levelDbfs: SPEECH_HOLD_DBFS_DEFAULT, capMs: SPEECH_HOLD_CAP_MS_DEFAULT };
  assert.deepStrictEqual({ ...speechHoldFrom(null) }, defaults);
  assert.deepStrictEqual({ ...speechHoldFrom({ level_dbfs: "loud", cap_ms: -1 }) }, defaults);
  assert.deepStrictEqual({ ...speechHoldFrom({ level_dbfs: -30, cap_ms: 0 }) }, { levelDbfs: -30, capMs: 0 }, "a cap of 0 means never wait");
  assert.deepStrictEqual({ ...speechHoldFrom({ level_dbfs: 6, cap_ms: 9e9 }) }, { levelDbfs: 0, capMs: 600000 });
  assert.equal(bufferDbfs(new Float32Array(4)), -Infinity);
  assert.equal(bufferDbfs(new Float32Array(0)), -Infinity);
  assert.equal(bufferDbfs(new Float32Array([1, -1, 1, -1])), 0);
  assert.ok(Math.abs(bufferDbfs(new Float32Array([0.1, -0.1])) + 20) < 1e-5, "a tenth of full scale is -20 dBFS (to float32 precision)");
});

test("`#2266 vibe-talk-doc-reader-navigator-campaign`: model blocks and prepared pieces map by element groups, both ways", () => {
  const { documentBlockGroups, documentGroupsAlign, documentListedBlockForModel,
    documentModelBlockForListed, documentModelPieceForListed } = SPEECH_PIECES;
  // The diagnosis canary in miniature: 9 model blocks; the long paragraph is
  // 2 server pieces and the table is one piece a body row (13 listed).
  const kinds = ["heading", "heading", "paragraph", "table", "heading", "paragraph", "paragraph", "heading", "paragraph"];
  const listed = [];
  const push = (element, kind, pieces) => {
    for (let piece = 0; piece < pieces; piece += 1) listed.push({ kind, element, piece, pieces, spoken: true });
  };
  push(0, "heading", 1); push(1, "heading", 1); push(2, "paragraph", 2); push(3, "table", 4);
  push(4, "heading", 1); push(5, "paragraph", 1); push(6, "paragraph", 1);
  push(7, "heading", 1); push(8, "paragraph", 1);
  assert.equal(listed.length, 13);

  const groups = documentBlockGroups(listed);
  assert.deepStrictEqual(groups.map((group) => group.members.length), [1, 1, 2, 4, 1, 1, 1, 1, 1]);
  assert.equal(documentGroupsAlign(groups, kinds), true, "the canary cuts do not align");

  // Reporting: a listed piece names its model block (table rows are block 3).
  assert.equal(documentModelBlockForListed(listed, kinds, 7), 3, "the last table row is not the table block");
  assert.equal(documentModelBlockForListed(listed, kinds, 8), 4, "the heading after the table is not model block 4");
  assert.equal(documentModelBlockForListed(listed, kinds, 3), 2, "the long paragraph's second piece is not its block");
  assert.deepStrictEqual(documentModelPieceForListed(listed, kinds, 6), { block: 3, piece: 2 });
  assert.deepStrictEqual(documentModelPieceForListed(listed, kinds, 3), { block: 2, piece: 1 });

  // Seeking: a model block (and piece) names its listed piece — the inverse.
  assert.equal(documentListedBlockForModel(listed, kinds, 4), 8, "section after the table sought in listed space");
  assert.equal(documentListedBlockForModel(listed, kinds, 3, 2), 6, "the table's third row was not sought");
  assert.equal(documentListedBlockForModel(listed, kinds, 2, 1), 3);
  assert.equal(documentListedBlockForModel(listed, kinds, 2, 99), 3, "a piece past the block did not clamp inside it");
  assert.equal(documentListedBlockForModel(listed, kinds, 99), 12, "a block past the end did not clamp to the last block");
  assert.equal(documentListedBlockForModel(listed, kinds, -1), 0);
  for (let at = 0; at < listed.length; at += 1) {
    const model = documentModelPieceForListed(listed, kinds, at);
    assert.equal(documentListedBlockForModel(listed, kinds, model.block, model.piece), at,
      `listed ${at} does not round-trip through model space`);
  }

  // Unaligned cuts (a front-matter block only one side counts): fall back to
  // the server's drawn element, never silence and never the top.
  const shifted = ["paragraph", ...kinds];
  assert.equal(documentGroupsAlign(groups, shifted), false);
  assert.equal(documentModelBlockForListed(listed, shifted, 8), 4, "unaligned reporting lost the drawn element");
  assert.equal(documentListedBlockForModel(listed, shifted, 4), 8, "unaligned seeking missed its drawn block");
  assert.equal(documentListedBlockForModel(listed, shifted, 3, 1), 5);

  // No model cut: the prepared list's own numbering, as before.
  assert.equal(documentModelBlockForListed(listed, undefined, 7), 7);
  assert.equal(documentListedBlockForModel(listed, undefined, 7), 7);
  assert.equal(documentListedBlockForModel([], kinds, 0), -1, "an empty list sought somewhere");

  // A pre-pieces server (no `element`): one listed block a drawn block.
  const plain = kinds.map((kind) => ({ kind, spoken: true }));
  assert.equal(documentGroupsAlign(documentBlockGroups(plain), kinds), true);
  assert.equal(documentListedBlockForModel(plain, kinds, 4), 4);
  assert.equal(documentModelBlockForListed(plain, kinds, 4), 4);
});
