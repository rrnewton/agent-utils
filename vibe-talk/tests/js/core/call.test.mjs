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

test("`#624 voice-page-modularity`: a call's audio route, from its microphone to its output, from the module alone", () => {
  const { createAudioRoute } = CALL;
  const classify = (label) => (/airpods/i.test(label) ? "bluetooth" : /usb/i.test(label) ? "usb" : "other");
  const route = createAudioRoute({ classify });
  assert.equal(route.current(), null, "a route stood before any call");
  route.noteSink("desk");
  route.noteOutput("USB speaker");
  assert.equal(route.current(), null, "an output aimed between calls made a route");
  route.begin();
  assert.deepStrictEqual(route.current(), { inputKind: "other", outputKind: "other", inputLabel: "", outputLabel: "", sink: "" });
  route.noteInput("");
  route.settleInput("AirPods Pro");
  assert.deepStrictEqual([route.current().inputKind, route.current().inputLabel, route.current().outputKind],
    ["bluetooth", "AirPods Pro", "bluetooth"], "an unlabelled track did not take the chosen device's label and kind");
  route.noteInput("USB headset");
  route.settleInput("AirPods Pro");
  assert.deepStrictEqual([route.current().inputKind, route.current().inputLabel], ["usb", "USB headset"],
    "the chosen device's label replaced the one the track gave");
  route.noteSink("desk");
  route.noteOutput("AirPods Pro");
  assert.deepStrictEqual(route.current(),
    { inputKind: "usb", outputKind: "bluetooth", inputLabel: "USB headset", outputLabel: "AirPods Pro", sink: "desk" });
  const first = route.current();
  route.begin();
  assert.notEqual(route.current(), first, "a new call's route is the last call's object");
  route.set(null);
  assert.equal(route.current(), null);
  assert.throws(() => route.noteInput("USB headset"), TypeError, "a call that ended mid-open no longer throws at the next update");
  assert.equal(createAudioRoute({ classify }).current(), null, "two routes share their state");
});

test("`#624 voice-page-modularity`: the conversational voice as the provider describes it, from the module alone", () => {
  const { createVoiceProvider } = CALL;
  const voice = createVoiceProvider();
  const unnamed = { name: "voice provider", settings_url: null, instance_id: null };
  assert.deepStrictEqual(voice.current(), unnamed);
  voice.learn({ name: "  Example voice  ", settings_url: "https://voice.example/agent", instance_id: "agent_1" });
  assert.deepStrictEqual(voice.current(), { name: "Example voice", settings_url: "https://voice.example/agent", instance_id: "agent_1" });
  const before = voice.current();
  voice.learn({ name: "   ", settings_url: 7, instance_id: null });
  assert.deepStrictEqual(voice.current(), unnamed, "a blank name or a field that is not a string was kept");
  assert.equal(before.name, "Example voice", "a new description changed the old one in place");
  for (const described of [undefined, null, false, "Example voice"]) {
    voice.learn(described);
    assert.deepStrictEqual(voice.current(), unnamed, `${String(described)} described a voice`);
  }
  assert.deepStrictEqual(createVoiceProvider().current(), unnamed, "two descriptions share their state");
});

test("`#624 voice-page-modularity`: resuming's switch, the clause under the control, and a new call's payload, from the module alone", async () => {
  const { createCallResume, blankResumeLast } = CALL;
  const stored = new Map();
  const storage = {
    getItem: (key) => (stored.has(key) ? stored.get(key) : null),
    setItem: (key, value) => { stored.set(key, value); },
    removeItem: (key) => { stored.delete(key); },
  };
  const asked = [];
  const details = [];
  let answer = async () => ({ enabled: true, included: 3, dropped: 0, text: "earlier", words: 42 });
  let renders = 0;
  const resume = createCallResume({
    key: "resume-key",
    storage: () => storage,
    api: async (path) => { asked.push(path); return answer(); },
    addDetail: (text) => { details.push(text); },
    render: () => { renders += 1; },
  });
  assert.deepStrictEqual([resume.allowed(), resume.conversationId(), resume.last()], [false, null, blankResumeLast()]);
  assert.deepStrictEqual([resume.wanted(), resume.armed(), resume.note()], [false, false, "the agent starts fresh"]);
  // Off: nothing is asked of the server, and the screen is told.
  assert.equal(await resume.fetchPayload(), null);
  assert.deepStrictEqual([asked, renders, resume.last()], [[], 1, blankResumeLast()]);
  resume.setAllowed(true);
  resume.persist(true);
  assert.equal(stored.get("resume-key"), "on");
  resume.setConversationId("conv a/b");
  assert.deepStrictEqual([resume.wanted(), resume.armed(), resume.note()], [true, true, "the earlier conversation is replayed"]);
  // Armed: the payload is answered, and the outcome says it went.
  const payload = await resume.fetchPayload();
  assert.deepStrictEqual(asked, ["/api/v1/conversations/conv%20a%2Fb/replay"]);
  assert.equal(payload.text, "earlier");
  assert.deepStrictEqual(resume.last(), { ...blankResumeLast(), attempted: true, armed: true, words: 42 });
  // A conversation too long to send any of is not an empty one.
  answer = async () => ({ enabled: true, included: 0, dropped: 5, text: "" });
  assert.equal(await resume.fetchPayload(), null);
  assert.deepStrictEqual(resume.last(), { ...blankResumeLast(), attempted: true, dropped: 5 });
  assert.equal(resume.note(), "the agent starts fresh — the earlier conversation was too long to replay");
  answer = async () => ({ enabled: true, included: 0, dropped: 0, text: "" });
  assert.equal(await resume.fetchPayload(), null);
  assert.equal(resume.note(), "the agent starts fresh — there was nothing to replay");
  // A failed read degrades to a fresh call, said in the connection details and never thrown.
  answer = async () => { throw new Error("offline"); };
  assert.equal(await resume.fetchPayload(), null);
  assert.deepStrictEqual(details, ["the earlier conversation could not be replayed: offline"]);
  assert.deepStrictEqual([resume.armed(), resume.note()], [false, "the agent starts fresh — the earlier conversation could not be read"]);
  assert.equal(renders, 5, "an attempt's outcome was not shown, or was shown twice");
  // What a bridge did with the last one, set by the call.
  resume.setLast({ ...blankResumeLast(), attempted: true, armed: true, dropped: 2, unsent: true });
  assert.equal(resume.note(), "the earlier conversation is replayed in part");
  resume.setLast({ ...blankResumeLast(), attempted: true, refused: true });
  assert.equal(resume.note(), "the agent starts fresh — the voice service took no recap");
  // A storage that refuses the write: not remembered, and never thrown at the switch.
  storage.setItem = () => { throw new Error("QuotaExceededError"); };
  assert.doesNotThrow(() => resume.persist(false));
  assert.equal(resume.wanted(), true, "a refused write changed what is kept");
  assert.notEqual(blankResumeLast(), blankResumeLast(), "a blank outcome is shared, so one attempt's marks reach the next");
});
