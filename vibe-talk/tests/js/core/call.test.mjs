// Tests for web/src/core/call/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** A call's reconnects and the words its seams say (web/src/core/call). */
const CALL = await moduleForTests("src/core/call/index.ts");

test("`#624 voice-page-modularity`: when a call reconnects after a lost turn, and when it ends, from the module alone", () => {
  const { reconnectAsked, reconnectVerdict, LOST_TURN, RECONNECT_MIN_GAP_MS, RECONNECTS_PER_CALL_MAX } = CALL;
  const call = { protocol: "vibe-talk-v1", v1Ready: true, reconnects: 0, lastReconnectAt: 0 };
  const said = "turn 4 never started; reconnect.";
  assert.equal(reconnectAsked(call, said), true);
  assert.equal(reconnectAsked(call, "turn 4 never started"), false, "an error that did not ask reconnected");
  assert.equal(reconnectAsked({ ...call, v1Ready: false }, said), false, "a session that never started reconnected");
  assert.equal(reconnectAsked({ ...call, protocol: "elevenlabs" }, said), false, "the hosted vendor's protocol reconnected");
  assert.equal(LOST_TURN.exec(said)[1], "4");
  assert.equal(reconnectVerdict(call, 1000), "reconnect");
  assert.equal(reconnectVerdict({ ...call, reconnects: 1, lastReconnectAt: 1000 }, 1000 + RECONNECT_MIN_GAP_MS - 1), "soon");
  assert.equal(reconnectVerdict({ ...call, reconnects: 1, lastReconnectAt: 1000 }, 1000 + RECONNECT_MIN_GAP_MS), "reconnect");
  assert.equal(reconnectVerdict({ ...call, reconnects: RECONNECTS_PER_CALL_MAX, lastReconnectAt: 0 }, 1e9), "count");
});

test("`#624 voice-page-modularity`: what a call's seams and the line under it say of its recap, from the module alone", () => {
  const { boundaryFor, callRecapLine, recapChip, recapWords, seamDetailFor, reconnectFreshWords, STORED_SEAM_DETAIL } = CALL;
  const fresh = (why) => `the page's reason for ${why}`;
  const sent = { state: "sent", words: 1234, lines: 9, dropped: 2, transport: "replay" };
  assert.deepStrictEqual([recapWords(sent), recapWords({ ...sent, words: 0 }), recapWords({ state: "sent" })], [1234, null, null]);
  assert.deepStrictEqual([recapChip(sent), recapChip({ state: "none", why: "off" }), recapChip({ state: "none", why: "refused" })],
    ["1,234-word recap sent", null, "recap refused"]);
  assert.equal(callRecapLine(sent, "accepted", fresh), "Caught the agent up with 1,234 words from earlier; 2 older lines did not fit");
  assert.equal(callRecapLine({ ...sent, transport: "profile", dropped: 0 }, "waiting", fresh), "Handing the agent 1,234 words from earlier");
  assert.equal(callRecapLine({ state: "none", why: "empty" }, null, fresh), "Started fresh: nothing was said aloud earlier");
  assert.equal(callRecapLine({ state: "none", why: "unsupported" }, null, fresh), "Started fresh: the page's reason for unsupported");
  assert.equal(callRecapLine({ state: "none", why: "empty", reconnected: true }, null, fresh),
    "Reconnected after a lost turn: started fresh, as nothing said in this call was recorded to send");
  assert.equal(reconnectFreshWords("too_long"), "the earlier conversation was too long to send", "the reconnect table lost the call table's reasons");
  // A seam: the resuming wording only where a recap went; the stored record's sentence with no cause.
  assert.notEqual(seamDetailFor("closed", true), seamDetailFor("closed", false));
  assert.equal(seamDetailFor("no such cause", false), seamDetailFor("ended", false));
  assert.equal(boundaryFor(null, null).detail, STORED_SEAM_DETAIL);
  assert.equal(boundaryFor(sent, null).detail, seamDetailFor("ended", true));
  assert.equal(boundaryFor({ state: "none", why: "replaced_session" }, null).detail, seamDetailFor("replaced", false));
  assert.equal(boundaryFor({ state: "none", why: "empty" }, "closed").detail, seamDetailFor("closed", false));
});

test("`#624 voice-page-modularity`: what a reconnect and a draft's fate say in the transcript, from the module alone", () => {
  const { reconnectSeamDetail, notSentDetail, spokenOutcomeKind, spokenYesSeam } = CALL;
  assert.match(reconnectSeamDetail("recap", "typed"), /sent this call's record, and your message went again/);
  assert.match(reconnectSeamDetail("fresh", "none"), /has seen nothing above this line/);
  assert.equal(reconnectSeamDetail("fresh", "nothing"), undefined, "a pair no sentence names was given one");
  assert.match(notSentDetail("withdrawn"), /^You tapped Don't send/);
  assert.equal(notSentDetail("declined"), spokenYesSeam("declined").detail, "a spoken no did not say what the spoken yes's table says");
  assert.equal(notSentDetail("something new"), notSentDetail("lapsed"));
  assert.deepStrictEqual([spokenOutcomeKind("refused"), spokenOutcomeKind("mystery")], ["refused", "not_armed"]);
  assert.equal(spokenYesSeam("sent").label, "Sent by voice");
  assert.match(spokenYesSeam("gone").detail, /no draft was waiting/);
});
