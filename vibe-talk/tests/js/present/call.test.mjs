// Tests for web/src/present/call/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the same
// module, in a call, is tested in ../page/call.mjs and ../page/call-sending.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** What a voice call shows and says (web/src/present/call). */
const CALL_WORDS = await moduleForTests("src/present/call/index.ts");

test("`#624 voice-page-modularity`: a turn's words from the frames the voice service streams, from the module alone", () => {
  const { mergeHypothesis, shownSpoken, spokenWords, CUT_OFF_MARK } = CALL_WORDS;
  assert.deepStrictEqual(spokenWords("It's done, OK?"), ["it's", "done", "ok"]);
  // A cumulative hypothesis wins; a late repeat changes nothing; a correction replaces; a fragment appends.
  assert.equal(mergeHypothesis("send the", "send the report"), "send the report");
  assert.equal(mergeHypothesis("send the report", "the report"), "send the report");
  assert.equal(mergeHypothesis("send the report to Ada now", "send the report to Ida now"), "send the report to Ida now");
  assert.equal(mergeHypothesis("yes", "no"), "yes no", "a one-word difference was a correction of a short turn");
  assert.equal(mergeHypothesis("", "  hello  "), "hello");
  assert.equal(shownSpoken({ settled: "partly said", pending: null, cut: true }), `partly said ${CUT_OFF_MARK}`);
  assert.equal(shownSpoken({ settled: "", cut: true }), CUT_OFF_MARK);
});

test("`#624 voice-page-modularity`: what the agent is told of a draft, and the line for an ended call, from the module alone", () => {
  const { outcomeNotice, closeReason } = CALL_WORDS;
  assert.match(outcomeNotice({ kind: "sent", proposal: { channel_name: "#ops" } }, "tap"), /^Voice page: the user tapped Send, and the message was SENT to #ops\./);
  assert.match(outcomeNotice({ kind: "refused", reason: "too long" }, "voice"), /NOT sent \(too long\)/);
  assert.equal(outcomeNotice({ kind: "declined" }, "voice"), null, "the agent was told of a no it heard itself");
  assert.deepStrictEqual([closeReason(1000), closeReason(1006), closeReason(4000)],
    ["Call ended.", "The call ended — the connection dropped.", "The call ended unexpectedly. Settings has the details."]);
});

test("`#624 voice-page-modularity`: what a spoken answer says to \"Send it?\", and a read-back held word for word, from the module alone", () => {
  const { classifySpokenAnswer, isExactReadBack, answerWords, unsayableDraft, couldBeSpokenYes, namesWhereToSend } = CALL_WORDS;
  assert.deepStrictEqual(["yes please", "no, change it", "yes and tell him", "Send it?", "what time is it"].map(classifySpokenAnswer),
    // A question is never a yes, but "send" in it sounds like one.
    ["affirmative", "negative", "hint", "hint", "other"]);
  const shown = { channel_name: "lead team", text: "Ship at noon." };
  assert.equal(isExactReadBack(answerWords("Okay, to the lead team channel: ship at noon. Send it?"), shown), true);
  assert.equal(isExactReadBack(answerWords("To lead team: do not ship at noon. Send it?"), shown), false);
  assert.deepStrictEqual([unsayableDraft("Ship it at 10 o'clock, please."), unsayableDraft("see github.com"), unsayableDraft("<@U1> hi")],
    [null, "symbols", "mention"]);
  assert.deepStrictEqual([couldBeSpokenYes(["ok", "se"]), couldBeSpokenYes(["no"])], [true, false]);
  assert.deepStrictEqual([namesWhereToSend("send it to Bob"), namesWhereToSend("ok send it")], [true, false]);
});
