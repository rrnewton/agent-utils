// The app page's tests: reading messages aloud: blocks, sentences, pieces, the pace, tickets and recovery.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/read-aloud.mjs.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { WEB } from "../../../web/page_sources.mjs";

import { BASE_CSS, BOTTOM_SLACK_PX, CHANNEL, CODER, COLLAPSE_OVER_CHARS, CSS_CODE, DESKTOP_QUERY,
  DISCORD_POLL_MS, HTML_CODE, MODULE_CODE, NOISE_MATCHING, OWN, PAGE_ELEMENTS, PULL_ARM_PX,
  RUN_THREAD, SCRIPT, SCRIPT_CODE, assertMarkupContains, atBottomOf, bookmarkAt, brokenModule,
  brokenScript, channelRow, cssBlock, cssBlockIn, cssBlockOutside, cssRules, delayedReply, deliver,
  doneButton, errorResponse, focusOn, foldButton, helpEntry, hold, inReadingMode, joined, json,
  keyboardPageAtTop, longMessage, mainOnlyThreadPage, markupPlace, message, newPage, openReplyOn,
  optionText, outgoingRetry, outgoingRows, outgoingSpinner, parkMidway, pcm, pickThread, press,
  pressHere, pullDown, reReadChannel, readAloudMessages, readButton, relayed, reloadWith,
  reopenedChannel, replyArrowOf, replyButton, replyInStream, row, rowState, rowWithId, runId,
  runThread, savedOutgoing, selectedId, sentWithoutKey, setReadNew, settingsGroup, showDiscord,
  shownIds, signIn, sourceConstant, speakRelay, splitPost, sseMessage, startTalking, takesASide,
  tallChannel, test, threadBadge, threadButton, threadData, threadOptions, threadPage,
  timelineAnswer, turnSummariesOn, turnTodoOn, unreadChip, viewOf, wavHeader, whoOf, withLiveChannel } from "./fixture.mjs";

// Device speech is selected by the backend; cloud audio remains covered by the existing tests.
const DEVICE_SPEECH = { backend: "browser", label: "Device voice", playback: "browser", local_only: true };

function devicePage(voices, store = new Map()) {
  const page = newPage(store);
  page.readAloud = DEVICE_SPEECH;
  page.enableDeviceSpeech(voices);
  return page;
}

test("device speech reads the full raw combined row synchronously without a speech HTTP request", async () => {
  const page = devicePage();
  await signIn(page);
  const text = "The complete message. " + "Another sentence with enough detail to span several utterances. ".repeat(12);
  const messages = [
    message({ id: "1000000000000000001", content: text, timestamp: "2026-08-19T14:00:00Z" }),
    message({ id: "1000000000000000002", content: "The final part of the same row.", timestamp: "2026-08-19T14:00:01Z" }),
  ];
  const rows = await inReadingMode(page, messages);
  assert.equal(rows.length, 1, "fixture did not produce the combined row");
  const tapped = rows[0].dispatch("click", {});
  assert.equal(page.deviceSpeech.utterances.length, 1, "speak was deferred beyond the tap's call stack");
  await tapped;
  assert.equal(rowState(page, 0).reading, "true");
  assert.deepEqual(page.dismissCalls, []);
  const spoken = [];
  for (let at = 0; at < page.deviceSpeech.utterances.length; at += 1) {
    const utterance = page.deviceSpeech.utterances[at];
    spoken.push(utterance.text);
    assert.ok(Array.from(utterance.text).length <= 180, "an utterance exceeds mobile-safe chunk size");
    assert.equal(utterance.voice.localService, true);
    assert.equal(utterance.lang, "en-US");
    if (at + 1 < 3) assert.deepEqual(page.dismissCalls, [], "archived before all chunks finished");
    utterance.end();
  }
  await page.settle();
  assert.equal(spoken.join(" "), messages.map((m) => m.content).join(" ").replace(/\s+/g, " ").trim());
  assert.deepEqual(page.dismissCalls, [{ messages: messages.map((m) => m.id) }]);
  assert.deepEqual(page.prepareCalls, []);
  assert.deepEqual(page.speakCalls, []);
  assert.equal(page.players.length, 0);
});

test("tapping Read with no error showing leaves a reader near the newest line where they are", async () => {
  // `#33 error-banner-scroll-shift`. Reading clears the error panel on its way in whether or not
  // one is up. Holding the reader for a panel that is not there snapped anyone within
  // BOTTOM_SLACK_PX of the newest line down onto it, under the thumb that had just tapped a row.
  const readNearTheBottom = async (script) => {
    const page = newPage(new Map(), script);
    page.readAloud = DEVICE_SPEECH;
    page.enableDeviceSpeech();
    await signIn(page);
    const messages = Array.from({ length: 8 }, (_, i) => message({
      id: `10000000000000001${String(i).padStart(2, "0")}`,
      author: i % 2 ? "ci-bot" : "deploy-bot",
      author_id: i % 2 ? "1000000000000000001" : "1000000000000000002",
      content: longMessage(`m${i}`),
    }));
    const rows = await inReadingMode(page, messages);
    const area = page.el("scroll-area");
    assert.ok(area.scrollHeight > area.clientHeight * 2, "the fixture cannot scroll, so this proves nothing");
    area.scrollTop = area.scrollHeight - area.clientHeight - Math.floor(BOTTOM_SLACK_PX / 2);
    const before = area.scrollTop;
    assert.ok(atBottomOf(area) && before < area.scrollHeight - area.clientHeight,
      "the reader is not inside the slack short of the newest line, so this is not the case here");
    assert.equal(page.el("error-wrap").hidden, true, "an error is showing, so this is not the case here");
    await rows.at(-2).dispatch("click", {});
    await page.settle();
    assert.equal(page.deviceSpeech.utterances.length > 0, true, "the tap did not start reading");
    return area.scrollTop - before;
  };
  const held = await readNearTheBottom(SCRIPT);
  assert.equal(held, 0, `tapping Read moved the reader by ${held}px`);
  const moved = await readNearTheBottom(brokenScript("  if (box.hidden) return;\n", ""));
  assert.ok(moved > 0, "the negative control did not snap the reader, so the check above proves nothing");
});

test("device speech ignores remote voices and waits for a fresh tap when installed voices appear", async () => {
  const remote = { name: "Cloud voice", lang: "en-US", localService: false, default: true };
  const page = devicePage([remote]);
  await signIn(page);
  const rows = await inReadingMode(page, [message({ content: "Hello from the phone" })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, 0);
  assert.match(page.el("error").textContent, /Text-to-speech.*download a voice/);
  assert.equal(readButton(page).getAttribute("data-read-state"), "failed");
  const local = { name: "Downloaded English", lang: "en-US", localService: true, default: false };
  page.deviceSpeech.setVoices([remote, local]);
  assert.equal(page.deviceSpeech.utterances.length, 0, "voiceschanged tried to speak without a new user gesture");
  assert.equal(readButton(page).getAttribute("data-read-state"), "ready");
  assert.match(page.el("status").textContent, /Tap a message/);
  await rows[0].dispatch("click", {});
  assert.equal(page.deviceSpeech.utterances[0].voice, local);
  assert.deepEqual(page.prepareCalls, []);
  assert.deepEqual(page.speakCalls, []);
});

test("device speech gives useful unsupported-browser guidance without falling back to cloud audio", async () => {
  const page = newPage();
  page.readAloud = DEVICE_SPEECH;
  await signIn(page);
  const rows = await showDiscord(page, [message()]);
  await readButton(page).click();
  await page.settle();
  assert.match(page.el("error").textContent, /Chrome on Android/, "Read looked inert until a row tap");
  assert.equal(readButton(page).getAttribute("data-read-state"), "failed");
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.match(page.el("error").textContent, /Chrome on Android/);
  assert.equal(readButton(page).getAttribute("data-read-state"), "failed");
  assert.deepEqual(page.prepareCalls, []);
  assert.deepEqual(page.speakCalls, []);
  assert.deepEqual(page.dismissCalls, []);
});

test("device speech picks the device language and applies pace to every subsequent chunk", async () => {
  const french = { name: "French", lang: "fr-FR", localService: true, default: true };
  const english = { name: "English", lang: "en-US", localService: true, default: false };
  const page = devicePage([french, english]);
  await signIn(page);
  const rows = await inReadingMode(page, [message({ content: "A sentence long enough to make multiple parts. ".repeat(12) })]);
  assert.match(page.el("speed-value").textContent, /device default/);
  page.el("read-speed-box").value = "145";
  await page.el("read-speed-box").dispatch("change");
  await rows[0].dispatch("click", {});
  assert.equal(page.deviceSpeech.utterances[0].voice, english);
  assert.equal(page.deviceSpeech.utterances[0].rate, 1.45);
  page.el("read-speed-box").value = "180%";
  await page.el("read-speed-box").dispatch("change");
  page.deviceSpeech.utterances[0].end();
  assert.equal(page.deviceSpeech.utterances[1].rate, 1.8);
  assert.deepEqual(page.prepareCalls, []);
});

test("device speech refuses a default local voice in the wrong language", async () => {
  const french = { name: "French default", lang: "fr-FR", localService: true, default: true };
  const page = devicePage([french]);
  await signIn(page);
  const rows = await inReadingMode(page, [message({ content: "This sentence must stay English." })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, 0);
  assert.match(page.el("error").textContent, /matches this browser's language/);
  assert.equal(readButton(page).getAttribute("data-read-state"), "failed");
});

test("device speech accepts a local voice from another region of the same language", async () => {
  const french = { name: "French default", lang: "fr-FR", localService: true, default: true };
  const british = { name: "British English", lang: "en_GB", localService: true, default: false };
  const page = devicePage([french, british]);
  await signIn(page);
  const rows = await inReadingMode(page, [message({ content: "This sentence stays English." })]);
  await rows[0].dispatch("click", {});
  assert.equal(page.deviceSpeech.utterances.length, 1);
  assert.equal(page.deviceSpeech.utterances[0].voice, british);
  assert.equal(page.deviceSpeech.utterances[0].lang, "en_GB");
});

test("device speech prefers the configured default among same-language local voices", async () => {
  const first = { name: "First English", lang: "en-US", localService: true, default: false };
  const configured = { name: "Configured English", lang: "en-US", localService: true, default: true };
  const page = devicePage([first, configured]);
  await signIn(page);
  const rows = await inReadingMode(page, [message({ content: "Use the configured device voice." })]);
  await rows[0].dispatch("click", {});
  assert.equal(page.deviceSpeech.utterances[0].voice, configured);
});

test("device speech ignores late callbacks from a chunk that has already finished", async () => {
  const page = devicePage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ content: "A sentence with several more parts to follow. ".repeat(10) })]);
  await rows[0].dispatch("click", {});
  const first = page.deviceSpeech.utterances[0];
  first.end();
  assert.equal(page.deviceSpeech.utterances.length, 2);
  first.end();
  first.fail();
  assert.equal(page.deviceSpeech.utterances.length, 2, "a completed chunk queued its successor twice");
  assert.equal(page.deviceSpeech.cancellations, 0, "an old chunk canceled the next chunk");
  assert.equal(rowState(page, 0).reading, "true");
  assert.deepEqual(page.dismissCalls, []);
});

test("device speech keeps Unicode intact across a long unbroken message", async () => {
  const page = devicePage();
  await signIn(page);
  const content = "🎵".repeat(410);
  const rows = await inReadingMode(page, [message({ content })]);
  await rows[0].dispatch("click", {});
  for (let at = 0; at < page.deviceSpeech.utterances.length; at += 1) {
    assert.doesNotMatch(page.deviceSpeech.utterances[at].text, /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u);
    page.deviceSpeech.utterances[at].end();
  }
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.map((part) => part.text).join(""), content);
  assert.equal(page.dismissCalls.length, 1);
});

test("device speech reads the server-prepared body, including UID and diff names", async () => {
  const page = devicePage();
  await signIn(page);
  const messages = [message({
    content: "Review D1234567890 for 1000000000000000789",
    spoken_content: "Review Diff ending in 890 for UID ending in 789",
  })];
  const rows = await inReadingMode(page, messages);
  await rows[0].dispatch("click", {});

  assert.equal(page.deviceSpeech.utterances[0].text, messages[0].spoken_content);
  assert.doesNotMatch(page.deviceSpeech.utterances[0].text, /D1234567890|1000000000000000789/);
  assert.deepEqual(page.prepareCalls, [], "device speech made a network preparation request");
});

// With empty messages shown (`#386 hide-empty-messages`): hidden, the default, there is no row to tap.
for (const content of ["", "   "]) {
  test(`device speech does not archive a message with ${JSON.stringify(content)} text`, async () => {
    const page = devicePage(undefined, new Map([["vibe-talk.voice.hide-empty", "0"]]));
    await signIn(page);
    const rows = await inReadingMode(page, [message({ content })]);
    await rows[0].dispatch("click", {});
    await page.settle();
    assert.match(page.el("error").textContent, /no text to read/);
    assert.equal(page.deviceSpeech.utterances.length, 0);
    assert.deepEqual(page.dismissCalls, []);
  });
}

test("device speech cancellation, replacement, and stale events cannot archive an unread row", async () => {
  const page = devicePage();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", content: "first" }),
    message({ id: "1000000000000000002", content: "second" }),
  ]);
  await rows[0].dispatch("click", {});
  const first = page.deviceSpeech.utterances[0];
  await rows[1].dispatch("click", {});
  assert.equal(page.deviceSpeech.cancellations, 1);
  first.end();
  first.fail();
  assert.equal(rowState(page, 1).reading, "true");
  assert.deepEqual(page.dismissCalls, []);
  await rows[1].dispatch("click", {});
  page.deviceSpeech.utterances[1].end();
  assert.equal(page.deviceSpeech.cancellations, 2);
  assert.equal(page.deviceSpeech.utterances.length, 2, "second tap restarted rather than stopped");
  assert.deepEqual(page.dismissCalls, []);
  assert.equal(rowState(page, 1).reading, "false");
});

test("device speech is interruptible while the engine has not started speaking", async () => {
  const page = devicePage();
  page.deviceSpeech.autoStart = false;
  await signIn(page);
  const rows = await inReadingMode(page, [message()]);
  await rows[0].dispatch("click", {});
  assert.equal(readButton(page).getAttribute("data-read-state"), "working");
  await rows[0].dispatch("click", {});
  page.deviceSpeech.utterances[0].start();
  page.deviceSpeech.utterances[0].end();
  assert.equal(page.deviceSpeech.cancellations, 1);
  assert.deepEqual(page.dismissCalls, []);
  assert.equal(rowState(page, 0).reading, "false");
  assert.equal(readButton(page).getAttribute("data-read-state"), "ready");
});

for (const action of ["stop", "view", "settings"]) {
  test(`device speech cancels when leaving playback by ${action}`, async () => {
    const page = devicePage();
    await signIn(page);
    const rows = await inReadingMode(page, [message()]);
    await rows[0].dispatch("click", {});
    // Stop, beside Read: Read itself is the read's Pause while it plays (`#434 read-aloud-pause`).
    if (action === "stop") await page.el("read-aloud-stop").click();
    if (action === "view") await page.el("view-switch").click();
    if (action === "settings") await page.el("open-settings").click();
    page.deviceSpeech.utterances[0].end();
    assert.equal(page.deviceSpeech.cancellations, 1);
    assert.deepEqual(page.dismissCalls, []);
    assert.equal(rowState(page, 0).reading, "false");
  });
}

for (const error of ["not-allowed", "voice-unavailable", "audio-hardware", "network"]) {
  test(`device speech surfaces ${error} without archiving or starting a cloud request`, async () => {
    const page = devicePage();
    await signIn(page);
    const rows = await inReadingMode(page, [message()]);
    await rows[0].dispatch("click", {});
    page.deviceSpeech.utterances[0].fail(error);
    await page.settle();
    assert.equal(page.el("error").hidden, false);
    assert.equal(readButton(page).getAttribute("data-read-state"), "failed");
    assert.deepEqual(page.dismissCalls, []);
    assert.deepEqual(page.speakCalls, []);
    assert.equal(rowState(page, 0).reading, "false");
  });
}

for (const started of [false, true]) {
  test(`device speech watchdog cancels an engine that never ${started ? "finishes" : "starts"}`, async () => {
    const page = devicePage();
    page.deviceSpeech.autoStart = started;
    await signIn(page);
    const rows = await inReadingMode(page, [message()]);
    await rows[0].dispatch("click", {});
    const timerDelay = sourceConstant(started ? "BROWSER_SPEECH_FINISH_MS" : "BROWSER_SPEECH_START_MS");
    assert.ok(page.expireTimers(timerDelay) >= 1);
    assert.match(page.el("error").textContent, /stopped responding/);
    assert.equal(readButton(page).getAttribute("data-read-state"), "failed");
    assert.equal(page.deviceSpeech.cancellations, 1);
    page.deviceSpeech.utterances[0].start();
    page.deviceSpeech.utterances[0].end();
    assert.deepEqual(page.dismissCalls, []);
    assert.equal(rowState(page, 0).reading, "false");
  });
}

test("device speech clears watchdogs on stop and replaces the failed status on a successful retry", async () => {
  const page = devicePage();
  await signIn(page);
  const rows = await inReadingMode(page, [message()]);
  await rows[0].dispatch("click", {});
  page.deviceSpeech.utterances[0].fail("not-allowed");
  assert.match(page.el("status").textContent, /was blocked/);
  await rows[0].dispatch("click", {});
  assert.equal(page.el("error").hidden, true);
  assert.match(page.el("status").textContent, /Reading with Device voice/);
  await readButton(page).click();
  assert.equal(page.expireTimers(sourceConstant("BROWSER_SPEECH_START_MS")), 0);
  assert.equal(page.expireTimers(sourceConstant("BROWSER_SPEECH_FINISH_MS")), 0);
  assert.equal(page.el("error").hidden, true);
  assert.deepEqual(page.dismissCalls, []);
});

test("device speech cannot overlap a live voice call or be recorded by its microphone", async () => {
  const page = devicePage();
  await startTalking(page);
  const rows = await inReadingMode(page, [message()]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.match(page.el("error").textContent, /Hang up the voice call/);
  assert.equal(page.deviceSpeech.utterances.length, 0);
  assert.equal(page.sockets[0].readyState, 1, "reading closed the live call");
  assert.ok(page.tracks.every((track) => track.stops === 0), "reading stopped microphone capture");
  assert.deepEqual(page.speakCalls, []);
});

test("a configured audio speech backend keeps using server audio even when device voices exist", async () => {
  const page = devicePage();
  page.readAloud = { backend: "elevenlabs", label: "ElevenLabs", playback: "audio", local_only: false };
  await signIn(page);
  const rows = await inReadingMode(page, [message()]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.ok(page.prepareCalls.length > 0);
  assert.equal(page.players.length, 1);
  assert.equal(page.deviceSpeech.utterances.length, 0);
});

test("the channel bar switches between configured agent audio and device audio locally", async () => {
  // Visibility is read from the `hidden` ATTRIBUTE: the icons are <svg>, where `.hidden` is an
  // expando a browser ignores. Reading the property back passed while a phone showed the phone
  // icon in both modes.
  const shown = (id) => !page.el(id).hasAttribute("hidden");
  const page = devicePage();
  // A label no vendor ships: the agent side is named only by the configured provider profile.
  page.readAloud = { backend: "conversation", label: "Internal voice preview", playback: "audio", local_only: false };
  await signIn(page);
  const rows = await showDiscord(page, [message({ content: "read this locally" })]);
  const toggle = page.el("audio-source");
  assert.equal(toggle.hidden, false);
  assert.equal(toggle.getAttribute("aria-checked"), "true");
  assert.equal(toggle.getAttribute("aria-label"), "Read messages with Internal voice preview");
  assert.equal(toggle.title, "Internal voice preview. Tap to use device audio.");
  assert.equal(page.el("settings-audio-source").value, "agent");
  assert.equal(page.el("settings-agent-audio").hasAttribute("hidden"), false);
  assert.equal(shown("audio-device-icon"), false, "agent audio still shows the phone icon");
  assert.equal(shown("audio-agent-icon"), true, "agent audio does not show its cloud icon");
  assert.match(HTML_CODE, /id="audio-device-icon"[^>]*data-icon="device"/);
  assert.match(HTML_CODE, /id="audio-agent-icon"[^>]*data-icon="cloud"/);
  for (const [where, code] of [["web/voice.js", SCRIPT_CODE], ["web/src/", MODULE_CODE], ["web/voice.html", HTML_CODE]]) {
    assert.doesNotMatch(code, /elevenlabs/i, `${where} names a speech vendor instead of the profile label`);
  }
  const requests = page.prepareCalls.length;
  await toggle.click();
  assert.equal(toggle.getAttribute("aria-checked"), "false");
  assert.equal(toggle.getAttribute("aria-label"), "Read messages with device audio");
  assert.equal(toggle.title, "Device audio. Tap to use Internal voice preview.");
  assert.equal(page.el("settings-audio-source").value, "device");
  assert.equal(shown("audio-device-icon"), true, "device speech lost its phone icon");
  assert.equal(shown("audio-agent-icon"), false, "device speech still shows the cloud icon");
  assert.equal(page.prepareCalls.length, requests, "changing the source issued a network request");
  await toggle.click();
  assert.equal(toggle.getAttribute("aria-checked"), "true");
  assert.equal(toggle.getAttribute("aria-label"), "Read messages with Internal voice preview");
  assert.equal(toggle.title, "Internal voice preview. Tap to use device audio.");
  assert.equal(shown("audio-device-icon"), false, "switching back left the phone icon showing");
  assert.equal(shown("audio-agent-icon"), true, "switching back did not restore the cloud icon");
  assert.equal(page.prepareCalls.length, requests, "switching back issued a network request");
  await toggle.click();
  await readButton(page).click();
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, 1);
  assert.deepEqual(page.speakCalls, []);
});

test("settings offers the car-compatible agent audio source and stays in sync with the bar", async () => {
  const page = devicePage();
  page.readAloud = {
    backend: "conversation",
    label: "Internal voice preview",
    playback: "audio",
    local_only: false,
  };
  await signIn(page);
  const setting = page.el("settings-audio-source");
  assert.equal(setting.value, "agent");
  setting.value = "device";
  await setting.dispatch("change");
  assert.equal(page.el("audio-source").getAttribute("aria-checked"), "false");
  assert.equal(page.storage.get("vibe-talk.voice.read-audio-source"), "device");
  setting.value = "agent";
  await setting.dispatch("change");
  assert.equal(page.el("audio-source").getAttribute("aria-checked"), "true");
  assert.equal(page.storage.get("vibe-talk.voice.read-audio-source"), "agent");
});

test("settings does not offer agent audio when this server has none", async () => {
  const page = devicePage();
  await signIn(page);
  assert.equal(page.el("settings-audio-source").value, "device");
  assert.equal(page.el("settings-agent-audio").hasAttribute("hidden"), true);
  assert.equal(page.el("settings-agent-audio").hasAttribute("disabled"), true);
});

test("READ REPLACES TALK IN THE CHANNEL VIEW, and Talk comes back in the transcript", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(readButton(page).hidden, true, "Read is offered over the voice transcript");
  assert.equal(page.el("talk").hidden, false);

  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  assert.equal(readButton(page).hidden, false, "Read is not offered where the messages are");
  assert.equal(page.el("talk").hidden, true, "Talk is still taking room in the channel view");

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(readButton(page).hidden, true, "Read followed the reader back to the transcript");
  assert.equal(page.el("talk").hidden, false);
});

test("tapping a message in reading mode speaks THAT message", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", content: "first" }),
    message({ id: "1000000000000000002", content: "second" }),
  ]);

  await rows[1].dispatch("click", {});
  await page.settle();

  assert.deepEqual(readAloudMessages(page), ["1000000000000000002"], "the wrong message was read");
  assert.equal(page.players.length, 1, "no player was built");
  assert.equal(page.players[0].playCount, 1, "the audio was fetched but never played");
  assert.equal(rowState(page, 1).reading, "true", "nothing on screen says which row is speaking");
  assert.equal(rowState(page, 0).reading, "false");
});

test("the message archives itself when the audio FINISHES, and not before", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001", content: "first" })]);

  await rows[0].dispatch("click", {});
  await page.settle();
  assert.deepEqual(page.dismissCalls, [], "the message was archived before it had been read");

  await page.players[0].end();
  await page.settle();
  assert.deepEqual(
    page.dismissCalls,
    [{ messages: ["1000000000000000001"] }],
    "finishing the audio did not archive the message"
  );
  assert.equal(rowState(page, 0).archived, "true", "the row did not grey once it had been read");
  assert.equal(rowState(page, 0).reading, "false", "the row is still marked as speaking");
});

test("audio that FAILS archives nothing — the archive is how the reader knows what is left", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  await rows[0].dispatch("click", {});
  await page.settle();
  await page.players[0].fail();
  await page.settle();

  assert.deepEqual(page.dismissCalls, [], "a message that could not be played was filed away");
  assert.equal(rowState(page, 0).archived, "false");
  assert.equal(rowState(page, 0).reading, "false", "the row is stuck looking like it is speaking");
});

test("a read that the SERVER refuses archives nothing and says why", async () => {
  const page = newPage();
  // THE FALLBACK PATH ON PURPOSE. What is under test is a FETCH failing, and a prepared row does
  // not fetch — it is handed a URL and the failure, if there is one, arrives on the audio element
  // instead. Preparing is disabled so the wait and the refusal this covers still exist.
  page.prepareFails = true;
  await signIn(page);
  page.speakStatus = 503;
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  await rows[0].dispatch("click", {});
  await page.settle();

  assert.equal(page.players.length, 0, "a player was built for audio that never arrived");
  assert.deepEqual(page.dismissCalls, [], "a message nobody heard was archived");
  // BOTH places, and they are not the same job. The standing panel is what survives; the status
  // line is the answer under the thumb that just tapped. A tap that reports only into a panel the
  // reader is not looking at reads as a tap that did nothing.
  assert.match(page.el("error").text(), /voice_id|configured/i, "the standing panel says nothing");
  assert.match(
    page.el("status").textContent,
    /could not read that message aloud/i,
    "the tap got no answer where the reader was looking"
  );
});

test("tapping the message that is PLAYING stops it, and stopping archives nothing", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  await rows[0].dispatch("click", {});
  await page.settle();
  await rows[0].dispatch("click", {});
  await page.settle();

  assert.equal(page.players[0].paused, true, "a second tap did not stop the audio");
  assert.deepEqual(page.dismissCalls, [], "stopping halfway filed the message away");
  assert.equal(rowState(page, 0).reading, "false");
  // NOTHING TO RELEASE ON THIS PATH, and releasing something would be the bug. A prepared row plays
  // a URL belonging to the SERVER; it is not an object URL, this page does not own it, and handing
  // it to `revokeObjectURL` would be the page confusing the two kinds of URL it now deals in.
  assert.deepEqual(
    page.revokedUrls,
    [],
    "a server URL was passed to revokeObjectURL, which owns nothing and frees nothing"
  );
});

test("...and the blob it fetched IS released, when it had to fetch one", async () => {
  // The other path, and the leak is real on it: falling back means one object URL per message, and
  // never revoking them grows with the backlog. Preparing is what usually avoids the fetch; this is
  // what happens when preparing could not.
  const page = newPage();
  page.prepareFails = true;
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  await rows[0].dispatch("click", {});
  await page.settle();
  assert.ok(page.speakCalls.length > 0, "the fallback never fetched, so nothing is being tested");
  await rows[0].dispatch("click", {});
  await page.settle();

  assert.equal(page.players[0].paused, true, "a second tap did not stop the audio");
  assert.equal(page.revokedUrls.length, 1, "the audio URL was never released");
});

test("THE MESSAGE BEING READ STAYS OPEN ACROSS THE POLL THAT REBUILDS THE LIST", async () => {
  // The owner watched a message he was listening to collapse mid-read and assumed a stray tap. It
  // was not. The channel re-reads itself every DISCORD_POLL_MS, `applyNewestPage` rebuilds every
  // row, and a freshly built row starts FOLDED — so a long message folded itself on the next poll,
  // every time, while its own audio was still playing.
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", content: "w".repeat(4000) }),
  ]);

  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(row(page, 0).getAttribute("data-collapsed"), "false", "reading did not open it");

  // The poll, exactly as it happens on its own.
  await reReadChannel(page);

  assert.equal(rowState(page, 0).reading, "true", "the rebuild lost track of what was playing");
  assert.equal(
    row(page, 0).getAttribute("data-collapsed"),
    "false",
    "the message being read folded itself under the reader"
  );
});

test("the row being read says so where its author was", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", author: "alice", author_is_bot: false }),
  ]);

  await rows[0].dispatch("click", {});
  await page.settle();
  const mark = row(page, 0).descendants().find((n) => n.className === "msg-author reading-mark");
  assert.ok(mark, "nothing in the row says it is the one being read");
  assert.equal(mark.text(), "reading");

  // ...and it goes back to naming the author once the audio is done.
  await page.players[0].end();
  await page.settle();
  assert.match(row(page, 0).text(), /alice/, "the author never came back");
  assert.doesNotMatch(row(page, 0).text(), /reading/, "the row still claims to be playing");
});

test("the reading pace is the AGENT'S until the reader chooses one", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  // THE PACE NOW TRAVELS WITH THE PREPARATION, not with the tap. It is chosen when the ticket is
  // minted so that playing one is a URL and nothing else, which is the whole point of preparing.
  assert.equal(
    page.prepareCalls[0].speed,
    null,
    "an unset pace sent 100%, overriding an agent configured to speak faster"
  );

  page.el("read-speed-box").value = "150";
  await page.el("read-speed-box").dispatch("change", {});
  await page.settle();
  // ...and a ticket minted at the OLD pace is thrown away rather than played. A reader who moves
  // the slider and hears no difference has been given the most confusing possible answer.
  assert.equal(
    page.prepareCalls[page.prepareCalls.length - 1].speed,
    1.5,
    `the chosen pace never reached the server; calls=${JSON.stringify(page.prepareCalls)}`
  );
  assert.ok(
    page.prepareCalls.length > 1,
    "changing the pace did not re-prepare, so the old pace would still be played"
  );
  assert.equal(page.storage.get("vibe-talk.voice.read-speed"), "150", "the pace was not kept");
});

test("TURNING READ-ALOUD ON PREPARES EVERY MESSAGE ON SCREEN, BEFORE ANY TAP", async () => {
  // The owner's ask: the phone already has the text of everything on screen, so the tap should be
  // the shortest possible path to audio and everything else should already have happened. This is
  // the "everything else" — one request, when the mode goes on, covering every visible row.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001" }),
    message({ id: "1000000000000000002" }),
  ]);
  assert.deepEqual(page.prepareCalls, [], "something was prepared before reading mode was on");

  await page.el("read-aloud").click();
  await page.settle();

  assert.equal(page.prepareCalls.length, 1, "turning the mode on prepared nothing");
  assert.deepEqual(
    page.prepareCalls[0].ids,
    ["1000000000000000001", "1000000000000000002"],
    "not every message on screen was prepared"
  );
  // AND NOTHING WAS SPENT. Preparing resolves text and mints tickets; it does not generate audio,
  // so turning the mode on over a long backlog must not bill for a backlog of speech.
  assert.deepEqual(page.speakCalls, [], "turning the mode on generated audio nobody asked for");
});

test("...so the tap itself fetches NOTHING and plays the prepared URL straight away", async () => {
  // The whole latency argument in one assertion. A tap used to fetch the audio and wait for the
  // last byte of it; now it hands an already-authorised URL to the player and the browser streams.
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  const preparedBefore = page.prepareCalls.length;

  await rows[0].dispatch("click", {});
  await page.settle();

  assert.deepEqual(page.speakCalls, [], "the tap still fetched the audio it had already prepared");
  assert.equal(
    page.prepareCalls.length,
    preparedBefore,
    "the tap re-prepared, so the work moved rather than being done ahead of time"
  );
  assert.match(
    page.players[0].url,
    /^\/api\/v1\/speech\/ticket-for-1000000000000000001\?selection=tap-[0-9a-f]{16}$/,
    "the player was not handed the prepared URL for this tap"
  );
  assert.equal(page.players[0].playCount, 1, "nothing was played");
});

test("A TAP DOES NOT WAIT FOR THE BATCH: it prepares only its own row and reads it at once", async () => {
  // `#191 read-aloud-first-lag`. The measured first read spent 1.78 s of five waiting for the
  // whole screen's preparation, because a tap joined it. Now the batch is held open for the whole
  // of this test, and the tap still plays.
  const page = newPage();
  await signIn(page);
  let releaseBatch = () => {};
  const batchHeld = new Promise((resolve) => { releaseBatch = resolve; });
  page.prepareHold = (asked) => (asked.ids.length > 1 ? batchHeld : null);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", author_id: "1000000000000000011", content: "first" }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "second" }),
    message({ id: "1000000000000000003", author_id: "1000000000000000013", content: "third" }),
  ]);
  assert.equal(rows.length, 3, "the fixture was drawn as one row");
  assert.equal(page.prepareCalls.length, 1, "turning Read on did not start the batch");
  assert.equal(page.prepareCalls[0].ids.length, 3);

  await rows[1].dispatch("click", {});
  await page.settle();

  assert.deepEqual(page.prepareCalls[1].ids, ["1000000000000000002"], "the tap prepared more than its row");
  assert.equal(page.players.length, 1, "the read waited for the batch");
  assert.match(page.players[0].url, /^\/api\/v1\/speech\/ticket-for-1000000000000000002\?selection=tap-/);
  assert.equal(page.players[0].playCount, 1, "nothing was played");
  assert.deepEqual(page.speakCalls, [], "the tap fell back to the slow route");
  // `#375 read-aloud-latency`. The batch gave way to the tap and starts again behind it, for the
  // rows still unprepared.
  assert.equal(page.prepareCalls.length, 3, "the batch did not start again behind the tap");
  assert.deepEqual([...page.prepareCalls[2].ids].sort(), ["1000000000000000001", "1000000000000000003"]);

  // The batch finishes behind the read, and the other rows are then ready for their own taps.
  releaseBatch();
  await page.settle();
  assert.equal(page.prepareCalls.length, 3, "the batch was asked for again");
  await rows[2].dispatch("click", {});
  await page.settle();
  assert.equal(page.prepareCalls.length, 3, "a row the batch had prepared was prepared again");
  assert.match(page.players[1].url, /^\/api\/v1\/speech\/ticket-for-1000000000000000003\?selection=tap-/);
});

test("a tap on a COMBINED row prepares every message in that row, and nothing else", async () => {
  const page = newPage();
  await signIn(page);
  page.prepareHold = (asked) => (asked.ids.length > 2 ? new Promise(() => {}) : null);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", content: "first half", timestamp: "2026-08-19T04:31:00.000Z" }),
    message({ id: "1000000000000000002", content: "second half", timestamp: "2026-08-19T04:31:01.000Z" }),
    message({ id: "1000000000000000003", author_id: "1000000000000000013", content: "another",
      timestamp: "2026-08-19T05:31:00.000Z" }),
  ]);
  assert.equal(rows.length, 2, "the first two were not combined");

  await rows[0].dispatch("click", {});
  await page.settle();

  assert.ok(
    page.prepareCalls.some((call) => JSON.stringify(call.ids) === JSON.stringify(["1000000000000000001", "1000000000000000002"])),
    `the combined row was not prepared on its own: ${JSON.stringify(page.prepareCalls)}`
  );
  assert.equal(page.players.length, 1, "the combined read did not start");
  assert.match(page.players[0].url, /ticket-for-1000000000000000001\?/);
});

test("audible playback reports content-free transfer and browser timing", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  const tappedAt = page.clock();

  await rows[0].dispatch("click", {});
  await page.settle();
  page.setClock(tappedAt + 120);
  await page.players[0].loaded();
  page.setClock(tappedAt + 155);
  await page.players[0].started();
  await page.settle();

  assert.deepEqual(page.speechTimingCalls, [{
    ticket: "ticket-for-1000000000000000001",
    timing: {
      tap_to_play_ms: 0,
      request_to_loaded_ms: 120,
      loaded_to_playing_ms: 35,
      tap_to_audible_ms: 155,
    },
    keepalive: true,
  }]);
  assert.doesNotMatch(
    JSON.stringify(page.speechTimingCalls),
    /message content|api-token|authorization/i,
    "the playback report carried content or credentials"
  );
});

test("...and a message that could not be prepared still reads, by the older slower route", async () => {
  // Preparing is an OPTIMISATION and has to fail like one. A server too old to know the route, or a
  // request that lost a race with a scroll, must cost speed and never the feature.
  const page = newPage();
  page.prepareFails = true;
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  await rows[0].dispatch("click", {});
  await page.settle();

  assert.deepEqual(
    page.speakCalls,
    ["1000000000000000001"],
    "preparing failed and took read-aloud down with it"
  );
  assert.equal(page.players[0].playCount, 1, "nothing was played on the fallback path");
});

// --- `#376 read-aloud-blocks`: a long message read block by block -----------------------------------

/**
 * A message of three blocks as the server renders it: a paragraph, a list of two items. The
 * server's own walk of the same HTML (`html_block_kinds` in src/render.rs) finds these kinds.
 */
const BLOCKED_HTML = "<p>The run finished.</p>\n<ul>\n<li>the budget landed</li>\n<li>the rewrite is a draft</li>\n</ul>\n";
const BLOCKED_KINDS = [{ kind: "paragraph", spoken: true }, { kind: "item", spoken: true },
  { kind: "item", spoken: true }];
const BLOCKED_ID = "1000000000000000001";

/** A page in Read with one message of three blocks prepared as such, streamed into Web Audio. */
async function blockedRead(options = {}) {
  const page = newPage(options.store);
  page.enableStreamedAudio();
  page.enableAbort();
  if (options.mediaSession) page.enableMediaSession();
  page.speechBlocks[BLOCKED_ID] = options.kinds || BLOCKED_KINDS;
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: BLOCKED_ID, content: options.content || "The run finished.\n\n- the budget landed\n- the rewrite is a draft",
      content_html: options.html || BLOCKED_HTML }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "one block" }),
  ]);
  return { page, rows };
}

const blockOf = (url) => new URL(url, "http://page").searchParams.get("block");
const selectionOf = (url) => new URL(url, "http://page").searchParams.get("selection");

/** Every element in `node`, depth first. */
const everyElement = (node) => (node.children || []).flatMap((kid) => [kid, ...everyElement(kid)]);

/** The tag and text of the element lit as the block being read in `row`, or "" when none is. */
function litBlock(row) {
  const lit = everyElement(row).filter((node) => node.getAttribute && node.getAttribute("data-reading-block") === "current");
  assert.ok(lit.length <= 1, "more than one block is lit");
  return lit.length === 0 ? "" : `${String(lit[0].tagName).toLowerCase()}:${lit[0].text().trim()}`;
}

/** The block elements of `row`'s first message, in order: the fixture's p, li, li. */
function blockElements(row) {
  const body = everyElement(row).find((node) => node.className === "md");
  return everyElement(body).filter((node) => ["p", "li"].includes(String(node.tagName).toLowerCase()));
}

/** Feed stream `at` a header and `frames` samples at 1 kHz, then end it. */
async function speakBlock(page, at, frames = 100) {
  const stream = page.audioStreams[at];
  stream.controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(frames).fill(0x2000))));
  await page.settle();
  stream.controller.drop();
  await page.settle();
}

/**
 * The message tests/read_aloud_blocks_browser.py walks: its text, the HTML the server renders it as
 * (src/render.rs's tests hold the two together), the blocks the server lists, and the element each
 * is drawn as.
 */
const BLOCKS_FIXTURE = JSON.parse(readFileSync(join(WEB, "..", "tests", "fixtures", "read_aloud_blocks.json"), "utf8"));

test("THE PAGE FINDS, in the server's rendering of a long message, exactly the blocks the server lists", async () => {
  // The two halves of one rule: the server walks its tree and its HTML, the page walks the body it
  // drew. Stepped through with Next, every block lit is the element the fixture says, in order.
  const { page, rows } = await blockedRead({ content: BLOCKS_FIXTURE.content, html: BLOCKS_FIXTURE.html,
    kinds: BLOCKS_FIXTURE.blocks.map((kind) => ({ kind, spoken: true })) });
  await rows[0].dispatch("click", {});
  await page.settle();
  const lit = [];
  for (let at = 0; at < BLOCKS_FIXTURE.blocks.length; at += 1) {
    lit.push(litBlock(page.el("discord-log").children[0]).split(":")[0]);
    await page.el("read-next").click();
    await page.settle();
  }
  assert.deepEqual(lit, BLOCKS_FIXTURE.elements);
});

test("A MESSAGE PREPARED IN BLOCKS IS READ BLOCK BY BLOCK, the next asked for once the current has audio", async () => {
  // The owner listens to long messages in the car, and one read as a single stretch gave him no way
  // to see or change where he was in it.
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.audioStreams.length, 1, "more than the first block was asked for before any audio");
  assert.equal(blockOf(page.audioStreams[0].url), "0");
  page.audioStreams[0].controller.bytes(joined(wavHeader(1000, 1), pcm(0x2000, 0x2000)));
  await page.settle();
  // On a voice with one session the next block queues behind this one and is generated the moment
  // it is: asked for now, it is there when this one ends.
  assert.equal(page.audioStreams.length, 2, "the next block was not asked for when the first had audio");
  assert.equal(blockOf(page.audioStreams[1].url), "1");
  assert.equal(selectionOf(page.audioStreams[1].url), selectionOf(page.audioStreams[0].url),
    "the blocks of one read were asked for under two selections, so the second would cut off the first");
  // ONE ahead: the next block's own audio, before it plays, asks for nothing more. A provider that
  // serves reads side by side would otherwise be asked for the whole message at once.
  page.audioStreams[1].controller.bytes(joined(wavHeader(1000, 1), pcm(0x2000, 0x2000)));
  await page.settle();
  assert.equal(page.audioStreams.length, 2, "more than one block ahead was asked for");
});

test("the next block is scheduled to BEGIN WHERE THE CURRENT ONE ENDS, so the boundary has no gap", async () => {
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  // Block 1's audio arrives while block 0 is still being received.
  page.audioStreams[0].controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(100).fill(0x2000))));
  await page.settle();
  page.audioStreams[1].controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(80).fill(0x2000))));
  await page.settle();
  const before = page.audio.sources.length;
  assert.equal(before, 1, "the next block played before the current one had all arrived");
  page.audioStreams[0].controller.drop();
  await page.settle();
  const [first, second] = page.audio.sources;
  assert.ok(second, "the next block was not scheduled once the current one had all arrived");
  const end = first.startedAt + first.buffer.duration;
  assert.ok(Math.abs(second.startedAt - end) < 1e-9,
    `the next block starts at ${second.startedAt}, the current ends at ${end}: a gap at the boundary`);

  // THE CONTROL: a block whose audio comes after the one before has finished playing cannot begin
  // where it ended, and starts the cushion ahead of now instead. The check above can tell the two.
  page.audio.currentTime = second.startedAt + second.buffer.duration + 1;
  page.audioStreams[1].controller.drop();
  await page.settle();
  page.audioStreams[2].controller.bytes(joined(wavHeader(1000, 1), pcm(0x2000, 0x2000)));
  await page.settle();
  const third = page.audio.sources.at(-1);
  assert.ok(Math.abs(third.startedAt - (page.audio.currentTime + 0.5)) < 1e-9,
    `a late block started at ${third.startedAt} with the clock at ${page.audio.currentTime}`);
});

test("THE BLOCK BEING READ IS LIT and the row counts it, 'reading 1 of 3', with Previous and Next shown", async () => {
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  const row = page.el("discord-log").children[0];
  assert.equal(litBlock(row), "p:The run finished.");
  const mark = row.descendants().find((n) => n.className === "msg-author reading-mark");
  assert.equal(mark.text(), "reading 1 of 3");
  assert.equal(page.el("read-steer").hidden, false, "the steering strip is not shown");
  assert.equal(page.el("read-steer-count").text(), "1 of 3");
  assert.equal(page.el("read-next").disabled, false);

  // The light moves when the next block is HEARD, not when it is asked for.
  // Block 0 is due half a second from now (the cushion), and is 0.1 s long.
  page.audioStreams[0].controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(100).fill(0x2000))));
  await page.settle();
  assert.equal(page.expireTimers(500), 1, "nothing waited for block 0 to be heard");
  await page.settle();
  page.audioStreams[1].controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(100).fill(0x2000))));
  await page.settle();
  page.audioStreams[0].controller.drop();
  await page.settle();
  assert.equal(litBlock(row), "p:The run finished.", "the light moved before the next block was heard");
  // Block 1 is due where block 0 ends, 0.6 s from now.
  assert.equal(page.expireTimers(600), 1, "nothing waited for block 1 to be heard");
  await page.settle();
  assert.equal(litBlock(row), "li:the budget landed");
  assert.equal(page.el("read-steer-count").text(), "2 of 3");
  assert.equal(row.descendants().find((n) => n.className === "msg-author reading-mark").text(), "reading 2 of 3");

  // A PAUSE holds the light where the voice is: block 2 is due on the audio clock, and the clock
  // has stopped.
  page.audioStreams[1].controller.drop();
  await page.settle();
  page.audioStreams[2].controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(100).fill(0x2000))));
  await page.settle();
  page.audio.state = "suspended";
  assert.equal(page.expireTimers(700), 1, "nothing waited for block 2 to be heard");
  await page.settle();
  assert.equal(litBlock(row), "li:the budget landed", "the light moved on while the audio was paused");
  page.audio.state = "running";
  page.audio.currentTime = 0.7;
  assert.equal(page.expireTimers(250), 1, "nothing looked again once the audio played on");
  await page.settle();
  assert.equal(litBlock(row), "li:the rewrite is a draft");

  // ...and is lit again on a row the poll rebuilds under it.
  await reReadChannel(page);
  assert.equal(litBlock(page.el("discord-log").children[0]), "li:the rewrite is a draft", "the rebuild lost the light");
});

test("a tap on ANOTHER block of the message being read jumps there; on the block being read, stops it", async () => {
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  const row = page.el("discord-log").children[0];
  const [, , last] = blockElements(row);
  const first = page.audioStreams[0];
  await row.dispatch("click", { target: last });
  await page.settle();
  assert.equal(first.options.signal.aborted, true, "the read before the jump was left open");
  const jumped = page.audioStreams.at(-1);
  assert.equal(blockOf(jumped.url), "2", "the jump did not ask for the block tapped");
  assert.notEqual(selectionOf(jumped.url), selectionOf(first.url),
    "the jump reused the old selection, so the server would queue it behind the old read");
  assert.equal(litBlock(page.el("discord-log").children[0]), "li:the rewrite is a draft");
  assert.equal(page.el("read-steer-count").text(), "3 of 3");
  assert.equal(page.el("read-next").disabled, true, "Next is offered on the last block");

  await page.el("discord-log").children[0].dispatch("click", { target: blockElements(page.el("discord-log").children[0])[2] });
  await page.settle();
  assert.equal(jumped.options.signal.aborted, true, "a tap on the block being read did not stop it");
  assert.equal(page.el("read-steer").hidden, true, "the strip outlived the read");
  assert.equal(litBlock(page.el("discord-log").children[0]), "", "a block stayed lit after the read stopped");
});

test("Previous and Next move the read a block at a time; Previous on the first block starts it again", async () => {
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-next").click();
  await page.settle();
  assert.equal(blockOf(page.audioStreams.at(-1).url), "1");
  assert.equal(page.el("read-steer-count").text(), "2 of 3");
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "3 of 3");
  const before = page.audioStreams.length;
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.audioStreams.length, before, "Next past the last block asked for something");
  await page.el("read-prev").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3");
  await page.el("read-prev").click();
  await page.settle();
  await page.el("read-prev").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 3");
  assert.equal(blockOf(page.audioStreams.at(-1).url), "0", "Previous on the first block did not start it again");
});

// --- `#627 scrubber-thumb-arrows`: the fat arrows step a section, the thin ones a block -------------

/** A message of two sections under headings: h2, p, p, h2, p. Sections begin at blocks 0 and 3. */
const SECTIONED_HTML = "<h2>Done</h2>\n<p>The run finished.</p>\n<p>The budget landed.</p>\n<h2>Open</h2>\n<p>The rewrite is a draft.</p>\n";
const SECTIONED_KINDS = ["heading", "paragraph", "paragraph", "heading", "paragraph"].map((kind) => ({ kind, spoken: true }));
const SECTIONED_CONTENT = "## Done\n\nThe run finished.\n\nThe budget landed.\n\n## Open\n\nThe rewrite is a draft.";

test("`#627 scrubber-thumb-arrows`: four arrows, each a 44px target with its own name, the thin pair stepping blocks as Previous and Next did", () => {
  for (const [id, label, weight] of [["read-section-prev", "Previous section", "fat"], ["read-section-next", "Next section", "fat"],
    ["read-prev", "Previous block", "thin"], ["read-next", "Next block", "thin"]]) {
    const tag = HTML_CODE.match(new RegExp(`<button id="${id}"[^>]*>`))[0];
    assert.match(tag, new RegExp(`aria-label="${label}"`), `${id} is not named ${label}`);
    assert.match(tag, new RegExp(`data-weight="${weight}"`), `${id} is not a ${weight} arrow`);
    assert.match(tag, /class="read-steer-button read-steer-arrow"/, `${id} is not drawn as an arrow`);
  }
  assert.match(cssBlock(".read-steer-arrow"), /min-width:\s*2\.75rem/, "an arrow's target is narrower than 44px");
  assert.match(cssBlock(".read-steer-button"), /min-height:\s*2\.75rem/, "an arrow's target is shorter than 44px");
  // On a phone the group stands at the right edge, under the right thumb, not spread to both edges.
  const phone = cssRules(CSS_CODE, ":root[data-read-strip] #read-steer").join("\n");
  assert.match(phone, /justify-content:\s*flex-end/, "on a phone the arrows do not stand at the right edge");
  assert.doesNotMatch(phone, /space-between/, "on a phone the arrows are still spread to the two edges");
});

test("`#627 scrubber-thumb-arrows`: the fat arrows jump between the sections a message's headings begin", async () => {
  const { page, rows } = await blockedRead({ html: SECTIONED_HTML, kinds: SECTIONED_KINDS, content: SECTIONED_CONTENT });
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 5");
  assert.equal(page.el("read-section-next").disabled, false, "Next section is not offered before the second heading");
  await page.el("read-section-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "4 of 5", "Next section did not land on the second heading");
  assert.equal(blockOf(page.audioStreams.at(-1).url), "3");
  assert.equal(page.el("read-section-next").disabled, true, "Next section is offered in the last section");
  const before = page.audioStreams.length;
  await page.el("read-section-next").click();
  await page.settle();
  assert.equal(page.audioStreams.length, before, "Next section in the last section asked for something");
  // Previous section from a section's first block: the one before. From inside one: its own start.
  await page.el("read-section-prev").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 5", "Previous section from the second heading did not go to the first");
  await page.el("read-next").click();
  await page.settle();
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "3 of 5", "the thin Next did not step one block");
  await page.el("read-section-prev").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 5", "Previous section inside a section did not go to its start");
  // On the first section it starts it again, as Previous on the first block does.
  await page.el("read-section-prev").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 5");
  assert.equal(blockOf(page.audioStreams.at(-1).url), "0", "Previous section on the first section did not start it again");
});

test("`#627 scrubber-thumb-arrows`: with no headings a section is a block whole, a list of items one section", async () => {
  // The three-block fixture: a paragraph, then a list of two items. Its sections: the paragraph, the list.
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-section-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "Next section did not go to the list");
  assert.equal(page.el("read-section-next").disabled, true, "the list's second item was taken for a section of its own");
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "3 of 3");
  await page.el("read-section-prev").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "Previous section inside the list did not go to the list's start");
});

test("`#627 scrubber-thumb-arrows`: a paragraph's pieces are one section, and the media keys still step one block", async () => {
  // A long paragraph read in two pieces, then a paragraph: sections at 0 and 2.
  const kinds = [{ kind: "paragraph", spoken: true, element: 0, piece: 0, pieces: 2 },
    { kind: "paragraph", spoken: true, element: 0, piece: 1, pieces: 2 }, { kind: "paragraph", spoken: true, element: 1 }];
  const { page, rows } = await blockedRead({ mediaSession: true, kinds,
    html: "<p>The run finished. The budget landed.</p>\n<p>The rewrite is a draft.</p>\n",
    content: "The run finished. The budget landed.\n\nThe rewrite is a draft." });
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-section-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "3 of 3", "Next section stopped at a paragraph's second piece");
  page.mediaSession.handlers.previoustrack();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "the media key's previous did not step one block");
});

test("THE MEDIA KEYS move the read: previoustrack, nexttrack, pause and play, kept by a silent element", async () => {
  // A steering wheel's or a headset's buttons reach a page only while it plays a media element,
  // and Web Audio is not one: the page plays silence alongside the read to hold them.
  const { page, rows } = await blockedRead({ mediaSession: true });
  await rows[0].dispatch("click", {});
  await page.settle();
  const session = page.mediaSession;
  assert.deepEqual(Object.keys(session.handlers).sort(), ["nexttrack", "pause", "play", "previoustrack", "stop"]);
  assert.equal(session.playbackState, "playing");
  const keeper = page.players.find((player) => /^blob:/.test(player.url));
  assert.ok(keeper && keeper.playCount >= 1 && keeper.loop === true, "no silent element holds the media keys");

  session.handlers.nexttrack();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3");
  session.handlers.previoustrack();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 3");

  page.audio.state = "running";
  page.audio.suspend = async () => { page.audio.state = "suspended"; };
  session.handlers.pause();
  await page.settle();
  assert.equal(page.audio.state, "suspended", "pause did not stop the audio clock");
  assert.equal(session.playbackState, "paused");
  assert.equal(keeper.paused, true, "the silent element played on through a pause");
  assert.equal(page.el("read-steer-count").text(), "1 of 3", "pausing moved the read");
  session.handlers.play();
  await page.settle();
  assert.equal(page.audio.state, "running", "play did not start the audio clock again");
  assert.equal(session.playbackState, "playing");

  // A media key's jump keeps the read's keys and its silence: nothing in between let them go.
  const statesBefore = session.states.length;
  session.handlers.nexttrack();
  await page.settle();
  assert.ok(!session.states.slice(statesBefore).includes("none"), `a jump let the media keys go: ${session.states}`);

  // When the read stops, the keys, the title and the silence go with it. The media stop is the
  // way used here: the Read button is Pause while a read plays (`#434 read-aloud-pause`).
  session.handlers.stop();
  await page.settle();
  assert.equal(session.playbackState, "none");
  assert.equal(keeper.paused, true);
  assert.equal(session.handlers.nexttrack, null, "the page kept the media keys after the read");
  assert.equal(session.metadata, null);
});

test("...and on a browser without Web Audio, pause and play pause and play the element reading", async () => {
  const page = newPage();
  page.enableMediaSession();
  page.speechBlocks[BLOCKED_ID] = BLOCKED_KINDS;
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: BLOCKED_ID,
    content: "The run finished.\n\n- the budget landed\n- the rewrite is a draft", content_html: BLOCKED_HTML })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  const reading = page.players.find((player) => /block=0/.test(player.url));
  assert.ok(reading, "the first block was not handed to an <audio>");
  page.mediaSession.handlers.pause();
  await page.settle();
  assert.equal(reading.paused, true, "pause left the element playing");
  const plays = reading.playCount;
  page.mediaSession.handlers.play();
  await page.settle();
  assert.equal(reading.playCount, plays + 1, "play did not play the element again");
});

test("`#434 read-aloud-pause`: THE READ BUTTON AND THE MEDIA KEYS ARE ONE STATE, AND THE STRIP HAS NO PAUSE OF ITS OWN", async () => {
  // The owner: play/pause never reached the Read button. One state path, said two ways: the Read
  // button and the media session. `#537 scrub-bar-no-play-pause`: the strip's own Pause is gone.
  const { page, rows } = await blockedRead({ mediaSession: true });
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Pause", "the Read button does not say Pause while a read plays");
  assert.equal(page.el("read-steer-count").text(), "1 of 3");

  // Pause from the Read button: the clock stops, the position stays, every control agrees.
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.audio.state, "suspended", "the Read button's Pause did not stop the audio clock");
  assert.equal(page.el("read-steer-count").text(), "1 of 3", "pausing moved the read");
  assert.equal(page.el("read-aloud-label").text(), "Resume", "the Read button does not say Resume while paused");
  assert.equal(page.mediaSession.playbackState, "paused", "the media session was not told of the pause");

  // Resume from the Read button: the same state.
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.audio.state, "running", "the Read button's Resume did not start the clock again");
  assert.equal(page.el("read-aloud-label").text(), "Pause");
  assert.equal(page.el("read-aloud-label").text(), "Pause");
  assert.equal(page.mediaSession.playbackState, "playing");

  // Pause from a media key, resume from the Read button: still the one state.
  page.mediaSession.handlers.pause();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Resume", "a media-key pause left the Read button saying Pause");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.audio.state, "running", "the Read button did not resume a media-key pause");
  assert.equal(page.mediaSession.playbackState, "playing");

  // A jump made while held cues the next block up HELD: Next is not a Play nobody pressed.
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.audio.state, "suspended");
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "Next while held did not cue the next block");
  assert.equal(page.audio.state, "suspended", "Next while held started the audio clock");
  assert.equal(page.el("read-aloud-label").text(), "Resume", "the cued block is not held");
  assert.equal(page.mediaSession.playbackState, "paused");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.audio.state, "running", "Play did not start the cued block");

  // Stopped, the Read button is the way out again and the strip is gone.
  page.mediaSession.handlers.stop();
  await page.settle();
  assert.equal(page.el("read-steer").hidden, true, "the strip outlived the read");
  assert.equal(page.el("read-aloud-label").text(), "Stop", "with nothing playing the Read button is not the way out");
});

test("`#434 read-aloud-pause`: a resume after the speech ticket ended re-prepares and reads the paused block again", async () => {
  // A pause can outlive the ten-minute ticket (#368 speech-ticket-gone territory): resuming on the
  // dead URL would fail the read part-way through. It prepares again and restarts the paused block.
  const { page, rows } = await blockedRead({ mediaSession: true });
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "the premise: the read is not on its second block");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.audio.state, "suspended", "the premise: the read is not paused");
  const before = page.prepareCalls.length;
  const streams = page.audioStreams.length;
  page.speechTicketSuffix = "-again";
  page.setClock(page.clock() + page.speechTicketTtlSeconds * 1000 + 1_000);
  await page.el("read-aloud").click();
  await page.settle();
  const again = page.prepareCalls.slice(before).filter((call) => (call.ids || []).includes(BLOCKED_ID)).length;
  assert.ok(again >= 1, "the resume on an ended ticket prepared nothing again");
  assert.ok(page.audioStreams.length > streams, "the resume asked for no audio again");
  const resumed = page.audioStreams.at(-1);
  assert.match(resumed.url, /-again\?/, `the resume played the URL whose ticket had ended: ${resumed.url}`);
  assert.equal(blockOf(resumed.url), "1", "the resume did not restart the paused block");
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "the resume moved the read off the paused block");
  assert.equal(page.audio.state, "running", "the resumed read is not playing");
  assert.equal(page.el("read-aloud-label").text(), "Pause");
});

test("`#434 read-aloud-pause`: the device voice stops at the sentence on Pause and says it again on Resume, from the Read button", async () => {
  // `#554 read-pause-reply-resume`: never the engine's own pause, which Chrome on Android does not have
  // (its pause stops the voice and its resume does nothing, so a resume that asked the engine was silent).
  const page = devicePage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001", content: "One short message." })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, 1, "the premise: nothing is being said");
  assert.equal(page.el("read-aloud-label").text(), "Pause");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.deviceSpeech.pauses, 0, "Pause asked the engine for a pause Chrome on Android does not have");
  assert.equal(page.deviceSpeech.cancellations, 1, "Pause did not stop the device voice");
  assert.equal(page.el("read-aloud-label").text(), "Resume");
  // The cancelled utterance's late events are not the read ending or failing.
  page.deviceSpeech.utterances[0].fail("interrupted");
  page.deviceSpeech.utterances[0].end();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Resume", "the stopped utterance's events ended the held read");
  assert.deepEqual(page.dismissCalls, [], "a paused read was archived as heard");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, 2, "Resume said nothing");
  assert.equal(page.deviceSpeech.utterances[1].text, "One short message.", "Resume did not say the paused sentence again");
  assert.equal(page.el("read-aloud-label").text(), "Pause");
  page.deviceSpeech.utterances[1].end();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [{ messages: ["1000000000000000001"] }], "the resumed read did not finish as heard");
});

test("`#554 read-pause-reply-resume`: the device voice says one sentence at a time and resumes at the sentence it was paused in", async () => {
  const page = devicePage();
  await signIn(page);
  const sentences = [
    "Before we cut the release, is the migration finished?",
    // A sentence closed inside curly quotes ends there too.
    "Whether the dashboards picked up the \u201cnew partitions.\u201d",
    "Ok then.",
    "And the alert we silenced on Friday should be back on.",
  ];
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001", content: sentences.join(" ") })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  const said = () => page.deviceSpeech.utterances.map((one) => one.text);
  assert.deepEqual(said(), [sentences[0]], "the first utterance is not the first sentence alone");
  page.deviceSpeech.utterances[0].end();
  await page.settle();
  // A sentence too short to stand alone is said with the next.
  assert.deepEqual(said(), [sentences[0], sentences[1]]);
  await page.el("read-aloud").click();
  await page.settle();
  // The reader is away for as long as a reply takes: no watchdog gives the held read up meanwhile.
  page.expireTimers(sourceConstant("BROWSER_SPEECH_FINISH_MS"));
  await page.settle();
  assert.equal(page.el("error").textContent, "", "the held read timed out");
  assert.equal(page.el("read-aloud-label").text(), "Resume");
  await page.el("read-aloud").click();
  await page.settle();
  assert.deepEqual(said(), [sentences[0], sentences[1], sentences[1]], "Resume did not start the paused sentence again");
  page.deviceSpeech.utterances[2].end();
  await page.settle();
  assert.equal(said().at(-1), `${sentences[2]} ${sentences[3]}`);
  page.deviceSpeech.utterances.at(-1).end();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [{ messages: ["1000000000000000001"] }], "the resumed read did not finish as heard");
});

test("`#434 read-aloud-pause`: stopping a held device-voice read leaves the engine unpaused for the next", async () => {
  // An engine cancelled while paused can stay paused, and the next read would speak into that
  // paused queue and die on its start watchdog. `#554 read-pause-reply-resume`: a held read no
  // longer pauses the engine at all, so the engine is left paused here as something else left it.
  const page = devicePage();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", content: "First held message.", timestamp: "2026-08-19T14:00:00Z" }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "Second message.", timestamp: "2026-08-19T14:01:00Z" }),
  ]);
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.deviceSpeech.paused, false, "Pause held the engine, which Chrome on Android cannot resume");
  page.deviceSpeech.paused = true;
  await page.el("discord-log").children[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.deviceSpeech.paused, false, "stopping the held read left the engine paused");
  const spoken = page.deviceSpeech.utterances.length;
  await page.el("discord-log").children[1].dispatch("click", {});
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, spoken + 1, "the next read never reached the engine");
  assert.equal(page.deviceSpeech.paused, false, "the next read is speaking into a paused engine");
});

test("`#433 read-strip-low`: the strip hugs the chips' measured height and the pill stacks over it", () => {
  // The gap was a fixed 2.6rem kept for the status pill even while it was hidden. The strip now
  // stands on the chips' own measured height, and what floats higher keeps room for the strip.
  const strip = cssBlock("#read-steer");
  assert.match(strip, /bottom:\s*calc\(var\(--chips-foot, 0\.5rem\) \+ var\(--scroll-tools-height, 0px\)\);/,
    "the strip does not stand on the chips' measured top edge");
  assert.doesNotMatch(strip, /2\.6rem/, "the fixed gap over the chips is still reserved");
  assert.match(cssBlock("#status-line"), /bottom:[^;]*--read-steer-clearance/,
    "the status pill does not stack over the strip and would cover it");
  const own = cssRules(CSS_CODE, "#channel-composer").find((rule) => /margin-top:/.test(rule));
  assert.match(own, /margin-top:[^;]*--read-steer-clearance/,
    "the list's foot keeps no room for the strip, so its last message cannot scroll clear of it");
  assert.match(SCRIPT_CODE, /"--scroll-tools-height"/, "nothing writes the chips' measured height");
  assert.match(SCRIPT_CODE, /"--read-steer-clearance"[\s\S]*?\.observe\(el\("read-steer"\)\)/,
    "nothing measures the strip for the room kept over it");
});

test("`#442 read-strip-order`: the strip reads its arrows and k of n in the markup as on screen, and holds no Pause or Stop (`#537`)", () => {
  // In the markup, not by `order`, so Tab and a screen reader meet the controls as they are seen. The
  // desk's grip comes first, at the strip's left. `#537 scrub-bar-no-play-pause`, the owner: "let's
  // remove the pause/play from the floating scrub bar. I like how we currently have stop and pause on
  // the lower bar. We don't need to redundantly have it. We can just have the next and previous".
  const strip = HTML_CODE.slice(HTML_CODE.indexOf('id="read-steer"'), HTML_CODE.indexOf('id="read-timeline"'));
  const ids = [...strip.matchAll(/\bid="(read-[\w-]+)"/g)].map((match) => match[1]);
  // `#627 scrubber-thumb-arrows`: the fat Previous and Next section, the count, the thin Previous and Next block.
  assert.deepEqual(ids, ["read-steer", "read-steer-grip", "read-section-prev", "read-section-next", "read-steer-count",
    "read-prev", "read-next"], "the strip's controls are not the grip, the section arrows, the count and the block arrows alone");
  assert.ok(!PAGE_ELEMENTS.has("read-pause") && !PAGE_ELEMENTS.has("read-stop"), "the strip still has a Pause or a Stop of its own");
  // The bar under the list keeps both: Read (the Pause of a read playing) and Stop beside it.
  assert.ok(PAGE_ELEMENTS.has("read-aloud") && PAGE_ELEMENTS.has("read-aloud-stop"), "the bar lost its Pause or its Stop");
  for (const selector of ["#read-steer", ".read-steer-button", ".read-steer-arrow", "#read-steer-grip", "#read-section-prev",
    "#read-section-next", "#read-prev", "#read-steer-count", "#read-next"]) {
    for (const rule of cssRules(CSS_CODE, selector)) {
      assert.doesNotMatch(rule, /(^|[\s;])order:|row-reverse|direction:\s*rtl/,
        `${selector} moves the strip's controls on screen away from their order in the markup`);
    }
  }
});

/** Reading mode is off: the Read button says Read and is not the way-out colour, nothing plays. */
function assertReadingModeLeft(page, why) {
  assert.equal(page.el("read-aloud-label").text(), "Read", `${why}: the Read button does not say Read`);
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "false", `${why}: reading mode is still on`);
  assert.equal(page.el("read-steer").hidden, true, `${why}: the strip outlived the read`);
  assert.equal(page.el("read-aloud-stop").hidden, true, `${why}: the bar's Stop outlived the read`);
}

test("`#442 read-strip-order`: the bar's Stop ends the read and leaves reading mode in one tap", async () => {
  // The owner: once Read became Pause (`#434`) there was "no way then to STOP or exit read mode".
  // `#537 scrub-bar-no-play-pause`: the bar's Stop is the one; the strip has none.
  const { page, rows } = await blockedRead({ mediaSession: true });
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-steer").hidden, false, "the premise: the strip is up");
  page.el("read-aloud-stop").focus();
  await page.el("read-aloud-stop").click();
  await page.settle();
  assertReadingModeLeft(page, "the bar's Stop");
  assert.equal(page.document.activeElement, page.el("read-aloud"), "the focus went with the Stop that vanished");
  assert.equal(page.mediaSession.playbackState, "none", "the media session still holds a stopped read");

  // Held, too: Stop is not a Play, and a paused read stops the same way.
  await page.el("read-aloud").click();
  await page.settle();
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Resume", "the premise: the read is held");
  await page.el("read-aloud-stop").click();
  await page.settle();
  assertReadingModeLeft(page, "the bar's Stop on a held read");
});

test("`#442 read-strip-order`: Read is Pause with Stop beside it while a read plays, and only then", async () => {
  const { page, rows } = await blockedRead();
  assert.equal(page.el("read-aloud-stop").hidden, true, "Stop stands in the bar with reading mode on and nothing playing");
  assert.equal(page.el("read-aloud-label").text(), "Stop", "the premise: with nothing playing Read is the way out");
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Pause");
  assert.equal(page.el("read-aloud-stop").hidden, false, "a read plays and the bar has no Stop beside its Pause");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Resume");
  assert.equal(page.el("read-aloud-stop").hidden, false, "a held read lost the bar's Stop");
  await page.el("read-aloud-stop").click();
  await page.settle();
  assertReadingModeLeft(page, "the bar's Stop");

  // A message of one block has no strip; the bar's Stop is its one-tap way out.
  await page.el("read-aloud").click();
  await page.settle();
  await rows[1].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-steer").hidden, true, "the premise: a one-block read has no strip");
  assert.equal(page.el("read-aloud-stop").hidden, false, "a one-block read has no Stop in the bar");
  await page.el("read-aloud-stop").click();
  await page.settle();
  assertReadingModeLeft(page, "the bar's Stop on a one-block read");
});

test("`#442 read-strip-order`: a jump that waits for its block keeps Pause and Stop where they were", async () => {
  // A ticket near its end makes a jump prepare again before the next block plays, and for that
  // wait nothing is playing. The bar must not fold Stop away and turn Read into a Stop under the
  // thumb meanwhile: a tap meant to pause would leave reading mode.
  const page = newPage();
  page.enableStreamedAudio();
  page.speechBlocks[BLOCKED_ID] = BLOCKED_KINDS;
  page.speechTicketTtlSeconds = 200;
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: BLOCKED_ID,
    content: "The run finished.\n\n- the budget landed\n- the rewrite is a draft", content_html: BLOCKED_HTML })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 3");
  await page.el("read-next").click();
  assert.equal(page.el("read-aloud-stop").hidden, false, "the bar's Stop folded away during the jump");
  assert.equal(page.el("read-aloud-label").text(), "Pause", "Read turned into a Stop under the thumb during the jump");
  await page.el("read-aloud").click();
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "true", "a tap on Pause during the jump left reading mode");
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "the jump did not land");
  assert.equal(page.el("read-aloud-stop").hidden, false);
});

/** What the root tells the stylesheet about the strip (`scrubber-full-width`): "playing", "paused" or null. */
const readStrip = (page) => page.documentElement.getAttribute("data-read-strip");

test("`scrubber-full-width`: the root says whether the strip's read plays or is held, and nothing once it stops", async () => {
  // The owner, 2026-10-27: the scrubber "full width and flush with the top of the bottom row of the
  // controls. Those other controls you don't need to access while we are in active playback mode."
  // The stylesheet hides them by this attribute; held, only the second row stays away, and a stop
  // brings everything back.
  const { page, rows } = await blockedRead();
  assert.equal(readStrip(page), null, "the root claims a strip with reading mode on and nothing playing");
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-steer").hidden, false, "the premise: the strip is up");
  assert.equal(readStrip(page), "playing", "a read playing does not lay the phone out for playback");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Resume", "the premise: the read is held");
  assert.equal(readStrip(page), "paused", "a held read still says it plays, so the chips stay away");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(readStrip(page), "playing", "the resumed read does not say it plays again");
  await page.el("read-aloud-stop").click();
  await page.settle();
  assertReadingModeLeft(page, "the bar's Stop");
  assert.equal(readStrip(page), null, "the playback layout outlived the read");

  // A tap on the block being read stops it too; a message of one block draws no strip, and no layout.
  await page.el("read-aloud").click();
  await page.settle();
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(readStrip(page), "playing");
  await rows[0].dispatch("click", { target: blockElements(rows[0])[0] });
  await page.settle();
  assert.equal(page.el("read-steer").hidden, true, "the premise: the tap stopped the read");
  assert.equal(readStrip(page), null, "a read stopped by a tap kept the playback layout");
  await rows[1].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Pause", "the premise: the one-block read plays");
  assert.equal(readStrip(page), null, "a one-block read, which draws no strip, laid the phone out for one");
});

test("`scrubber-full-width`: a jump that waits for its block keeps the strip and the layout, and a Stop then ends both", async () => {
  // On a phone the strip is a row of the layout: hidden for a jump's wait, the list would grow into its
  // room and the second row and the chips come back for the moment of a Next.
  const page = newPage();
  page.enableStreamedAudio();
  page.speechBlocks[BLOCKED_ID] = BLOCKED_KINDS;
  page.speechTicketTtlSeconds = 200;
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: BLOCKED_ID,
    content: "The run finished.\n\n- the budget landed\n- the rewrite is a draft", content_html: BLOCKED_HTML })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  const blocks = page.el("read-timeline").children.length;
  await page.el("read-next").click();
  assert.equal(page.el("read-steer").hidden, false, "the strip went for the jump's wait");
  assert.equal(page.el("read-steer-count").text(), "1 of 3", "the held strip does not say where the read was");
  assert.equal(page.el("read-timeline").children.length, blocks, "the held strip lost its timeline");
  assert.equal(readStrip(page), "playing", "the playback layout dropped for the jump's wait");
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "the jump did not land");
  assert.equal(readStrip(page), "playing");

  await page.el("read-next").click();
  assert.equal(page.el("read-steer").hidden, false, "the premise: the strip is held for the jump");
  await page.el("read-aloud-stop").click();
  await page.settle();
  assertReadingModeLeft(page, "the bar's Stop during a jump");
  assert.equal(readStrip(page), null, "the playback layout outlived a read stopped mid-jump");
  assert.equal(page.el("read-timeline").children.length, 0, "the timeline outlived the read");
});

/** Every `@media (pointer: coarse)` body in the stylesheet, joined: the rules a phone gets. */
function coarseBodies() {
  const bodies = [];
  let from = 0;
  for (;;) {
    const at = CSS_CODE.indexOf("@media (pointer: coarse)", from);
    if (at < 0) return bodies.join("\n");
    const open = CSS_CODE.indexOf("{", at);
    let depth = 0;
    let end = open;
    for (; end < CSS_CODE.length; end += 1) {
      if (CSS_CODE[end] === "{") depth += 1;
      else if (CSS_CODE[end] === "}" && (depth -= 1) === 0) break;
    }
    bodies.push(CSS_CODE.slice(open + 1, end));
    from = end;
  }
}

test("`scrubber-full-width`: on a phone the strip playing is a full-width row on the dock, the second row and the chips hidden; a desk is untouched", () => {
  const phone = coarseBodies();
  const rule = (selector) => {
    const blocks = cssRules(phone, selector);
    assert.ok(blocks.length > 0, `no phone rule for "${selector}"`);
    return blocks.join("\n");
  };
  const strip = rule(":root[data-read-strip] #read-steer");
  assert.match(strip, /bottom:\s*0;/, "the strip does not stand on the dock");
  assert.match(strip, /inset-inline:\s*0;/);
  assert.match(strip, /max-width:\s*none;/, "the strip is still narrower than the screen");
  assert.match(strip, /margin-inline:\s*0;/);
  assert.match(strip, /padding:[^;]*safe-area-inset-right[^;]*safe-area-inset-left[^;]*;/,
    "the full-width strip's insets are not the safe area's on both sides");
  // The list ends at its top edge: the screen keeps the strip's measured height under the list, and the
  // list's foot keeps no second room for it.
  assert.match(rule(":root[data-read-strip] #screen-main"), /padding-bottom:\s*var\(--read-steer-height, 0px\);/,
    "the list runs under the strip");
  assert.match(rule(":root[data-read-strip] #scroll-area"), /--read-steer-clearance:\s*0px;/,
    "the list's foot keeps room for a strip it no longer runs under");
  assert.match(rule(":root[data-read-strip] #frame-body"), /--chips-foot:\s*max\(calc\(var\(--read-steer-height, 0px\) \+ 0\.5rem\)/,
    "the chips and the pill do not stand on the strip");
  assert.match(SCRIPT_CODE, /"--read-steer-height"[\s\S]*?\.observe\(el\("read-steer"\)\)/, "nothing measures the strip's height");
  // Playing: the second row (in the dock or the header) and the chips go. Held: the second row only.
  assert.match(rule(":root[data-read-strip] #control-bar"), /display:\s*none;/, "the second row stays up during playback");
  // (By the text: the rule parser splits a selector at the commas inside its `:has`.)
  assert.match(phone, /:root\[data-read-strip\] #topbar:has\(#control-bar-top > #control-bar\):not\([^{]*\{\s*display:\s*none;/,
    "the header holding only the bar stays up during playback");
  // A header gone this way is not `hidden`, so the body takes the notch's inset over (`#197`).
  assert.match(phone, /:root\[data-read-strip\] #topbar:has\([^{]*\+ #frame-body\s*\{\s*padding-top:\s*env\(safe-area-inset-top\);/,
    "the header gone for playback leaves the list under the notch");
  assert.match(rule(":root[data-read-strip] #side-column"), /--read-steer-clearance:\s*0px;/,
    "the thread list keeps room for a strip it no longer runs under");
  assert.match(rule(':root[data-read-strip="playing"] #scroll-tools'), /display:\s*none;/, "the chips stay up while the read plays");
  assert.equal(cssRules(phone, ":root[data-read-strip] #scroll-tools").length + cssRules(phone, ':root[data-read-strip="paused"] #scroll-tools').length,
    0, "the chips are hidden while the read is held too");
  // A desk is untouched: every rule that reads the attribute is inside a phone's query.
  const uses = (css) => (css.match(/data-read-strip/g) || []).length;
  assert.equal(uses(CSS_CODE), uses(phone), "a playback layout rule applies outside a coarse pointer");
});

test("`#442 read-strip-order`: the phone tile gives Stop a fourth column on Pause's left, only while it shows", () => {
  // On Read's LEFT: Read, the Pause reached for without looking, keeps the edge under the thumb,
  // so a blind tap to pause never lands on a Stop that has just appeared where Read was.
  assert.match(cssBlock("#control-pane:has(#read-aloud-stop:not([hidden]))"),
    /grid-template-columns:\s*3\.5rem 1fr 1fr 1fr;/, "the tile has no column for Stop");
  const stop = cssBlock("#control-pane #read-aloud-stop");
  assert.match(stop, /grid-column:\s*3;/, "Stop is not beside Pause, on its left");
  assert.match(stop, /grid-row:\s*1 \/ span 2;/);
  assert.match(cssBlock("#control-pane:has(#read-aloud-stop:not([hidden])) #read-aloud"), /grid-column:\s*4;/,
    "Read gave up the edge under the thumb to Stop");
  const pane = HTML_CODE.slice(HTML_CODE.indexOf('id="control-pane"'));
  assert.ok(pane.indexOf('id="read-speed"') < pane.indexOf('id="read-aloud-stop"') &&
    pane.indexOf('id="read-aloud-stop"') < pane.indexOf('id="read-aloud"'), "Stop is not between Pace and Read in the markup, so Tab reaches it out of order");
  assert.match(/<button id="read-aloud-stop"[^>]*>/.exec(HTML_CODE)[0], /(?:^|\s)hidden(?=[\s>])/, "the bar's Stop ships on screen");
});

/** A strip whose drawn box follows the offset the page writes, as the desk's stylesheet applies it. */
function deskStrip(page, { left = 450, top = 620, width = 380, height = 60, desk = true } = {}) {
  const strip = page.el("read-steer");
  page.el("read-steer-grip").getClientRects = () => (desk ? [{}] : []);
  strip.getBoundingClientRect = () => {
    const dx = desk ? parseFloat(strip.style.getPropertyValue("--read-steer-dx")) || 0 : 0;
    const dy = desk ? parseFloat(strip.style.getPropertyValue("--read-steer-dy")) || 0 : 0;
    return { left: left + dx, top: top + dy, right: left + dx + width, bottom: top + dy + height, width, height };
  };
  page.documentElement.clientWidth = 1280;
  page.documentElement.clientHeight = 800;
}

const stripOffset = (page) => [
  page.el("read-steer").style.getPropertyValue("--read-steer-dx"),
  page.el("read-steer").style.getPropertyValue("--read-steer-dy"),
];

test("`#442 read-strip-order`: on a desk the strip is dragged by its grip, held in the window, and remembered", async () => {
  const store = new Map();
  const { page, rows } = await blockedRead({ store });
  deskStrip(page);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.deepEqual(stripOffset(page), ["", ""], "a strip never moved is not docked");
  const grip = page.el("read-steer-grip");
  await grip.dispatch("pointerdown", { button: 0, pointerId: 1, clientX: 460, clientY: 640 });
  await grip.dispatch("pointermove", { pointerId: 1, clientX: 400, clientY: 340 });
  assert.deepEqual(stripOffset(page), ["-60px", "-300px"], "the strip does not follow the pointer");
  await grip.dispatch("pointerup", { pointerId: 1 });
  assert.equal(page.el("read-steer").getAttribute("data-moved"), "true");
  assert.match(store.get("vibe-talk.voice.read-steer-offset") || "", /"x":-60/, "the place is not remembered");

  // Held inside the window: dragged far off its top-left corner, it stops at the corner.
  await grip.dispatch("pointerdown", { button: 0, pointerId: 1, clientX: 400, clientY: 340 });
  await grip.dispatch("pointermove", { pointerId: 1, clientX: -2000, clientY: -2000 });
  await grip.dispatch("pointerup", { pointerId: 1 });
  assert.deepEqual(stripOffset(page), ["-450px", "-620px"], "the strip left the window");
  // The keyboard: an arrow moves it, Home docks it, as a double-click does.
  await grip.dispatch("keydown", { key: "ArrowRight" });
  assert.deepEqual(stripOffset(page), ["-438px", "-620px"], "ArrowRight did not move the strip");
  await grip.dispatch("keydown", { key: "Home" });
  assert.deepEqual(stripOffset(page), ["", ""], "Home did not dock the strip");
  assert.equal(store.has("vibe-talk.voice.read-steer-offset"), false, "a docked strip still has a place remembered");
  await grip.dispatch("keydown", { key: "ArrowUp", shiftKey: true });
  assert.deepEqual(stripOffset(page), ["0px", "-48px"], "Shift+ArrowUp did not move the strip by the larger step");

  // A reload puts it back where it was left, the next time a read shows it. (Only the strip's key
  // carried over: the rest of this storage remembers the channel view, which `blockedRead` opens.)
  const key = "vibe-talk.voice.read-steer-offset";
  const again = await blockedRead({ store: new Map([[key, store.get(key)]]) });
  deskStrip(again.page);
  await again.rows[0].dispatch("click", {});
  await again.page.settle();
  assert.deepEqual(stripOffset(again.page), ["0px", "-48px"], "the strip forgot where it was put");
  await again.page.el("read-steer-grip").dispatch("dblclick");
  assert.deepEqual(stripOffset(again.page), ["", ""], "a double-click on the grip did not dock the strip");
});

test("`#442 read-strip-order`: a phone keeps the strip docked whatever a desk remembered", async () => {
  const store = new Map([["vibe-talk.voice.read-steer-offset", JSON.stringify({ x: -100, y: -200 })]]);
  const { page, rows } = await blockedRead({ store });
  deskStrip(page, { desk: false });
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.deepEqual(stripOffset(page), ["", ""], "a phone's strip moved off the chips row");
  await page.el("read-steer-grip").dispatch("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
  await page.el("read-steer-grip").dispatch("pointermove", { pointerId: 1, clientX: 90, clientY: 90 });
  assert.deepEqual(stripOffset(page), ["", ""], "a strip with no grip drawn was dragged");
  // The stylesheet applies the offset, and draws the grip, only in the desk's regime.
  assert.match(cssBlockIn(DESKTOP_QUERY, "#read-steer"), /translate:\s*var\(--read-steer-dx, 0px\) var\(--read-steer-dy, 0px\)/);
  assert.doesNotMatch(cssBlockOutside(DESKTOP_QUERY, "#read-steer"), /translate/, "the offset applies on a phone");
  assert.match(cssBlockOutside(DESKTOP_QUERY, "#read-steer-grip"), /display:\s*none/, "the grip is drawn on a phone");
  assert.match(cssBlockIn(DESKTOP_QUERY, "#read-steer-grip"), /display:\s*inline-flex/);
});

test("`#451 read-strip-followups`: a read started with s outside reading mode has the bar's Stop, and Read pauses it", async () => {
  // From `#442`'s handoff: s reads one message without entering the mode, and that read had no
  // bar Stop — Read, pressed to stop it, entered reading mode instead and left it playing.
  const page = newPage();
  page.enableStreamedAudio();
  page.enableAbort();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "one block" }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "another block",
      timestamp: "2026-08-19T05:31:00.000Z" }),
  ]);
  assert.equal(page.el("read-aloud-label").text(), "Read", "the premise: reading mode is off");
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "false", "the premise: reading mode is off");
  assert.equal(page.el("read-aloud-stop").hidden, true, "the premise: no Stop before anything plays");
  await press(page, page.el("scroll-area"), "ArrowDown");
  await press(page, page.el("scroll-area"), "s");
  await page.settle();
  assert.equal(page.el("read-steer").hidden, true, "the premise: a one-block read has no strip");
  assert.equal(page.el("read-aloud-stop").hidden, false, "a read started with s has no Stop in the bar");
  assert.equal(page.el("read-aloud-label").text(), "Pause", "Read does not pause a read started with s");
  assert.equal(page.el("read-aloud").getAttribute("aria-pressed"), "false", "starting the read entered reading mode");
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "true", "the one-off read does not light its Pause as a session's does");
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Resume", "Read did not hold the read started with s");
  assert.equal(page.audio.state, "suspended", "the held read's clock is still running");
  assert.equal(page.el("read-aloud").getAttribute("aria-pressed"), "false", "pausing it entered reading mode");
  assert.equal(page.el("read-aloud-stop").hidden, false, "a held read started with s lost the bar's Stop");
  await page.el("read-aloud-stop").click();
  await page.settle();
  assertReadingModeLeft(page, "the bar's Stop on a read started with s");

  // Still being fetched, it cannot be paused, so Read itself is the Stop that cancels it — and
  // pressing it there does not enter the mode over the fetch. The preparation is held open, so
  // the read stays in that window until it is let go, by which time nobody wants it.
  let releasePrepare = () => {};
  const prepareHeld = new Promise((resolve) => { releasePrepare = resolve; });
  page.prepareHold = () => prepareHeld;
  // The second message: the first's preparation survived its read, so asking for it again would
  // play at once and never stand in the window this half is about.
  await press(page, page.el("scroll-area"), "ArrowDown");
  await press(page, page.el("scroll-area"), "s");
  assert.equal(page.el("read-aloud-label").text(), "Stop", "a read still being fetched is not offered its Stop");
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "true", "the fetched read lights no way out");
  assert.equal(page.el("read-aloud").getAttribute("aria-pressed"), "false", "the fetched read entered reading mode");
  await page.el("read-aloud").click();
  await page.settle();
  assertReadingModeLeft(page, "Read as Stop on a read still being fetched");
  releasePrepare();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Read", "the cancelled fetch still started a read");
  assertReadingModeLeft(page, "the cancelled fetch let go");
});

test("`#451 read-strip-followups`: a moved strip is held in the window again when the chips row grows", async () => {
  // The strip's place is an offset from where it docks on the chips row. The row growing — a chip
  // arriving, the row wrapping — lifts that docked place, and an offset clamped against the old
  // place can stand the strip partly out of the window with no resize to pull it back.
  const page = newPage(new Map(), SCRIPT, (p) => p.enableResizeObserver());
  page.enableStreamedAudio();
  page.enableAbort();
  page.speechBlocks[BLOCKED_ID] = BLOCKED_KINDS;
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: BLOCKED_ID, content: "The run finished.\n\n- the budget landed\n- the rewrite is a draft",
      content_html: BLOCKED_HTML }),
  ]);
  const base = { left: 450, top: 620 };
  const strip = page.el("read-steer");
  page.el("read-steer-grip").getClientRects = () => [{}];
  strip.getBoundingClientRect = () => {
    const dx = parseFloat(strip.style.getPropertyValue("--read-steer-dx")) || 0;
    const dy = parseFloat(strip.style.getPropertyValue("--read-steer-dy")) || 0;
    return { left: base.left + dx, top: base.top + dy, right: base.left + dx + 380,
      bottom: base.top + dy + 60, width: 380, height: 60 };
  };
  page.documentElement.clientWidth = 1280;
  page.documentElement.clientHeight = 800;
  await rows[0].dispatch("click", {});
  await page.settle();
  const grip = page.el("read-steer-grip");
  await grip.dispatch("pointerdown", { button: 0, pointerId: 1, clientX: 460, clientY: 640 });
  await grip.dispatch("pointermove", { pointerId: 1, clientX: -2000, clientY: -2000 });
  await grip.dispatch("pointerup", { pointerId: 1 });
  assert.deepEqual(stripOffset(page), ["-450px", "-620px"], "the premise: the strip is not held at the window's corner");
  // The chips row grows by 60px, so the docked place stands 60px higher; the window does not change.
  base.top = 560;
  page.el("scroll-tools").textContent = "Expand all";
  page.resized(page.el("scroll-tools"));
  await page.settle();
  assert.deepEqual(stripOffset(page), ["-450px", "-560px"],
    "the chips grew and left the moved strip 60px out of the window's top");
  // And the held place is where the next grip key starts from, not the offset the growth overtook.
  await grip.dispatch("keydown", { key: "ArrowDown" });
  assert.deepEqual(stripOffset(page), ["-450px", "-548px"],
    "a grip key after the growth started from the place the window no longer allows");
  // The composer's growth holds it by the same call, where the box's top edge is written: the
  // fixture lays no box out, so this half is read where it is written, as its neighbours are.
  const composerTop = SCRIPT_CODE.indexOf('"--composer-top"');
  assert.ok(composerTop >= 0 && SCRIPT_CODE.slice(composerTop, composerTop + 400).includes("placeReadSteer()"),
    "the composer moving the strip's band holds no moved strip in the window");
});

test("`#451 read-strip-followups`: the list of keys names the reading strip grip's keys", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001", content: "one block" })]);
  const listed = page.el("keyboard-keys").descendants().filter((node) => node.tagName === "kbd").map((node) => node.textContent);
  const said = page.el("keyboard-keys").text();
  assert.ok(listed.some((keys) => keys.includes("Shift+") && keys.includes("←")),
    `the grip's Shift+arrows are not in the list of keys: ${listed.join(", ")}`);
  assert.match(said, /reading controls' grip/, "the list does not say whose arrows those are");
  assert.match(said, /Home docks|docks it on the chips row/, "the list does not say Home docks the strip");
  const shortcuts = page.el("read-steer-grip").getAttribute("aria-keyshortcuts") || "";
  assert.match(shortcuts, /ArrowLeft/, "the grip does not say its arrows to a screen reader");
  assert.match(shortcuts, /Home/, "the grip does not say Home to a screen reader");
});

test("a read in blocks on a ticket with under five minutes left prepares the row again first", async () => {
  // Its last block is asked for minutes after the tap; a ticket that ends part-way through is a
  // read that stops with "could not be played".
  const page = newPage();
  page.enableStreamedAudio();
  page.speechBlocks[BLOCKED_ID] = BLOCKED_KINDS;
  page.speechTicketTtlSeconds = 200;
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: BLOCKED_ID,
    content: "The run finished.\n\n- the budget landed\n- the rewrite is a draft", content_html: BLOCKED_HTML })]);
  const before = page.prepareCalls.length;
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.prepareCalls.length, before + 1, "the read started on a ticket about to end");
  assert.deepEqual(page.prepareCalls.at(-1).ids, [BLOCKED_ID]);
  // THE CONTROL: with the whole ten minutes the tap prepares nothing.
  const fresh = newPage();
  fresh.enableStreamedAudio();
  fresh.speechBlocks[BLOCKED_ID] = BLOCKED_KINDS;
  await signIn(fresh);
  const freshRows = await inReadingMode(fresh, [message({ id: BLOCKED_ID,
    content: "The run finished.\n\n- the budget landed\n- the rewrite is a draft", content_html: BLOCKED_HTML })]);
  const freshBefore = fresh.prepareCalls.length;
  await freshRows[0].dispatch("click", {});
  await fresh.settle();
  assert.equal(fresh.prepareCalls.length, freshBefore, "a fresh ticket was prepared again");
});

test("blocks the page draws DIFFERENTLY from the server's list still read block by block, with no block lit", async () => {
  // The negative control for the mapping: when the two walks disagree, lighting block k would light
  // the wrong words. The row stays lit and counted; no block is.
  const { page, rows } = await blockedRead({ kinds: [{ kind: "paragraph", spoken: true },
    { kind: "quote", spoken: true }, { kind: "item", spoken: true }] });
  await rows[0].dispatch("click", {});
  await page.settle();
  const row = page.el("discord-log").children[0];
  assert.equal(blockOf(page.audioStreams[0].url), "0");
  assert.equal(page.el("read-steer-count").text(), "1 of 3");
  assert.equal(litBlock(row), "", "a block was lit though the page and the server disagree about the blocks");
});

test("a block that says nothing is drawn and skipped, and a message of ONE block reads whole as before", async () => {
  const { page, rows } = await blockedRead({ kinds: [{ kind: "paragraph", spoken: true },
    { kind: "item", spoken: false }, { kind: "item", spoken: true }] });
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 2");
  page.audioStreams[0].controller.bytes(joined(wavHeader(1000, 1), pcm(0x2000)));
  await page.settle();
  assert.equal(blockOf(page.audioStreams[1].url), "2", "the silent block was asked for");

  await rows[1].dispatch("click", {});
  await page.settle();
  const whole = page.audioStreams.at(-1);
  assert.match(whole.url, /ticket-for-1000000000000000002\?selection=tap-[0-9a-f]{16}$/, "a one-block message was read by block");
  assert.equal(page.el("read-steer").hidden, true, "a one-block read shows the steering strip");
  const mark = page.el("discord-log").children[1].descendants().find((n) => n.className === "msg-author reading-mark");
  assert.equal(mark.text(), "reading");
});

test("the blocks of a message heard to its end archive it once, after the last block ends", async () => {
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  const tenFrames = () => joined(wavHeader(1000, 1), pcm(...new Array(10).fill(0x2000)));
  for (let at = 0; at < 3; at += 1) {
    page.audioStreams[at].controller.bytes(tenFrames());
    await page.settle();
    page.audioStreams[at].controller.drop();
    await page.settle();
  }
  assert.equal(page.audio.sources.length, 3, "not every block was scheduled");
  for (const source of page.audio.sources.slice(0, -1)) source.onended();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [], "archived before the last block ended");
  page.audio.sources.at(-1).onended();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [{ messages: [BLOCKED_ID] }]);
  assert.equal(page.el("read-steer").hidden, true);
});

// --- `#379 read-aloud-sentences`: the sentence being said, inside the block being read ------------

/** A page reading a message whose first block is three sentences, with the highlight API or not. */
async function sentencedRead(withHighlights = true) {
  const page = newPage();
  page.enableStreamedAudio();
  page.enableAbort();
  const highlights = withHighlights ? page.enableHighlights() : null;
  page.speechBlocks[BLOCKED_ID] = [{ kind: "paragraph", spoken: true, chars: 40 },
    { kind: "item", spoken: true, chars: 1 }, { kind: "item", spoken: true, chars: 1 }];
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: BLOCKED_ID,
    content: "One two three. Four five six! Seven eight?\n\n- a\n- b",
    content_html: "<p>One two three. Four five six! Seven eight?</p>\n<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n" })]);
  return { page, rows, highlights };
}

/** The words the sentence highlight covers, or "" when there is none. */
function litSentence(highlights) {
  const lit = highlights.get("read-aloud-sentence");
  if (!lit) return "";
  const [range] = lit.ranges;
  assert.equal(range.start[0], range.end[0], "the fixture's sentences are each in one text node");
  return range.start[0].textContent.slice(range.start[1], range.end[1]);
}

test("THE SENTENCE BEING SAID is lit inside the block being read, by its share of the block's audio", async () => {
  // No request per sentence: each would be a pause as long as the voice's first audio.
  const { page, rows, highlights } = await sentencedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  const stream = page.audioStreams[0];
  // A fifth of a second has arrived, due half a second from now: the block's length is still the
  // estimate from what it says, 40 characters at 0.068 s, 2.72 s.
  stream.controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(200).fill(0x2000))));
  await page.settle();
  page.audio.currentTime = 0.5;
  assert.equal(page.expireTimers(150), 1, "nothing looked for the sentence being said");
  await page.settle();
  assert.equal(litSentence(highlights), "One two three.");
  // 1.4 s has arrived, and 1.36 s of it said: half the estimate, where by what has arrived it would
  // be nearly all of it.
  page.audio.currentTime = 0.6;
  stream.controller.bytes(pcm(...new Array(1200).fill(0x2000)));
  await page.settle();
  page.audio.currentTime = 0.5 + 1.36;
  page.expireTimers(150);
  await page.settle();
  assert.equal(litSentence(highlights), "Four five six!", "the estimate did not move the light on");
  // All of it arrives, before the first part has run out: two seconds long. Its length is now the
  // audio's own.
  page.audio.currentTime = 0.6;
  stream.controller.bytes(pcm(...new Array(600).fill(0x2000)));
  await page.settle();
  stream.controller.drop();
  await page.settle();
  page.audio.currentTime = 0.5 + 1.9;
  page.expireTimers(150);
  await page.settle();
  assert.equal(litSentence(highlights), "Seven eight?");
  page.audio.currentTime = 0.5 + 0.2;
  page.expireTimers(150);
  await page.settle();
  assert.equal(litSentence(highlights), "One two three.", "the light did not follow the audio back");
  // The block itself is still lit, and the sentence goes with the read. Stopped by a tap on the
  // row being read, as ever: the Read button pauses a playing read now (`#434`).
  assert.equal(litBlock(page.el("discord-log").children[0]), "p:One two three. Four five six! Seven eight?");
  await page.el("discord-log").children[0].dispatch("click", {});
  await page.settle();
  assert.equal(highlights.has("read-aloud-sentence"), false, "the sentence stayed lit after the read stopped");
  assert.equal(page.expireTimers(150), 0, "the page kept looking for a sentence with nothing being read");
});

test("a stall in the middle of a block holds the sentence light through the silence, and it never runs ahead", async () => {
  // `#472 gate-flake-hardening`. On a loaded host the browser check saw the light reach the third
  // sentence and go back to the second: an underrun's silence was counted as speech.
  const { page, rows, highlights } = await sentencedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  const stream = page.audioStreams[0];
  const look = async (at) => {
    page.audio.currentTime = at;
    page.expireTimers(150);
    await page.settle();
    return litSentence(highlights);
  };
  // A fifth of a second, due at 0.5 s, runs out at 0.7 s, and nothing more comes until 1.5 s: the
  // clock runs on through a silence nobody has measured yet, and 0.2 s of speech is still the first
  // sentence, not the second the clock alone reaches.
  stream.controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(200).fill(0x2000))));
  await page.settle();
  assert.equal(await look(1.5), "One two three.", "the light ran on into the stall");
  // The next piece comes at 1.5 s, due a doubled lead later, at 2.5 s: 1.8 s of silence after 0.2 s of
  // speech. 0.55 s said of an estimated 2.72 s is the first sentence still.
  stream.controller.bytes(pcm(...new Array(600).fill(0x2000)));
  await page.settle();
  assert.equal(await look(2.85), "One two three.", "the light ran on through the silence");
  // The rest arrives, 2.72 s of speech in all, and the block is whole. Its length is its speech,
  // not its speech and the silence: nothing goes back, and each sentence comes as it is said.
  page.audio.currentTime = 2.9;
  stream.controller.bytes(pcm(...new Array(1920).fill(0x2000)));
  await page.settle();
  stream.controller.drop();
  await page.settle();
  assert.equal(await look(2.9), "One two three.", "the block's arrival moved the light");
  assert.equal(await look(4.1), "Four five six!", "1.8 s of 2.72 s said is the second sentence");
  assert.equal(await look(4.5), "Seven eight?", "2.2 s of 2.72 s said is the third sentence");
});

// --- `#486 read-aloud-pieces`: a block read in pieces --------------------------------------------

/** The cases src/render.rs `read_pieces` is held to: the page cuts each the same way. */
const PIECES_FIXTURE = JSON.parse(readFileSync(join(WEB, "..", "tests", "fixtures", "read_aloud_pieces.json"), "utf8"));

test("THE PAGE CUTS A BLOCK INTO THE PIECES THE SERVER READS IT IN, case by case", () => {
  const page = newPage();
  const readPieces = page.pure("readPieces");
  for (const { name, text, pieces, alone } of PIECES_FIXTURE.cases) {
    // Through JSON: the page's arrays are of its own realm.
    assert.deepEqual(JSON.parse(JSON.stringify(readPieces(text, alone === true).map(({ start, end }) => text.slice(start, end)))), pieces, name);
  }
  // The control: a bullet typed as a dash is a sentence, not a piece, and a short block is whole.
  assert.equal(readPieces("Intro.\n- a\n- b").length, 1);
  assert.equal(readPieces("").length, 0);
});

test("`read-chunk-balance`: no block of the timeline takes most of its width, and a block's pieces stand together", () => {
  const page = newPage();
  const weights = (chars) => JSON.parse(JSON.stringify(page.pure("readTimelineWeights")(chars)));
  // The owner's read as it was: 53 words and 8, 325px and 45px of the line. By the square root it is
  // under three to one; and one block among many short ones is held to twice the mean.
  const [long, short] = weights([330, 45]);
  assert.ok(long / (long + short) < 0.75, `the long block takes ${long / (long + short)} of the line`);
  const many = weights([2000, 40, 40, 40, 40, 40]);
  const mean = [2000, 40, 40, 40, 40, 40].map(Math.sqrt).reduce((a, b) => a + b) / 6;
  assert.ok(Math.abs(many[0] - 2 * mean) < 0.001, `the long block is not held to twice the mean: ${many}`);
  assert.ok(many[0] / many.reduce((a, b) => a + b) < 0.5, "one block of six takes most of the line");
  assert.match(cssBlock(".read-timeline-block[data-continues]"), /margin-inline-start:\s*-0\.1rem;/,
    "a block's pieces do not stand closer together than two blocks");
});

/** A page reading a paragraph of typed bullets as three pieces, and a list item after it as a fourth. */
async function piecedRead() {
  const page = newPage();
  page.enableStreamedAudio();
  page.enableAbort();
  const highlights = page.enableHighlights();
  const piece = (at, chars) => ({ kind: "paragraph", spoken: true, chars, element: 0, piece: at, pieces: 3 });
  page.speechBlocks[BLOCKED_ID] = [piece(0, 6), piece(1, 8), piece(2, 11),
    { kind: "item", spoken: true, chars: 4, element: 1, piece: 0, pieces: 1 }];
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: BLOCKED_ID,
    content: "Intro:\n• one two.\n• three four.\n\n- tail",
    content_html: "<p>Intro:<br>\n• one two.<br>\n• three four.</p>\n<ul>\n<li>tail</li>\n</ul>\n" })]);
  return { page, rows, highlights };
}

test("A PARAGRAPH OF TYPED BULLETS IS READ IN PIECES: counted and stepped one by one, its block lit throughout", async () => {
  // The owner, 2026-10-23: a message whose bullets were "•" lines was one stretch on the controller.
  const { page, rows, highlights } = await piecedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  const row = () => page.el("discord-log").children[0];
  assert.equal(page.el("read-steer-count").text(), "1 of 4");
  assert.equal(blockOf(page.audioStreams[0].url), "0");
  assert.match(litBlock(row()), /^p:Intro:/);
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 4");
  assert.equal(blockOf(page.audioStreams.at(-1).url), "1");
  assert.match(litBlock(row()), /^p:Intro:/, "the paragraph went dark while its second piece was read");
  // The light inside the block is the piece being read, whole, when it is one sentence.
  const stream = page.audioStreams.at(-1);
  stream.controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(300).fill(0x2000))));
  await page.settle();
  page.audio.currentTime = page.audio.sources.at(-1).startedAt + 0.1;
  page.expireTimers(150);
  await page.settle();
  assert.equal(litSentence(highlights), "• one two.");
  await page.el("read-next").click();
  await page.settle();
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "4 of 4");
  assert.equal(litBlock(row()), "li:tail");
});

test("a tap on a block read in pieces jumps to its FIRST piece", async () => {
  const { page, rows } = await piecedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  const row = page.el("discord-log").children[0];
  const tail = everyElement(row).find((node) => String(node.tagName).toLowerCase() === "li");
  await row.dispatch("click", { target: tail });
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "4 of 4");
  const paragraph = everyElement(page.el("discord-log").children[0]).find((node) => String(node.tagName).toLowerCase() === "p");
  await page.el("discord-log").children[0].dispatch("click", { target: paragraph });
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 4", "a tap on the paragraph went to its last piece");
});

test("pieces the page cuts otherwise than the server still read in order, the block lit and no piece lit", async () => {
  // A mismatch costs the light its precision, never the read: here the server says the paragraph is two pieces.
  const page = newPage();
  page.enableStreamedAudio();
  page.enableAbort();
  const highlights = page.enableHighlights();
  page.speechBlocks[BLOCKED_ID] = [{ kind: "paragraph", spoken: true, chars: 6, element: 0, piece: 0, pieces: 2 },
    { kind: "paragraph", spoken: true, chars: 20, element: 0, piece: 1, pieces: 2 }];
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: BLOCKED_ID, content: "Intro:\n• one two.\n• three four.",
    content_html: "<p>Intro:<br>\n• one two.<br>\n• three four.</p>\n" })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "1 of 2");
  assert.match(litBlock(page.el("discord-log").children[0]), /^p:Intro:/);
  page.audioStreams[0].controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(300).fill(0x2000))));
  await page.settle();
  page.audio.currentTime = page.audio.sources.at(-1).startedAt + 0.1;
  page.expireTimers(150);
  await page.settle();
  // The block's own sentences, the first of which runs from its first words across the line break.
  const lit = highlights.get("read-aloud-sentence");
  assert.ok(lit, "nothing was lit inside the block");
  assert.equal(lit.ranges[0].start[0].textContent, "Intro:", "a piece the page could not find was lit");
});

test("a block of one sentence lights no sentence, and a browser without the highlight API looks for none", async () => {
  const { page, rows, highlights } = await sentencedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-next").click();
  await page.settle();
  page.audioStreams.at(-1).controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(200).fill(0x2000))));
  await page.settle();
  page.expireTimers(150);
  await page.settle();
  assert.equal(litSentence(highlights), "", "a one-sentence block lit a sentence as well as itself");

  // THE CONTROL: without the API, the read goes on block by block and nothing looks for sentences.
  const plain = await sentencedRead(false);
  await plain.rows[0].dispatch("click", {});
  await plain.page.settle();
  plain.page.audioStreams[0].controller.bytes(joined(wavHeader(1000, 1), pcm(...new Array(200).fill(0x2000))));
  await plain.page.settle();
  assert.equal(plain.page.expireTimers(150), 0, "the page looked for a sentence it cannot light");
  assert.equal(plain.page.el("read-steer-count").text(), "1 of 3");
});

// --- `#380 read-aloud-timeline`: the read as a line of its blocks ----------------------------------

test("THE TIMELINE shows the read's longer blocks wider, marks where the read is, and jumps", async () => {
  const { page, rows } = await blockedRead({ kinds: [{ kind: "paragraph", spoken: true, chars: 30 },
    { kind: "item", spoken: true, chars: 10 }, { kind: "item", spoken: true, chars: 20 }] });
  await rows[0].dispatch("click", {});
  await page.settle();
  const line = () => page.el("read-timeline").children;
  assert.equal(line().length, 3, "the timeline does not show the read's three blocks");
  // `read-chunk-balance`: by the square root of how long each is (`readTimelineWeights`).
  assert.deepEqual(line().map((button) => button.getAttribute("style")),
    ["--block-weight: 5.477", "--block-weight: 3.162", "--block-weight: 4.472"], "the longer blocks are not drawn wider");
  assert.deepEqual(line().map((button) => button.getAttribute("aria-current")), ["true", "false", "false"]);

  await line()[2].click();
  await page.settle();
  assert.equal(blockOf(page.audioStreams.at(-1).url), "2", "a tap on the third block did not read from it");
  assert.equal(page.el("read-steer-count").text(), "3 of 3");
  assert.deepEqual(line().map((button) => button.getAttribute("aria-current")), ["false", "false", "true"]);
  assert.deepEqual(line().map((button) => button.getAttribute("data-heard") !== null), [true, true, false],
    "the blocks before the one being read are not marked as passed");

  // Stopped by a tap on the row being read: the Read button pauses a playing read now (`#434`).
  await page.el("discord-log").children[0].dispatch("click", {});
  await page.settle();
  assert.equal(line().length, 0, "the timeline outlived the read");
});

// --- `#290 speech-ticket-refresh`: a prepared URL is never played after its ticket has ended ----------
//
// The server's tickets last `expires_in_seconds` (ten minutes) from when it minted them, and a tap that
// hands the player a URL whose ticket has gone gets a 404 and "that message could not be played", with
// no way back. So the page keeps a deadline per prepared row, counted from just before it asked, and a
// row past it is a row with nothing prepared: the tap prepares it again, once, and reads it by the
// authenticated route when that fails.

/** The prepare requests made since `from` that asked for message `id`. */
const preparedAgain = (page, from, id) => page.prepareCalls.slice(from).filter((call) => (call.ids || []).includes(id)).length;

test("`#290 speech-ticket-refresh`: a prepared URL is played until the last millisecond of its ticket, counted from before the prepare request went, and at that moment its row is prepared again and the new URL played", async () => {
  const ID = "1000000000000000001";
  const tap = async (script, late) => {
    const page = newPage(new Map(), script);
    // The server takes five seconds to answer: counted from the answer, the URL would outlive its ticket by that.
    page.prepareResponseDelayMs = 5_000;
    await signIn(page);
    const requestedAt = page.clock();
    const rows = await inReadingMode(page, [message({ id: ID })]);
    assert.equal(page.clock(), requestedAt + 5_000, "the premise: the prepare answer did not take its five seconds");
    const before = page.prepareCalls.length;
    page.setClock(requestedAt + page.speechTicketTtlSeconds * 1000 - (late ? 0 : 1));
    page.speechTicketSuffix = "-again";
    await rows[0].dispatch("click", {});
    await page.settle();
    return { again: preparedAgain(page, before, ID), url: page.players[0] && page.players[0].url, spoke: page.speakCalls.length };
  };
  const early = await tap(SCRIPT, false);
  assert.equal(early.again, 0, "a URL a millisecond short of its ticket's end was prepared again");
  assert.match(early.url, new RegExp(`^/api/v1/speech/ticket-for-${ID}\\?selection=tap-`), "the prepared URL was not played");
  const at = await tap(SCRIPT, true);
  assert.equal(at.again, 1, `the tap at its ticket's end prepared its row ${at.again} times, not once`);
  assert.match(at.url, new RegExp(`^/api/v1/speech/ticket-for-${ID}-again\\?selection=tap-`), `the URL whose ticket had ended was played: ${at.url}`);
  assert.equal(at.spoke, 0, "a refresh that worked fell back to the slower route");
  // The control: the deadline counted from the answer. Five seconds later than the ticket's, the dead URL is played.
  const fromAnswer = await tap(brokenScript("    const until = { mono: requestedAt.mono + lifetimeMs, wall: requestedAt.wall + lifetimeMs };",
    "    const answeredAt = speechDeadlineNow();\n    const until = { mono: answeredAt.mono + lifetimeMs, wall: answeredAt.wall + lifetimeMs };"), true);
  assert.match(fromAnswer.url, new RegExp(`ticket-for-${ID}\\?selection=`),
    "the negative control, counting from the answer, did not play the dead URL, so the check above proves nothing");
});

test("`#290 speech-ticket-refresh`: a phone that slept through the ticket's end prepares the row again, though the monotonic clock stood still while it slept (negative control: the deadline kept on that clock alone)", async () => {
  const ID = "1000000000000000001";
  const tap = async (script) => {
    const page = newPage(new Map(), script);
    await signIn(page);
    const rows = await inReadingMode(page, [message({ id: ID })]);
    const before = page.prepareCalls.length;
    // A minute of reading, then the phone is locked for a quarter of an hour, and unlocked.
    page.setClock(page.clock() + 60_000);
    page.sleep(15 * 60_000);
    page.speechTicketSuffix = "-again";
    await rows[0].dispatch("click", {});
    await page.settle();
    return { page, again: preparedAgain(page, before, ID), url: page.players[0] && page.players[0].url };
  };
  const slept = await tap(SCRIPT);
  assert.equal(slept.again, 1, `the tap after the phone slept prepared its row ${slept.again} times, not once`);
  assert.match(slept.url, new RegExp(`^/api/v1/speech/ticket-for-${ID}-again\\?selection=tap-`),
    `the URL whose ticket ended while the phone slept was played: ${slept.url}`);
  assert.deepEqual(slept.page.speakCalls, [], "a refresh that worked fell back to the slower route");
  const control = await tap(await brokenModule([{ file: "src/core/speech/prepared.ts",
    find: "now.mono < entry.until.mono && now.wall < entry.until.wall", replace: "now.mono < entry.until.mono" }]));
  assert.match(control.url, new RegExp(`ticket-for-${ID}\\?selection=`),
    "the negative control, on the monotonic clock alone, did not play the dead URL, so the check above proves nothing");
});

test("`#290 speech-ticket-refresh`: a tap after the ticket has ended, when preparing the row again fails, reads it by the authenticated route and never plays the dead URL", async () => {
  const ID = "1000000000000000001";
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: ID })]);
  const before = page.prepareCalls.length;
  page.prepareFails = true;
  page.setClock(page.clock() + page.speechTicketTtlSeconds * 1000 + 60_000);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.ok(preparedAgain(page, before, ID) >= 1, "the tap did not try to prepare its row again");
  assert.deepEqual(page.speakCalls, [ID], "a failed refresh did not fall back to the authenticated route");
  assert.equal(page.players.length, 1, "more than one player was built");
  assert.doesNotMatch(page.players[0].url, /ticket-for/, `the URL whose ticket had ended was played: ${page.players[0].url}`);
  assert.equal(page.players[0].playCount, 1, "the audio fetched by the authenticated route was not played");
});

test("`#290 speech-ticket-refresh`: each row's URL keeps its own ticket's deadline — a row prepared by a later tap is still played as prepared when one prepared earlier has to be prepared again — and the screen's own preparation renews the rows whose tickets have ended", async () => {
  const [FIRST, LATER, THIRD] = ["1000000000000000001", "1000000000000000002", "1000000000000000003"];
  const said = (id, author, content, hour) => message({ id, author_id: author, content, timestamp: `2026-08-19T0${hour}:31:00.000Z` });
  const page = newPage();
  const stream = await withLiveChannel(page, [said(FIRST, "1000000000000000011", "first", 4)]);
  const rowOf = (id) => page.el("discord-log").children.find((li) => String(li.getAttribute("data-ids")).split(" ").includes(id));
  const tap = async (id) => {
    await rowOf(id).dispatch("click", {});
    await page.settle();
    const player = page.players.at(-1);
    await player.end();
    await page.settle();
    return player.url;
  };
  const start = page.clock();
  await readButton(page).click();
  await page.settle();
  assert.deepEqual(page.prepareCalls.map((call) => call.ids), [[FIRST]], "the premise: turning Read on did not prepare the row");
  // Five minutes on, a message arrives, and a tap on it prepares it alone.
  page.setClock(start + 300_000);
  await deliver(page, stream, sseMessage(said(LATER, "1000000000000000012", "later", 5)));
  let before = page.prepareCalls.length;
  assert.match(await tap(LATER), new RegExp(`ticket-for-${LATER}\\?`), "the premise: the later row was not read as prepared");
  assert.deepEqual(page.prepareCalls.slice(before).map((call) => call.ids), [[LATER]], "the premise: the tap did not prepare its row alone");
  // Ten minutes from the first: its ticket has ended, and the later row's has five minutes left.
  page.setClock(start + page.speechTicketTtlSeconds * 1000);
  page.speechTicketSuffix = "-again";
  before = page.prepareCalls.length;
  const later = await tap(LATER);
  assert.equal(preparedAgain(page, before, LATER), 0, "the later row, its ticket alive, was prepared again");
  assert.match(later, new RegExp(`^/api/v1/speech/ticket-for-${LATER}\\?selection=`), `the later row's prepared URL was not played: ${later}`);
  const first = await tap(FIRST);
  assert.equal(preparedAgain(page, before, FIRST), 1, "the first row, its ticket ended, was not prepared again exactly once");
  assert.match(first, new RegExp(`^/api/v1/speech/ticket-for-${FIRST}-again\\?selection=`), `the first row's dead URL was played: ${first}`);
  assert.deepEqual(page.speakCalls, [], "a row fell back to the slower route");
  // Twenty minutes from the first: the later row's ticket ended at fifteen, and the first's renewed one at
  // twenty. A tap on a new row prepares it, and the screen's preparation behind it asks for both again.
  page.setClock(start + 2 * page.speechTicketTtlSeconds * 1000);
  await deliver(page, stream, sseMessage(said(THIRD, "1000000000000000013", "third", 6)));
  before = page.prepareCalls.length;
  await tap(THIRD);
  const asked = page.prepareCalls.slice(before).flatMap((call) => call.ids);
  assert.ok(asked.includes(FIRST) && asked.includes(LATER),
    `the screen's preparation did not renew the rows whose tickets had ended: ${JSON.stringify(page.prepareCalls.slice(before))}`);
});

test("with reading mode OFF a tap folds the message and speaks nothing", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1000000000000000001", content: "z".repeat(4000) }),
  ]);

  await rows[0].dispatch("click", {});
  await page.settle();

  assert.deepEqual(readAloudMessages(page), [], "a tap outside reading mode spent a vendor call");
  assert.equal(rows[0].getAttribute("data-collapsed"), "false", "the tap did not fold instead");
});

test("leaving the channel view stops the reading rather than playing over the transcript", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  await rows[0].dispatch("click", {});
  await page.settle();

  await page.el("view-switch").click();
  await page.settle();

  assert.equal(page.players[0].paused, true, "audio kept playing after leaving the channel");
  assert.deepEqual(page.dismissCalls, [], "leaving the view archived the message");
});

test("THE BRIDGE IS RECOGNISED FROM THE FIRST RENDER, WITHOUT THE READER HAVING REPLIED", async () => {
  // The owner's report: every message in the channel was the same colour, including his own.
  //
  // The page learned which account this bridge posts as only as a side effect of a reply sent
  // from the app, or of the live feed delivering a `self_posted` message. Until one of those
  // happened `selfAuthorId` was null, so nothing was `me` — AND the bridge still counted as a
  // second bot, which turned "the only bot that is not us" into a coin toss the page declines to
  // call. Every row fell through to the third-party colour. The server knows the answer from its
  // own token and now says so.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    // The bridge, posting the owner's words on his behalf.
    message({ id: "1000000000000000001", author: "vibe-talk", author_id: "1000000000000000009", author_is_bot: true }),
    // The coding agent: the only OTHER bot in the channel.
    message({ id: "1000000000000000002", author: "MyDiscordBot", author_id: "20", author_is_bot: true }),
    // A person who is neither.
    message({ id: "1000000000000000003", author: "alice", author_id: "30", author_is_bot: false }),
  ]);

  assert.equal(whoOf(page, 0), "me", "the bridge's own messages are not recognised as the owner's");
  assert.equal(whoOf(page, 1), "coder", "the lone other bot was not taken to be the coding agent");
  assert.equal(whoOf(page, 2), "human", "a person was not drawn as one");

  // The three buckets are three DIFFERENT treatments, which is the whole complaint: they were one.
  const mine = cssBlock('#discord-log li.discord-message[data-who="me"]');
  const theirs = cssBlock('#discord-log li.discord-message[data-who="coder"]');
  assert.match(mine, /--mine/, "the owner's messages do not use the transcript's own colour");
  assert.match(theirs, /--theirs/, "the coding agent is not drawn as the transcript's other speaker");
});

test("without the server's answer the page still recognises nobody — which is why it is sent", async () => {
  // The control for the test above. An older server sends no `self_author_id`, and the page has to
  // degrade to the behaviour that had the bug rather than inventing an id: labelling somebody
  // else's messages as the owner's would be worse than labelling nobody's.
  const page = newPage();
  page.selfAuthorId = null;
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", author: "vibe-talk", author_id: "1000000000000000009", author_is_bot: true }),
    message({ id: "1000000000000000002", author: "MyDiscordBot", author_id: "20", author_is_bot: true }),
  ]);

  assert.notEqual(whoOf(page, 0), "me", "an id nobody supplied was invented");
  assert.equal(whoOf(page, 1), "bot", "with two unidentified bots the coder guess must decline");
});

test("THE CODING AGENT'S MESSAGES RUN EDGE TO EDGE; THE OWNER'S KEEP THEIR INDENT", async () => {
  // The agent's messages are the thing this view exists to read, and they are long. The owner's
  // stay indented and coloured, so the two are still told apart by two signals either of which
  // would do alone.
  const coder = cssBlock('#discord-log li.discord-message[data-who="coder"]');
  assert.match(coder, /margin-left:\s*calc\(-/, "the agent's messages do not reclaim the gutter");
  assert.match(coder, /border-radius:\s*0/, "a full-width box with rounded corners is a card");
  assert.doesNotMatch(coder, /--speaker-inset/, "the agent's messages are still inset");

  const mine = cssBlock('#discord-log li.discord-message[data-who="me"]');
  assert.match(mine, /--speaker-inset/, "the owner's messages lost their indent");
  assert.match(mine, /--mine/, "the owner's messages lost their colour");
});

/**
 * Every rule, in every media block, that draws a channel row's own box — the row in any state, and
 * not a pseudo-element or anything inside it: its selectors and its body.
 */
const CHANNEL_ROW_RULES = [...CSS_CODE.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
  .map((rule) => ({ selectors: rule[1].split(",").map((part) => part.trim()), body: rule[2] }))
  .filter(({ selectors }) => selectors.some((selector) =>
    /^#(discord|pinned)-log li\.discord-message(\[[^\]]*\]|:[a-z-]+(\([^)]*\))?)*$/.test(selector)));

test("...AND IT IS A RECTANGLE: FOUR SIDES AND SQUARE CORNERS, ON A PHONE AND ON A DESK", () => {
  // `#213 row-side-borders`. The owner, from a desktop browser: the agent's square boxes had borders
  // on the top and bottom and NOT on the left and right, which looked weird — and on a desk, where
  // the tile is held inside the reading column, there was no screen edge for the missing sides to
  // hide against. The tile takes the hairline every row has on all four sides, and keeps its square
  // corners; on a phone it still runs edge to edge, with its two sides at the screen's edges.
  //
  // The page suite lays nothing out. Chromium measures a non-zero left and right border on the
  // agent's tile at 412 and 1280, and no horizontal scroll at 360 (tests/offline_cache_browser.py).
  // What is checked here is that the hairline is still declared for every row, and that no rule for
  // a channel row, in any media block and in any state, takes a side away again.
  assert.match(cssRules(BASE_CSS, ".messages li").join("\n"), /(?<![-\w])border:\s*1px solid var\(--edge\)/,
    "the hairline every row is drawn with is gone from web/style.css");
  assert.ok(CHANNEL_ROW_RULES.length > 20, "the channel row's rules were not found, so nothing was checked");
  for (const { selectors, body } of CHANNEL_ROW_RULES) {
    const taken = takesASide(body);
    assert.equal(taken, null, `"${selectors.join(", ")}" takes a side off the row: ${taken}`);
  }
  for (const list of ["#discord-log", "#pinned-log"]) {
    const coder = cssBlock(`${list} li.discord-message[data-who="coder"]`);
    assert.match(coder, /border-radius:\s*0/, `${list}: the agent's tile has rounded corners again`);
    assert.doesNotMatch(coder, /(?<![-\w])border(-(left|right))?(-color)?:/,
      `${list}: the agent's tile draws its sides differently from the rest of its border`);
  }
  // ...and on a desk too, where the tile is inside the column rather than across the screen.
  assert.doesNotMatch(
    cssBlockIn("(min-width: 900px) and (pointer: fine)", '#discord-log li.discord-message[data-who="coder"]'),
    /border/, "the desktop block draws the agent's tile's border differently from a phone's");

  // The control: the check above goes red for the rule this replaced.
  assert.equal(takesASide("background: var(--theirs); border-left: none; border-right: none;"), "border-left: none");
  assert.equal(takesASide("border-width: 1px 0;"), "border-width: 1px 0");
  assert.equal(takesASide("border-inline-style: hidden;"), "border-inline-style: hidden");
  assert.equal(takesASide("border: 1px solid var(--edge); border-left: 3px solid var(--summary-tint);"), null);
});

test("MY OWN MESSAGES ARE READ ALREADY, BY DEFAULT, WITHOUT VISITING SETTINGS", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("mark-own-read").checked, true, "the default is not on");

  await showDiscord(page, [
    message({ id: "1000000000000000001", ...OWN }),
    message({ id: "1000000000000000002", ...CODER }),
  ]);

  assert.equal(whoOf(page, 0), "me");
  // `#549 own-read-stored`: read in the one read bit the server keeps, which it set at the message's
  // first sight, so the row is drawn from that bit as any row is — archived, with Unarchive.
  assert.equal(rowState(page, 0).archived, "true", "the owner's own message is not marked read");
  assert.equal(rowState(page, 1).archived, "false", "the agent's message was marked read too");
  assert.equal(doneButton(row(page, 0)).textContent, "Unarchive");
  // The server decided it; the page wrote nothing.
  assert.deepEqual(page.dismissCalls, [], "the page wrote a dismissal for a message nobody archived");
});

test("...and they are kept out of the queue, and turning the setting off leaves them where their read state put them", async () => {
  const page = newPage();
  await signIn(page);
  page.messages = [
    message({ id: "1000000000000000001", ...OWN }),
    message({ id: "1000000000000000002", ...CODER }),
  ];
  await showDiscord(page, page.messages);
  await turnTodoOn(page);

  assert.equal(
    page.el("discord-log").children.length,
    1,
    "the queue still holds the reader's own words"
  );
  assert.equal(row(page, 0).getAttribute("data-id"), "1000000000000000002");

  // `#549 own-read-stored`: the setting decides where a message STARTS. Turned off, what was already
  // seen keeps its read state, and nothing is read again for it.
  const reads = page.todoReads;
  await page.el("mark-own-read").setChecked(false);
  await page.settle();
  assert.equal(page.el("discord-log").children.length, 1, "turning the setting off brought a read message back");
  assert.equal(page.todoReads, reads, "turning the setting off read the queue again");
});

test("`#549 own-read-stored`: another device turning own messages read off or on moves the switch here, and no row", async () => {
  // The setting decides where a message's read state starts, at its first sight on the server; the
  // rows on screen are drawn from that state, which the change does not touch.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", ...OWN }),
    message({ id: "1000000000000000002", ...CODER }),
  ]);
  assert.equal(rowState(page, 0).archived, "true", "the premise: the owner's message is read");
  page.pure("deviceStorage()").setItem("vibe-talk.voice.mark-own-read", "0");
  page.pure("applySyncedSetting")("vibe-talk.voice.mark-own-read");
  await page.settle();
  assert.equal(page.el("mark-own-read").checked, false, "the switch does not say the synced choice");
  assert.equal(rowState(page, 0).archived, "true", "another device's off put a read message back");
  page.pure("deviceStorage()").setItem("vibe-talk.voice.mark-own-read", "1");
  page.pure("applySyncedSetting")("vibe-talk.voice.mark-own-read");
  await page.settle();
  assert.equal(page.el("mark-own-read").checked, true);
  assert.equal(rowState(page, 1).archived, "false", "another device's on marked the agent's message read");
});

test("`#549 own-read-stored`: a row of the reader's own words offers Unarchive, and e puts it back like any other", async () => {
  // The owner, 2026-10-26: "record the state that they are read and then I can toggle one individually
  // and it's perfectly valid."
  const page = await keyboardPageAtTop([
    message({ id: "1", ...OWN }),
    message({ id: "2", ...CODER }),
    message({ id: "3", ...CODER }),
  ]);
  const button = (id) => doneButton(channelRow(page, id));
  assert.equal(button("1").disabled, false, "the owner's own row withholds its button");
  assert.equal(button("1").textContent, "Unarchive", "the owner's own row is not drawn read");
  assert.equal(button("2").textContent, "Done");
  await press(page, page.el("scroll-area"), "ArrowDown");
  assert.equal(selectedId(page), "1");
  await pressHere(page, "e");
  assert.deepStrictEqual(page.restoreCalls.map((call) => call.messages), [["1"]], "e did not put his message back");
  assert.deepStrictEqual(page.dismissCalls, [], "e wrote a mark for a message already read");
  assert.equal(button("1").textContent, "Done", "his message put back is not drawn unread");
  // ...and it stays unread through the next read: nothing marks it again.
  await reReadChannel(page);
  assert.equal(button("1").textContent, "Done", "a read marked his message read again");
});

test("`#549 own-read-stored`: Apply to existing messages asks twice, marks his undone messages read, and Undo puts back exactly those", async () => {
  const page = newPage();
  page.storage.set("vibe-talk.voice.mark-own-read", "0");
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", ...OWN }),
    message({ id: "1000000000000000002", ...CODER }),
    message({ id: "1000000000000000003", ...OWN }),
  ]);
  // First seen while the setting was off: his messages started unread.
  assert.deepStrictEqual([0, 1, 2].map((i) => rowState(page, i).archived), ["false", "false", "false"]);
  await page.el("mark-own-read").setChecked(true);
  await page.settle();
  assert.deepStrictEqual([0, 1, 2].map((i) => rowState(page, i).archived), ["false", "false", "false"],
    "turning the setting on reached back to messages already seen");
  // The first press says what the second does, and writes nothing.
  await page.el("own-read-apply").click();
  await page.settle();
  assert.equal(page.ownReadApplies, 0, "the first press applied it");
  assert.match(page.el("own-read-state").textContent, /every message of yours/);
  assert.equal(page.el("own-read-apply").textContent, "Mark them read");
  await page.el("own-read-apply").click();
  await page.settle();
  assert.equal(page.ownReadApplies, 1);
  assert.equal(page.el("own-read-state").textContent, "Marked 2 of your messages read. Undo puts them back.");
  assert.deepStrictEqual([0, 1, 2].map((i) => rowState(page, i).archived), ["true", "false", "true"],
    "the channel was not drawn from what Apply marked");
  assert.equal(page.el("own-read-undo").hidden, false, "no Undo offered");
  await page.el("own-read-undo").click();
  await page.settle();
  assert.deepStrictEqual(page.restoreCalls.at(-1), { messages: ["1000000000000000001", "1000000000000000003"] });
  assert.equal(page.el("own-read-undo").hidden, true);
  assert.deepStrictEqual([0, 1, 2].map((i) => rowState(page, i).archived), ["false", "false", "false"],
    "Undo did not put his messages back");
  // A first press left alone is forgotten: the next press asks again.
  await page.el("own-read-apply").click();
  assert.equal(page.el("own-read-apply").textContent, "Mark them read");
  page.expireTimers(sourceConstant("OWN_READ_APPLY_ARM_MS"));
  await page.settle();
  assert.equal(page.el("own-read-apply").textContent, "Apply to existing messages");
});

test("`#549 own-read-stored`: a message of his arriving live, or sent from here, is drawn read at once, and the server's next answer stands", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, [message({ id: "100", content: "already here", ...CODER })]);
  await deliver(page, stream, sseMessage(message({ id: "200", content: "typed in the chat app", ...OWN })));
  const arrived = rowWithId(page, "200");
  assert.equal(arrived.getAttribute("data-archived"), "true", "his own message arrived drawn unread");
  await deliver(page, stream, sseMessage(message({ id: "201", content: "the agent's", ...CODER })));
  assert.equal(rowWithId(page, "201").getAttribute("data-archived"), "false", "the agent's message arrived drawn read");
  // The server decided otherwise — he had turned the setting off on another device — and its next
  // answer is the one that stands.
  page.messages = [message({ id: "100", content: "already here", ...CODER }),
    message({ id: "200", content: "typed in the chat app", ...OWN })];
  page.ownDecided.add("200");
  await reReadChannel(page);
  assert.equal(rowWithId(page, "200").getAttribute("data-archived"), "false", "the server's answer did not stand");
  // Sent from here: drawn read as the post is acknowledged, before any read.
  const posted = message({ id: "300", content: "sent from the page", ...OWN });
  page.replyResponse = async () => json(200, { posted });
  await page.el("channel-compose-text").setValue(posted.content);
  await page.el("channel-send").click();
  await page.settle();
  assert.ok(rowWithId(page, "300"), "the sent message was not drawn");
  assert.equal(rowWithId(page, "300").getAttribute("data-archived"), "true", "what he just sent was drawn unread");
  assert.ok(page.ownDecided.has("300"), "the server did not decide what it posted");
});

test("`#536 mark-own-messages-read`: an account only the server's configuration names is drawn as the reader's, and read", async () => {
  // `[owner] user_ids` reached the server's own rules but not the page: such an account was a person here.
  const page = newPage();
  page.ownerAuthorIds = ["U0OLDACCOUNT"];
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", author: "Ada", author_id: "U0OLDACCOUNT", author_is_bot: false }),
    message({ id: "1000000000000000002", ...CODER }),
  ]);
  assert.equal(whoOf(page, 0), "me", "the configured account is not drawn as the reader's");
  assert.equal(rowState(page, 0).archived, "true", "the configured account's message is not read");
  assert.equal(rowState(page, 1).archived, "false");
});

test("`#536 mark-own-messages-read`: in a thread mixing the reader's messages and the agent's, Main's unread count is exactly the rows still offering Done", async () => {
  // The owner, 2026-10-26: "lots of my messages would seem to be unread based on the status of their
  // icon in the upper-right corner, but the thread card doesn't list any unread." One rule for both.
  const page = newPage();
  const messages = runThread(page, { read: 4, tail: [["own", false], ["agent", false], ["own", false]] });
  await signIn(page);
  await showDiscord(page, messages);
  await pickThread(page, "flat");
  await pickThread(page, "main");
  await page.settle();
  const chip = unreadChip(page, runId(0));
  await pickThread(page, `thread:${RUN_THREAD}`);
  await page.settle();
  const offering = page.el("discord-log").children.filter((li) => {
    const done = doneButton(li);
    return li.getAttribute("data-id") !== runId(0) && li.getAttribute("data-archived") !== "true" &&
      done && !done.disabled;
  }).map((li) => li.getAttribute("data-id"));
  // The agent's answer and the agent's message after it; none of the reader's own.
  assert.deepStrictEqual(offering, [runId(500), runId(502)], "a row of the reader's own still offers Done");
  assert.equal(chip && chip.textContent, `${offering.length} unread`,
    "Main's count of the thread's unread replies disagrees with the rows that offer Done");
});

test("ONE VOICE AT A TIME: tapping twice never leaves two readings talking over each other", async () => {
  // The owner tapped a message twice and heard two copies read over each other. `readAloud` stops
  // whatever is playing and then AWAITS the audio, so a second tap arriving during that await
  // found nothing playing to stop; both fetches built a player, `nowPlaying` kept only the second,
  // and the first was audible with nothing able to pause it.
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  // Both taps land BEFORE either fetch resolves — the race, reproduced rather than described.
  const first = rows[0].dispatch("click", {});
  const second = rows[0].dispatch("click", {});
  await first;
  await second;
  await page.settle();

  const audible = page.players.filter((p) => p.playCount > 0 && !p.paused);
  assert.ok(audible.length <= 1, `${audible.length} readings are audible at once`);
});

test("...and a tap on ANOTHER message never leaves the first one playing", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", content: "one" }),
    message({ id: "1000000000000000002", content: "two" }),
  ]);

  const a = rows[0].dispatch("click", {});
  const b = rows[1].dispatch("click", {});
  await a;
  await b;
  await page.settle();

  const audible = page.players.filter((p) => p.playCount > 0 && !p.paused);
  assert.ok(audible.length <= 1, `${audible.length} readings are audible at once`);
  assert.equal(rowState(page, 0).reading, "false", "the first message is still marked as reading");
});

test("the Read control becomes a way OUT once a session is on", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);

  assert.equal(page.el("read-aloud-label").text(), "Read");
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "false");

  await page.el("read-aloud").click();
  await page.settle();
  // The ACT, not the state: "Reading" left the reader guessing what pressing it would do.
  assert.equal(page.el("read-aloud-label").text(), "Stop", "the control does not say how to leave");
  assert.equal(page.el("read-aloud").getAttribute("data-active"), "true");
  assert.match(
    cssBlock('#control-pane .control-read[data-active="true"]'),
    /--warn/,
    "an active session looks exactly like an inactive one"
  );
});

// --- `#567 pace-stepper`: [-] [100%] [+] in place of the slider ------------------------------------

const paceKey = (node, key) => node.dispatch("keydown", { key, preventDefault() {} });

test("`#567 pace-stepper`: the pace is a box between − and +, ten a step, within 50 to 200, typed or arrowed", async () => {
  const popover = HTML_CODE.slice(HTML_CODE.indexOf('id="speed-popover"'), HTML_CODE.indexOf('id="read-aloud-stop"'));
  assert.ok(popover.includes('id="read-speed-box"'), "the pace popover has no box");
  assert.doesNotMatch(popover, /type="range"/, "the pace slider is still in the popover");
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await page.el("read-speed").click();
  await page.settle();
  const box = page.el("read-speed-box");
  assert.equal(box.value, "100%", "the box does not start at 100%");
  assert.equal(page.storage.has("vibe-talk.voice.read-speed"), false, "opening it chose a pace");
  // Tapped into and out of, or Enter on what it shows: nothing is chosen, the voice keeps its own pace.
  await box.dispatch("blur");
  await paceKey(box, "Enter");
  assert.equal(page.storage.has("vibe-talk.voice.read-speed"), false, "leaving the box untouched chose 100%");
  assert.equal(page.el("read-speed-label").text(), "Pace");
  for (let press = 0; press < 3; press += 1) await page.el("read-speed-up").click();
  assert.equal(box.value, "130%");
  assert.equal(page.el("read-speed-label").text(), "130%");
  assert.equal(page.storage.get("vibe-talk.voice.read-speed"), "130", "the pace was not kept where it is synced");
  await page.el("read-speed-down").click();
  assert.equal(box.value, "120%");
  // Typed, with or without the sign, taken on Enter or when the box is left.
  box.value = "95";
  await paceKey(box, "Enter");
  assert.equal(box.value, "95%");
  await page.el("read-speed-up").click();
  assert.equal(box.value, "100%", "+ from 95 did not go to the next step of ten");
  box.value = " 72 % ";
  await box.dispatch("blur");
  assert.equal(box.value, "72%");
  await page.el("read-speed-down").click();
  assert.equal(box.value, "70%", "− from 72 did not go to the step of ten below");
  box.value = "fast";
  await box.dispatch("change");
  assert.equal(box.value, "70%", "a word in the box was taken for a pace");
  // Clamped to what the voices take, and the end's button says so.
  box.value = "1000";
  await box.dispatch("change");
  assert.equal(box.value, "200%", "a four-figure pace was not clamped");
  box.value = "190";
  await box.dispatch("change");
  box.value = "250%";
  await box.dispatch("change");
  assert.equal(box.value, "200%");
  assert.equal(page.el("read-speed-up").disabled, true, "+ is offered past the fastest pace");
  await page.el("read-speed-up").click();
  assert.equal(box.value, "200%");
  // The arrows step it from the box.
  await paceKey(box, "ArrowDown");
  assert.equal(box.value, "190%");
  assert.equal(page.el("read-speed-up").disabled, false);
  box.value = "40";
  await paceKey(box, "Enter");
  assert.equal(box.value, "50%");
  assert.equal(page.el("read-speed-down").disabled, true, "− is offered past the slowest pace");
  await paceKey(box, "ArrowUp");
  assert.equal(box.value, "60%");
  assert.equal(page.storage.get("vibe-talk.voice.read-speed"), "60");
});

test("`#567 pace-stepper`: the keyboard the pace box raises leaves its popover and the bar open, and a disabled end gives the focus to the box", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  await page.el("read-speed").click();
  await page.settle();
  await focusOn(page, page.el("read-speed-box"));
  await page.resizeViewport({ visual: 480 });
  assert.equal(page.el("speed-popover").hidden, false, "the keyboard for the pace box closed its popover");
  assert.notEqual(page.documentElement.getAttribute("data-keyboard"), "open",
    "the bars went for a keyboard raised by the pace box");
  // The message box, with the keyboard still up: the page lays out for it as ever.
  await focusOn(page, page.el("channel-compose-text"));
  await page.resizeViewport({ visual: 470 });
  assert.equal(page.documentElement.getAttribute("data-keyboard"), "open");
  assert.equal(page.el("speed-popover").hidden, true);
  // + pressed to the fastest pace, the focus on it: it goes to the box, not the page.
  await page.resizeViewport({ visual: 900 });
  await page.el("read-speed").click();
  await page.settle();
  page.el("read-speed-box").value = "190";
  await page.el("read-speed-box").dispatch("change");
  await focusOn(page, page.el("read-speed-up"));
  await page.el("read-speed-up").click();
  assert.equal(page.el("read-speed-up").disabled, true);
  assert.equal(page.document.activeElement, page.el("read-speed-box"), "the focus fell off the disabled +");
});

test("`#567 pace-stepper`: a pace changed mid-read is taken up at once by the device voice, from the sentence being read", async () => {
  const page = devicePage();
  await signIn(page);
  const sentences = ["The first sentence of the message is here.", "The second sentence of it comes after."];
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001", content: sentences.join(" ") })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.deviceSpeech.utterances[0].rate, 1);
  await page.el("read-speed-up").click();
  await page.el("read-speed-up").click();
  // The pace rests, and only then is the sentence said again: once, at the pace it came to.
  assert.equal(page.deviceSpeech.utterances.length, 1, "each press restarted the sentence");
  page.expireTimers(sourceConstant("READ_SPEED_SETTLE_MS"));
  await page.settle();
  assert.equal(page.deviceSpeech.cancellations, 1, "the sentence was not stopped for the new pace");
  // A beat after the cancel, the sentence again.
  page.expireTimers(50);
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, 2);
  assert.equal(page.deviceSpeech.utterances[1].text, sentences[0], "the read did not go on from the sentence being read");
  assert.equal(page.deviceSpeech.utterances[1].rate, 1.2);
  page.deviceSpeech.utterances[0].end();
  await page.settle();
  assert.equal(page.deviceSpeech.utterances.length, 2, "the sentence cut off for the new pace ended the read");
});

test("`#567 pace-stepper`: a pace changed mid-read is taken up at once by the cloud voice, the block being read again at the new pace", async () => {
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001", content: "Read this at a new pace." })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  const streams = page.audioStreams.length;
  const prepares = page.prepareCalls.length;
  await page.el("read-speed-up").click();
  page.expireTimers(sourceConstant("READ_SPEED_SETTLE_MS"));
  await page.settle();
  assert.ok(page.prepareCalls.slice(prepares).some((asked) => asked.speed === 1.1),
    `nothing was prepared at the new pace: ${JSON.stringify(page.prepareCalls.slice(prepares))}`);
  assert.equal(page.audioStreams.length, streams + 1, "the block was not read again");
});

test("A TAP SHOWS IMMEDIATELY, before anything has been fetched", async () => {
  // The complaint: tap a message, watch nothing happen for seconds. Everything after the tap is
  // remote — this server, then ElevenLabs, then synthesising the whole message — and until this
  // the row changed only when the audio arrived. #95 read-aloud-responsiveness.
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  // Deliberately NOT awaited: this is the state of the world mid-flight, which is the whole point.
  //
  // WHICH MARK depends on which path the tap took, and BOTH are immediate — that is the claim, not
  // the particular attribute. On a PREPARED row there is no flight at all: the URL is already in
  // hand, so the row goes straight to reading rather than pausing at pending. On the fallback path
  // the audio still has to be fetched, and pending is what covers that wait. A test that demanded
  // `pending` specifically would now be asserting that the fast path is slow.
  const inFlight = rows[0].dispatch("click", {});
  const marked = rowState(page, 0);
  assert.ok(
    marked.pending === "true" || marked.reading === "true",
    `the tap left no mark until the audio arrived: ${JSON.stringify(marked)}`
  );
  // The CONTROL is not asserted here. It says "working" only while a read is genuinely in flight,
  // and a prepared tap has no flight — it is already playing, and the control reads "ready" during
  // playback exactly as it always has. The fallback path's flight is covered by the test below.

  await inFlight;
  await page.settle();
  // ...and once it is really speaking, it is no longer merely promised.
  assert.equal(rowState(page, 0).pending, "false", "the row is still pending while it plays");
  assert.equal(rowState(page, 0).reading, "true");
  assert.equal(page.el("read-aloud").getAttribute("data-read-state"), "ready");
});

test("a read that fails says so on the control rather than only in a toast", async () => {
  const page = newPage();
  // THE FALLBACK PATH ON PURPOSE. What is under test is a FETCH failing, and a prepared row does
  // not fetch — it is handed a URL and the failure, if there is one, arrives on the audio element
  // instead. Preparing is disabled so the wait and the refusal this covers still exist.
  page.prepareFails = true;
  await signIn(page);
  page.speakStatus = 503;
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  await rows[0].dispatch("click", {});
  await page.settle();

  assert.equal(page.el("read-aloud").getAttribute("data-read-state"), "failed");
  assert.equal(rowState(page, 0).pending, "false", "the row is stuck looking like it is loading");
  // Leaving the mode clears it: last session's failure must not greet the next one.
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud").getAttribute("data-read-state"), "idle");
});

test("THE SUMMARY NOTE REPORTS WHAT SUMMARISING ACTUALLY COST", async () => {
  // The owner asked for this as an experiment: a round trip to the ElevenLabs agent should beat a
  // full-size model with a harness, and the way to find out is to measure it. The server times
  // each generation and reports `generated_in_ms`; before this the number existed and nothing
  // showed it.
  const page = newPage();
  await signIn(page);
  page.summaryMs = 1200;
  await showDiscord(page, [message({ id: "1000000000000000001", content: "q".repeat(4000) })]);
  await turnSummariesOn(page);
  await page.settle();

  const note = page.el("summary-note").text();
  assert.match(note, /1\.2s/, "the measured round trip is not reported anywhere");
  assert.match(note, /1 measured/, "a median over one sample must say it is over one sample");
});

test("...and a cache hit is not averaged in as instant", async () => {
  // A cache hit and a below-threshold answer both report no time, and absent is NOT zero: neither
  // asked the vendor anything, so neither is evidence about how fast the vendor is. Folding them
  // in as zeroes would report the backend as faster than it is — the wrong direction for a number
  // meant to inform a choice.
  const page = newPage();
  await signIn(page);
  page.summaryMs = null;
  await showDiscord(page, [message({ id: "1000000000000000001", content: "q".repeat(4000) })]);
  await turnSummariesOn(page);
  await page.settle();

  const note = page.el("summary-note").text();
  assert.doesNotMatch(note, /0\.0s/, "an unmeasured summary was averaged in as instant");
  assert.doesNotMatch(note, /measured/, "it claims a measurement it never made");
});

test("THE OWNER'S OWN DISCORD MESSAGES ARE HIS, NOT A STRANGER'S", async () => {
  // Two accounts carry the owner's words: the one this bridge posts as, and the one he types into
  // Discord with himself. The first is free — it is read out of the bot token. The second CANNOT
  // be derived: a bot's account has no relationship to the human reading the channel. So messages
  // he had typed himself came through as a third party, brown and centred, beside his own dictated
  // ones drawn as his.
  const page = newPage();
  page.ownerAuthorId = "777000000000000777";
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1", author: "vibe-talk", author_id: "1000000000000000009", author_is_bot: true }),
    message({ id: "2", author: "the owner", author_id: "777000000000000777", author_is_bot: false }),
    message({ id: "3", author: "alice", author_id: "30", author_is_bot: false }),
  ]);

  assert.equal(whoOf(page, 0), "me", "the bridge's own posts stopped being the owner's");
  assert.equal(whoOf(page, 1), "me", "the owner's own Discord account is drawn as a stranger");
  assert.equal(whoOf(page, 2), "human", "everybody became the owner");
});

test("...and without being told, it says nobody rather than guessing", async () => {
  // There is no derivation to fall back on, and inventing one would paint somebody else's messages
  // as the owner's. Settings can still assign it per account without a restart.
  const page = newPage();
  page.ownerAuthorId = null;
  await signIn(page);
  await showDiscord(page, [
    message({ id: "2", author: "the owner", author_id: "777000000000000777", author_is_bot: false }),
  ]);
  assert.equal(whoOf(page, 0), "human", "an account nobody named was claimed as the owner's");
});

test("CLEAR IS NOT OFFERED OVER THE CHANNEL, because the channel is not ours to clear", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("clear-view").hidden, false, "Clear left the transcript, where it belongs");

  await showDiscord(page, [message({ id: "1000000000000000001" })]);
  assert.equal(
    page.el("clear-view").hidden,
    true,
    "Clear is offered over a Discord channel, where it either means nothing or means something alarming"
  );

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.el("clear-view").hidden, false, "Clear did not come back with the transcript");
});

test("A SECOND TAP ABORTS A READ THAT HAS NOT STARTED, and takes the highlight with it", async () => {
  // The wait is exactly where a mis-tap is noticed: the message is long, the reader realises they
  // did not want it, and the only thing to do is tap the thing they just tapped. Checking only
  // `nowPlaying` made that start a SECOND read, because nothing was playing yet.
  const page = newPage();
  // THE FALLBACK PATH ON PURPOSE. What is under test is a FETCH failing, and a prepared row does
  // not fetch — it is handed a URL and the failure, if there is one, arrives on the audio element
  // instead. Preparing is disabled so the wait and the refusal this covers still exist.
  page.prepareFails = true;
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  // Not awaited: this is the state mid-flight, which is the only moment the abort exists in.
  const inFlight = rows[0].dispatch("click", {});
  assert.equal(rowState(page, 0).pending, "true", "the tap did not register");

  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(rowState(page, 0).pending, "false", "the aborted row is still lit with nothing coming");
  assert.equal(
    page.el("read-aloud").getAttribute("data-read-state"),
    "ready",
    "the control still claims a read is in flight"
  );

  await inFlight;
  await page.settle();
  // The abandoned fetch must not come back and start playing anyway.
  const audible = page.players.filter((p) => p.playCount > 0 && !p.paused);
  assert.equal(audible.length, 0, "the aborted read played after being cancelled");
  assert.equal(rowState(page, 0).reading, "false");
});

test("...and turning the mode off aborts a pending read too", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  const inFlight = rows[0].dispatch("click", {});
  await page.el("read-aloud").click();
  await page.settle();
  await inFlight;
  await page.settle();

  assert.equal(rowState(page, 0).pending, "false", "leaving the mode left a row lit");
  const audible = page.players.filter((p) => p.playCount > 0 && !p.paused);
  assert.equal(audible.length, 0, "audio started after the reader left reading mode");
});

test("ASKED-FOR AND BEING-READ LOOK DIFFERENT, and the loud one is actually loud", async () => {
  // The first version of the reading treatment was a 14% ACCENT tint with a one-pixel border, and
  // it was effectively invisible: accent is a blue close in value to the dark surface it sat on,
  // and the coder tile it usually lands on had no side borders then (`#213 row-side-borders` gave
  // it four). Every test passed, because they all asserted the ATTRIBUTE and none of them asserted
  // that the attribute did anything.
  const reading = cssBlock('#discord-log li.discord-message[data-reading="true"]');
  const pending = cssBlock('#discord-log li.discord-message[data-pending="true"]');

  // The event gets the SURFACE: a fill, and a bar, because a hairline in any colour is too thin to
  // find a row by, on four sides as on two.
  assert.match(reading, /background:/, "the row being read has no fill, so it barely shows");
  assert.match(reading, /box-shadow:\s*inset/, "the row being read has only a hairline round it, and no bar");
  assert.match(reading, /--warn/, "the reading row is not the colour of the control that stops it");

  // The promise gets an EDGE, and must not be mistaken for the event.
  assert.match(pending, /outline:/, "the asked-for row has no edge");
  assert.doesNotMatch(pending, /background:/, "a promise is filled in like an event");
  assert.notEqual(reading, pending, "the two states are drawn identically");
});

test("A MESSAGE YOU OPENED STAYS OPEN WHEN THE CHANNEL RE-READS ITSELF", async () => {
  // The reported bug: a long message collapsed itself under the reader, repeatedly, while they
  // were still reading it. The channel re-reads every DISCORD_POLL_MS, `applyNewestPage` rebuilds
  // every row, and a fresh row starts folded. The row being READ ALOUD had a special case for
  // this; every other row was left with it.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1000000000000000001", content: "a".repeat(4000) }),
    message({ id: "1000000000000000002", content: "b".repeat(4000) }),
  ]);
  assert.equal(rows[0].getAttribute("data-collapsed"), "true");

  await rows[0].dispatch("click", {});
  assert.equal(row(page, 0).getAttribute("data-collapsed"), "false", "the tap did not open it");

  await reReadChannel(page);

  assert.equal(
    row(page, 0).getAttribute("data-collapsed"),
    "false",
    "the message the reader opened collapsed itself under them"
  );
  // ...and one they never opened is still closed: this restores a choice, it does not open the list.
  assert.equal(row(page, 1).getAttribute("data-collapsed"), "true", "the rebuild opened everything");
});

test("...and folding it again is remembered too, so the poll cannot re-open it", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [message({ id: "1000000000000000001", content: "c".repeat(4000) })]);

  await rows[0].dispatch("click", {});
  await rows[0].dispatch("click", {});
  assert.equal(row(page, 0).getAttribute("data-collapsed"), "true");

  await reReadChannel(page);
  assert.equal(row(page, 0).getAttribute("data-collapsed"), "true", "the poll re-opened a closed message");
});

test("THE BOOKMARK is set from the details sheet too, kept on the server, and drawn as a line above its message", async () => {
  // `#240 reading-bookmark`. Press-and-hold already opens the sheet, so the bookmark lives there as
  // well as on Shift+B and in the ⋯ menu, rather than on a gesture of its own.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1000000000000000001" }),
    message({ id: "1000000000000000002" }),
  ]);
  assert.equal(page.el("jump-bookmark").hidden, true, "a way back is offered with nowhere to go");

  await hold(page, rows[1]);
  const keep = rows[1].descendants().find((n) => n.text() === "Bookmark above");
  assert.ok(keep, "the details sheet offers no way to set the bookmark");
  await keep.click();
  await page.settle();

  assert.deepStrictEqual(bookmarkAt(page), { id: "1000000000000000002", place: "above" },
    "the line is not drawn above the message it was set on");
  assert.equal(rowState(page, 0).bookmark, null, "a second row carries the line");
  // The line's word is the separator, and the X beside it is not inside it (`#420 bookmark-clear`).
  const line = bookmarkAt(page, "discord-log", true).children.find((node) => node.className === "bookmark-word");
  assert.equal(line.getAttribute("role"), "separator", "the line is not a separator to a screen reader");
  assert.equal(line.getAttribute("aria-label"), "Bookmark");
  assert.deepStrictEqual(page.bookmarkCalls.map((call) => [call.method, call.body.message_id, call.ifRevision]),
    [["PUT", "1000000000000000002", undefined]], "the bookmark was not kept on the server, unconditionally");
  assert.deepStrictEqual(page.bookmark.anchor && page.bookmark.anchor.message_id, "1000000000000000002");
  assert.equal(page.storage.get("vibe-talk.voice.place-marker"), undefined, "the bookmark went to this browser's storage");
  assert.deepStrictEqual(page.dismissCalls, [], "setting the bookmark marked something Done");
  // The line is on screen, so the chip that goes back to it is not.
  assert.equal(page.el("jump-bookmark").hidden, true, "the chip is offered with the line in view");
  // ...and the sheet, opened again, offers to take it away.
  await hold(page, row(page, 1));
  const remove = row(page, 1).descendants().find((n) => n.text() === "Remove bookmark");
  assert.ok(remove, "the sheet on the message under the line does not offer to take it away");
  await remove.click();
  await page.settle();
  assert.equal(bookmarkAt(page), null, "Remove bookmark left the line");
  assert.equal(page.bookmark.anchor, null);
});

test("...and there is exactly ONE bookmark in a channel: setting it again moves it", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({ id: "1000000000000000001" }),
    message({ id: "1000000000000000002" }),
  ]);
  await hold(page, rows[0]);
  await rows[0].descendants().find((n) => n.text() === "Bookmark above").click();
  await page.settle();
  await hold(page, row(page, 1));
  await row(page, 1).descendants().find((n) => n.text() === "Bookmark above").click();
  await page.settle();
  assert.equal(rowState(page, 0).bookmark, null, "the old place was kept as well as the new one");
  assert.equal(rowState(page, 1).bookmark, "above");
  assert.equal(page.el("discord-log").children.filter((li) => li.descendants().some((n) => n.className === "bookmark-line")).length,
    1, "two lines are drawn");
});

test("A MESSAGE THAT IS ITSELF A REPLY SAYS SO, with an arrow in the gutter to its left", async () => {
  // The channel already derived the OPPOSITE direction — `data-replied` marks a message somebody
  // answered — and had nothing for "this one is an answer", even though the pointer was already on
  // the row. #97 reply-indicator. That answer was a small glyph by the time, which the owner called
  // useless and asked to be an arrow out of the box's left side: `#204 reply-arrow`.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000001", content: "is the runner wedged?" }),
    message({ id: "1000000000000000002", content: "restarted it", reply_to: "1000000000000000001" }),
  ]);

  assert.equal(rowState(page, 0).isReply, "false", "the question was marked as an answer");
  assert.equal(rowState(page, 1).isReply, "true", "the answer is not marked at all");

  const arrow = replyArrowOf(row(page, 1));
  assert.ok(arrow, "there is no arrow on the reply, only an attribute nobody can see");
  assert.equal(replyArrowOf(row(page, 0)), undefined, "a message that answers nothing grew an arrow");
  // A control, and the FIRST thing in the row: a screen reader meets the way to the question
  // before the answer's own words, and the stylesheet draws it in the gutter, not in the meta line.
  assert.equal(arrow.tagName, "button");
  assert.equal(arrow.getAttribute("type"), "button", "the arrow is not declared a plain button");
  assert.equal(row(page, 1).children[0], arrow, "the arrow is not the first thing in the row");
  // An arrow on its own is announced as "up arrow", or as nothing: it is named for what it DOES.
  assert.equal(arrow.getAttribute("aria-label"), "Jump to the message this answers");
  // The tooltip adds what a second tap does (`#215 reply-coalesce`), where a pointer resting on it
  // can find out; the name stays what one tap does.
  assert.equal(arrow.getAttribute("title"),
    "Jump to the message this answers. Double-tap to gather its whole conversation.");
  // DRAWN, by the stylesheet: a typed glyph looks like whatever the phone's font makes of it.
  assert.equal(arrow.text(), "", "the arrow is a typed glyph again");

  // The old glyph is gone from the meta line. What is left there is words, for a screen reader.
  assert.doesNotMatch(row(page, 1).text(), /↩/, "the tiny glyph the owner called useless is still drawn");
  assert.ok(!row(page, 1).descendants().some((n) => n.hasClass("reply-mark")), "the old mark is still built");
  const said = row(page, 1).descendants().find((n) => n.hasClass("reply-said"));
  assert.ok(said, "a screen reader is no longer told the row is a reply");
  assert.ok(said.hasClass("sr-only"), "the meta line still DRAWS that the row is a reply");
  assert.ok(!row(page, 0).descendants().some((n) => n.hasClass("reply-said")), "every row says it is a reply");
});

test("...and it is correct even when the message it answers is not loaded", async () => {
  // This is why it is worth its own signal rather than being inferred from the list. `data-replied`
  // is DERIVED from what happens to be loaded and misses an answer further back than the window;
  // this pointer is on the message itself.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [
    message({ id: "1000000000000000002", content: "restarted it", reply_to: "999999999999999999" }),
  ]);
  assert.equal(
    rowState(page, 0).isReply,
    "true",
    "a reply to something outside the window stopped looking like a reply"
  );
});

// --- `#548 cloud-read-failures`: a cloud read the voice fails stops and says why ----------------------

/** `count` samples of digital silence, and of a tone loud enough to be speech. */
const silentPcm = (count) => pcm(...new Array(count).fill(0));
const loudPcm = (count) => pcm(...Array.from({ length: count }, (_, at) => (at % 2 ? 3000 : -3000)));

async function cloudReadPage() {
  const page = newPage();
  page.enableStreamedAudio();
  page.enableDeviceSpeech();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", author_id: "1000000000000000011", content: "A message the voice will not say." }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "Another message." }),
  ]);
  return { page, rows };
}

test("`#548 cloud-read-failures`: a cloud read answered with silence stops, says so, reports it, and offers the device voice", async () => {
  const { page, rows } = await cloudReadPage();
  await rows[0].dispatch("click", {});
  await page.settle();
  // What a voice service denied speech access sends: about 0.85 s of zeros, then the end of the turn.
  page.audioStreams[0].controller.bytes(joined(wavHeader(24000, 1), silentPcm(20400)));
  page.audioStreams[0].controller.drop();
  await page.settle();
  assert.match(page.el("error").textContent, /^Cloud voice isn't producing audio: it sent silence/);
  assert.equal(page.el("error-wrap").hidden, false);
  assert.equal(page.el("read-on-device").hidden, false, "the device voice was not offered");
  assert.equal(rowState(page, 0).reading, "false", "the read went on in silence");
  assert.equal(page.el("read-aloud-label").text(), "Stop", "the read is still in progress");
  assert.deepEqual(page.dismissCalls, [], "a message nobody heard was archived");
  assert.deepEqual(page.speechFailureCalls.map((call) => call.failure.reason), ["silent"]);
  assert.match(page.speechFailureCalls[0].failure.detail, /sent silence/);

  // Read with device voice: that message, by the device voice, this once.
  await page.el("read-on-device").click();
  await page.settle();
  assert.deepEqual(page.deviceSpeech.utterances.map((one) => one.text), ["A message the voice will not say."]);
  assert.equal(page.el("read-on-device").hidden, true);
  assert.equal(page.el("error-wrap").hidden, true);
  page.deviceSpeech.utterances[0].end();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [{ messages: ["1000000000000000001"] }], "the device voice's read was not archived");
  // The voice selected is still the cloud's: the next tap streams.
  const streams = page.audioStreams.length;
  await rows[1].dispatch("click", {});
  await page.settle();
  assert.equal(page.audioStreams.length, streams + 1, "Read with device voice changed the selected voice");
  assert.equal(page.deviceSpeech.utterances.length, 1);
});

test("`#548 cloud-read-failures`: no speech within a few seconds stops a cloud read, but a held read waits and a read with speech goes on", async () => {
  const { page, rows } = await cloudReadPage();
  const limit = sourceConstant("READ_NO_SPEECH_MS");
  await rows[0].dispatch("click", {});
  await page.settle();
  // A voice slow to answer is not a voice producing silence: the wait counts from the first audio.
  page.expireTimers(limit);
  await page.settle();
  assert.equal(page.el("error").textContent, "", "a read was stopped before its first audio came");
  page.audioStreams[0].controller.bytes(joined(wavHeader(24000, 1), loudPcm(2400)));
  await page.settle();
  page.expireTimers(limit);
  await page.settle();
  assert.equal(page.el("error").textContent, "", "a read with speech in it was stopped");
  assert.equal(rowState(page, 0).reading, "true");

  // The second message: a header, and a tenth of a second of silence, and then nothing.
  await rows[1].dispatch("click", {});
  await page.settle();
  page.audioStreams.at(-1).controller.bytes(joined(wavHeader(24000, 1), silentPcm(2400)));
  await page.settle();
  await page.el("read-aloud").click();
  await page.settle();
  assert.equal(page.el("read-aloud-label").text(), "Resume", "the premise: the read is held");
  page.expireTimers(limit);
  await page.settle();
  assert.equal(page.el("error").textContent, "", "a held read was judged for its silence");
  await page.el("read-aloud").click();
  await page.settle();
  page.expireTimers(limit);
  await page.settle();
  assert.match(page.el("error").textContent, /^Cloud voice isn't producing audio: no speech came within 4 seconds of its first audio/);
  assert.equal(rowState(page, 1).reading, "false");
  assert.deepEqual(page.speechFailureCalls.map((call) => call.failure.reason), ["no_audio"]);
  assert.deepEqual(page.dismissCalls, []);
});

test("`#548 cloud-read-failures`: a read the voice cut off naming why stops at once with that reason, and one merely dropped is tried again", async () => {
  const { page, rows } = await cloudReadPage();
  await rows[0].dispatch("click", {});
  await page.settle();
  // The private bridge: silent audio, then an error naming the cause, then the socket closed; the
  // server can only cut the response off, and keeps why under the ticket.
  page.ticketFailure = {
    error: "conversation_speech_error",
    detail: "Test agent read-aloud session failed: the agent reported: the speech service denied access " +
      "(ACL TIER:example); no audio was produced (speech_access_denied)",
  };
  page.audioStreams[0].controller.bytes(joined(wavHeader(24000, 1), silentPcm(9600)));
  await page.settle();
  page.audioStreams[0].controller.fail();
  await page.settle();
  // The answer to the question, then what it says.
  await page.settle();
  assert.equal(page.speechFailureLookups.length, 1, "the server was not asked why the read was cut off");
  assert.equal(page.el("error").textContent, "Cloud voice: speech access denied (ACL). The message was not archived.");
  assert.equal(page.audioStreams.length, 1, "a read the voice refused was tried again");
  assert.doesNotMatch(page.el("status").textContent, /trying again/);
  assert.equal(page.el("read-on-device").hidden, false);
  assert.deepEqual(page.speechFailureCalls.map((call) => call.failure.reason), ["refused"]);
  assert.deepEqual(page.dismissCalls, []);

  // The server's own judgement of a turn of silence is said as the silence it is.
  page.ticketFailure = {
    error: "conversation_speech_silent",
    detail: "Test agent answered the read with silence: no speech was produced (conversation_speech_silent)",
  };
  await rows[1].dispatch("click", {});
  await page.settle();
  page.audioStreams.at(-1).controller.bytes(joined(wavHeader(24000, 1), silentPcm(9600)));
  await page.settle();
  page.audioStreams.at(-1).controller.fail();
  await page.settle();
  await page.settle();
  assert.match(page.el("error").textContent, /^Cloud voice isn't producing audio: it sent silence/);
  assert.deepEqual(page.speechFailureCalls.map((call) => call.failure.reason), ["refused", "silent"]);

  // A connection merely dropped: nothing kept, and the read is tried again (`#487`).
  page.ticketFailure = null;
  await rows[1].dispatch("click", {});
  await page.settle();
  page.audioStreams.at(-1).controller.bytes(joined(wavHeader(24000, 1), loudPcm(2400)));
  await page.settle();
  page.audioStreams.at(-1).controller.fail();
  await page.settle();
  await page.settle();
  assert.match(page.el("status").textContent, /cut off; trying again/);
});

test("`#548 cloud-read-failures`: a refused cloud read says the voice's reason in short words", async () => {
  const { page, rows } = await cloudReadPage();
  page.ticketStatus = 502;
  page.ticketRefusal = {
    error: "conversation_speech_error",
    detail: "Test agent read-aloud connection failed: the agent reported: speech access denied (speech_access_denied)",
  };
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("error").textContent, "Cloud voice: speech access denied (ACL). The message was not archived.");
  assert.equal(page.el("read-on-device").hidden, false);
  assert.deepEqual(page.speechFailureCalls.map((call) => call.failure.reason), ["refused"]);
  // A reason no pattern knows is said as it came.
  page.ticketRefusal = { error: "conversation_speech_error", detail: "the voice is on fire" };
  await rows[1].dispatch("click", {});
  await page.settle();
  assert.equal(page.el("error").textContent, "Cloud voice: the voice is on fire. The message was not archived.");
});

// --- `#487 read-aloud-recovery`: a read whose server went away ---------------------------------------

test("A READ CUT OFF BY A RESTART prepares again and carries on from the block it was on", async () => {
  // The owner, 2026-10-23: a read died part-way when the server restarted under it, and tapping again
  // did nothing. A restarted server has forgotten every ticket: the page prepares again and goes on.
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  await page.el("read-next").click();
  await page.settle();
  assert.equal(page.el("read-steer-count").text(), "2 of 3", "the premise: the read is not on its second block");
  const before = page.prepareCalls.length;
  page.audioStreams.at(-1).controller.bytes(joined(wavHeader(1000, 1), pcm(0x2000, 0x2000)));
  await page.settle();
  // The server goes away: the block being heard is cut off, and its ticket is gone with it.
  page.speechTicketSuffix = "-after-restart";
  page.audioStreams.at(-2).controller.fail();
  await page.settle();
  assert.match(page.el("status").textContent, /cut off; trying again/);
  assert.equal(rowState(page, 0).pending, "true", "the row does not say its read is coming back");
  // `scrubber-full-width`: the strip, and a phone's layout for it, stay up through the wait.
  assert.equal(page.el("read-steer").hidden, false, "the strip went for the wait to try again");
  assert.equal(page.el("read-steer-count").text(), "2 of 3");
  assert.equal(readStrip(page), "playing", "the playback layout dropped for the wait to try again");
  assert.equal(page.expireTimers(page.pure("READ_RECOVERY_WAITS_MS")[0]), 1, "nothing waited to try again");
  await page.settle();
  assert.ok(page.prepareCalls.slice(before).some((call) => (call.ids || []).includes(BLOCKED_ID)),
    "the read went on without preparing again");
  const resumed = page.audioStreams.at(-1);
  assert.match(resumed.url, /-after-restart\?/, `the read went on from a ticket minted before the restart: ${resumed.url}`);
  assert.equal(blockOf(resumed.url), "1", "the read did not go on from the block it was cut off in");
  assert.equal(page.el("read-steer-count").text(), "2 of 3");
});

test("a tap after a read that could not be played prepares again, rather than playing another dead URL", async () => {
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [
    message({ id: "1000000000000000001", author_id: "1000000000000000011", content: "first" }),
    message({ id: "1000000000000000002", author_id: "1000000000000000012", content: "second" }),
  ]);
  // Both rows prepared before the server restarts; every try at the first then finds it gone.
  page.ticketStatus = 404;
  await rows[0].dispatch("click", {});
  await page.settle();
  for (const wait of page.pure("READ_RECOVERY_WAITS_MS")) {
    assert.equal(page.expireTimers(wait), 1);
    await page.settle();
  }
  assert.match(page.el("status").textContent, /Cloud voice: the connection to it kept dropping\./);
  // The server is back. A tap on the other row asks for a fresh ticket first.
  page.ticketStatus = 200;
  page.speechTicketSuffix = "-fresh";
  const before = page.prepareCalls.length;
  await rows[1].dispatch("click", {});
  await page.settle();
  assert.ok(page.prepareCalls.slice(before).some((call) => (call.ids || []).includes("1000000000000000002")),
    "the tap played the ticket prepared before the server restarted");
  assert.match(page.audioStreams.at(-1).url, /-fresh\?/);
});

test("a read stopped while it waits to start again gives back the media keys it held", async () => {
  // From review: the wait keeps the media session as a jump does; a stop then found nothing playing and
  // left the lock screen's controls claimed.
  const { page, rows } = await blockedRead({ mediaSession: true });
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.mediaSession.playbackState, "playing", "the premise: the read holds the media session");
  page.audioStreams[0].controller.fail();
  await page.settle();
  assert.match(page.el("status").textContent, /cut off; trying again/);
  await page.el("discord-log").children[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.mediaSession.playbackState, "none", "the stopped read kept the media session");
  assert.equal(page.mediaSession.handlers.pause, null, "the stopped read left its media keys bound");
});

test("a tap during the wait to try again is the reader's own: the try leaves it alone", async () => {
  const { page, rows } = await blockedRead();
  await rows[0].dispatch("click", {});
  await page.settle();
  page.audioStreams[0].controller.fail();
  await page.settle();
  assert.match(page.el("status").textContent, /cut off; trying again/);
  assert.equal(page.el("read-steer").hidden, false, "the premise: the strip is held through the wait");
  // A tap on the row whose read is coming back stops it, as a tap on a row being read does.
  await page.el("discord-log").children[0].dispatch("click", {});
  await page.settle();
  // `scrubber-full-width`: and the strip held through the wait goes with it.
  assert.equal(page.el("read-steer").hidden, true, "the strip held through the wait outlived the stop");
  assert.equal(readStrip(page), null, "the playback layout outlived the stop during the wait");
  const asked = page.audioStreams.length;
  page.expireTimers(10_000);
  await page.settle();
  assert.equal(page.audioStreams.length, asked, "the try started a read the reader had stopped");
  assert.equal(rowState(page, 0).pending, "false");
});

test("a streamed read that ENDS PART-WAY through a sample did not arrive whole, and archives nothing", async () => {
  // `#50 stream-trailing-byte`. A frame is two bytes a channel; a body that stops inside one was
  // cut short, or its length disagrees with its format, whatever the transport said about the end.
  const endings = [
    // Samples loud enough to be speech: a part that is all silence is refused (`#540`, below).
    ["whole stereo frames", 2, pcm(1000, 2000, 3000, 4000), true],
    ["an odd trailing byte", 1, joined(pcm(1000, 2000, 3000), new Uint8Array([7])), false],
    ["half a stereo frame", 2, pcm(1000, 2000, 3000, 4000, 5000), false],
    ["a stereo frame short by one byte", 2, joined(pcm(1000, 2000, 3000, 4000, 5000), new Uint8Array([7])), false],
  ];
  for (const [ending, channels, samples, whole] of endings) {
    const page = newPage();
    page.enableStreamedAudio();
    await signIn(page);
    const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
    await rows[0].dispatch("click", {});
    await page.settle();
    page.audioStreams[0].controller.bytes(joined(wavHeader(24000, channels), samples));
    await page.settle();
    page.audioStreams[0].controller.drop();
    await page.settle();
    for (const source of page.audio.sources) source.onended();
    await page.settle();
    if (whole) {
      assert.deepEqual(page.dismissCalls, [{ messages: ["1000000000000000001"] }], `${ending} was not archived`);
      continue;
    }
    assert.equal(page.audio.sources[0].halted, true, `${ending} played on`);
    assert.match(page.el("status").textContent, /Cloud voice: it could not play this message\./, `${ending} passed as a clean end`);
    assert.equal(rowState(page, 0).reading, "false", `${ending} left the row looking like it is speaking`);
    assert.deepEqual(page.dismissCalls, [], `${ending} was archived as heard`);
  }
});

test("a streamed part that arrives whole with no speech in it stops the read, says so, and archives nothing", async () => {
  // `#540 read-aloud-desktop`. A voice that answered with silence: the page used to step on through every
  // block in silence and then archive the message as heard.
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);
  await rows[0].dispatch("click", {});
  await page.settle();
  page.audioStreams[0].controller.bytes(joined(wavHeader(24000, 1), pcm(0, 3, -2, 0)));
  await page.settle();
  page.audioStreams[0].controller.drop();
  await page.settle();
  assert.match(page.el("status").textContent, /sent silence/, "the reader was not told the voice was silent");
  assert.match(page.el("error").textContent, /sent silence/);
  assert.equal(page.audio.sources[0].halted, true, "the silent part played on");
  assert.equal(rowState(page, 0).reading, "false", "the row still looks like it is speaking");
  for (const source of page.audio.sources) source.onended();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [], "a message read in silence was archived as heard");
});

test("a streamed COMBINED row fetches its next part on the first part's audio and plays it after", async () => {
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, splitPost());

  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(page.audioStreams.length, 1, "the second part was requested before the first had audio");
  const audio = page.audio;
  page.audioStreams[0].controller.bytes(joined(wavHeader(24000, 1), pcm(1000, 2000)));
  await page.settle();
  assert.equal(page.audioStreams.length, 2, "the second part was never requested");
  const selection = (url) => new URL(url, "http://page").searchParams.get("selection");
  assert.equal(selection(page.audioStreams[1].url), selection(page.audioStreams[0].url));

  page.audioStreams[1].controller.bytes(joined(wavHeader(24000, 1), pcm(3000, 4000)));
  await page.settle();
  assert.equal(audio.sources.length, 1, "both parts played over each other");

  page.audioStreams[0].controller.drop();
  await page.settle();
  audio.sources[0].onended();
  await page.settle();
  assert.equal(audio.sources.length, 2, "the second part never followed the first");
  assert.deepEqual([...audio.sources[1].buffer.getChannelData(0)], [3000 / 0x8000, 4000 / 0x8000]);
  assert.deepEqual(page.dismissCalls, [], "the row was filed away with half of it unheard");

  page.audioStreams[1].controller.drop();
  await page.settle();
  audio.sources[1].onended();
  await page.settle();
  assert.deepEqual(page.dismissCalls, [
    { messages: ["1000000000000000001", "1000000000000000002"] },
  ]);
});

test("leaving Read aborts a streamed read and lets the audio output go, until the next tap", async () => {
  const page = newPage();
  page.enableStreamedAudio();
  await signIn(page);
  const rows = await inReadingMode(page, [message({ id: "1000000000000000001" })]);

  await rows[0].dispatch("click", {});
  await page.settle();
  const audio = page.audio;
  assert.equal(audio.state, "running");
  // `#434 read-aloud-pause`: the Read button first PAUSES a playing read — the response is held,
  // not aborted — and stopping the read (a tap on its row) and then leaving aborts it as before.
  await readButton(page).click();
  await page.settle();
  assert.equal(page.audioStreams[0].options.signal.aborted, false, "pausing aborted the response it only meant to hold");
  assert.equal(audio.state, "suspended", "the pause did not stop the audio clock");
  await rows[0].dispatch("click", {});
  await page.settle();
  await readButton(page).click();
  await page.settle();
  assert.equal(page.audioStreams[0].options.signal.aborted, true, "the response outlived the mode");
  assert.equal(audio.state, "suspended", "the audio output was held open with nothing to read");

  await readButton(page).click();
  await page.settle();
  await rows[0].dispatch("click", {});
  await page.settle();
  assert.equal(audio.state, "running", "the next tap did not wake the audio back up");
  assert.equal(page.audioStreams.length, 2);
});

test("...and stopping a combined read part-way releases EVERY audio URL it fetched", async () => {
  const page = newPage();
  page.prepareFails = true; // This test owns the blob-URL lifecycle, not prepared streaming URLs.
  await signIn(page);
  const rows = await inReadingMode(page, splitPost());

  await rows[0].dispatch("click", {});
  await page.settle();
  await rows[0].dispatch("click", {});
  await page.settle();

  assert.equal(page.players[0].paused, true, "a second tap did not stop the audio");
  assert.equal(
    page.revokedUrls.length,
    2,
    "the part that was fetched but never reached was leaked"
  );
  assert.deepEqual(page.dismissCalls, [], "stopping halfway filed the row away");
});

test("COMBINING IS ON BY DEFAULT, and the switch redraws the rows without re-reading the channel", async () => {
  const page = newPage();
  await signIn(page);
  assert.equal(page.el("combine-messages").checked, true, "the default is not on");
  const rows = await showDiscord(page, splitPost());
  assert.equal(rows.length, 1);
  const reads = page.pageReads;

  await page.el("combine-messages").setChecked(false);
  await page.settle();

  const raw = page.el("discord-log").children;
  assert.equal(raw.length, 2, "turning it off did not put the raw Discord rows back");
  assert.deepEqual(shownIds(page), ["1000000000000000001", "1000000000000000002"]);
  assert.equal(page.pageReads, reads, "flipping a display switch cost a round trip to the server");
  assert.equal(page.storage.get("vibe-talk.voice.combine"), "0", "the choice was not kept");

  await page.el("combine-messages").setChecked(true);
  await page.settle();
  assert.equal(page.el("discord-log").children.length, 1, "turning it back on did not re-combine");

  // ...and the choice survives a reload, which is what makes it a setting rather than a mode.
  await page.el("combine-messages").setChecked(false);
  const again = reloadWith(page.storage, splitPost());
  await signIn(again);
  assert.equal(again.el("combine-messages").checked, false, "the switch came back on by itself");
  assert.equal((await reopenedChannel(again)).length, 2, "the rows combined anyway");
});

test("A MESSAGE ARRIVING SECONDS AFTER THE LAST JOINS ITS ROW, without moving the reader", async () => {
  // The live half. Split posts arrive over the stream far more often than they are first seen in a
  // poll, so an appender that could not join would leave exactly the pair this feature is for as
  // two rows until something else happened to redraw them.
  const page = newPage();
  const channel = tallChannel();
  const stream = await withLiveChannel(page, channel);
  const area = page.el("scroll-area");
  const parked = parkMidway(page);
  assert.ok(parked > 0, "the fixture is not tall enough to have a place to lose");
  const before = page.el("discord-log").children.length;

  const previous = channel[channel.length - 1];
  const overflow = message({
    id: "9000000000000000001",
    content: "and the rest of that sentence.",
    timestamp: new Date(Date.parse(previous.timestamp) + 1500).toISOString(),
  });
  await deliver(page, stream, sseMessage(overflow));

  const rows = page.el("discord-log").children;
  assert.equal(rows.length, before, "the trailing half arrived as a row of its own");
  const last = rows[rows.length - 1];
  assert.match(last.text(), /and the rest of that sentence/, "the arriving text is not on screen");
  assert.equal(
    last.getAttribute("data-ids").split(" ").includes("9000000000000000001"),
    true,
    "the row it joined does not admit to holding it"
  );
  // "New events, new messages. None of that should move our place."
  assert.equal(area.scrollTop, parked, "a message joining a row dragged the reader");
  assert.equal(page.el("jump-newest").hidden, false, "nothing said a message had arrived");

  // ...and the same message arriving twice does not double it, even though the row it belongs to
  // is not filed under its id.
  await deliver(page, stream, sseMessage(overflow));
  assert.equal(page.el("discord-log").children.length, before, "a combined message arrived twice");
  // ...and it is not silently absorbed into the same row a SECOND time, which is what a dedupe
  // that only compared row identities would do: the row count would not move and the text would
  // simply be there twice.
  const everyId = [...page.el("discord-log").children].flatMap((li) =>
    li.getAttribute("data-ids").split(" ")
  );
  assert.equal(
    everyId.filter((one) => one === "9000000000000000001").length,
    1,
    "the arriving message was combined into the row twice"
  );
});

test("THE BOOKMARK DOES NOT SLIDE PAST A ROW THAT IS ONLY HALF DEALT WITH, and names a real message", async () => {
  // `#240 reading-bookmark`. A combined row with one message still outstanding is still something to
  // deal with, so a read that finds half of it Done moves nothing — reads never move the bookmark —
  // and the line is anchored to the row's FIRST real message, never to an id this page made up.
  const page = newPage();
  await signIn(page);
  const later = message({
    id: "1000000000000000003",
    content: "a separate thing entirely",
    timestamp: "2026-08-19T04:40:00.000Z",
  });
  const rows = await showDiscord(page, [...splitPost(), later]);
  assert.equal(rows.length, 2, "the fixture is not exercising a combined row");
  await hold(page, rows[0]);
  await rows[0].descendants().find((n) => n.text() === "Bookmark above").click();
  await page.settle();
  assert.equal(page.bookmarkCalls[0].body.message_id, "1000000000000000001", "the bookmark is not a real message id");

  page.dealtWith.add("1000000000000000001");
  await reReadChannel(page);
  assert.equal(rowState(page, 0).bookmark, "above", "the line stepped over a row still half to do");
  assert.equal(rowState(page, 1).bookmark, null);

  // The row's own Done archives what is left of it, and only then does the line slide on.
  await doneButton(row(page, 0)).click();
  await page.settle();
  assert.equal(rowState(page, 1).bookmark, "above", "the line never followed the reader down");
  assert.equal(page.bookmark.anchor.message_id, "1000000000000000003");
});

test("THE BULK CLEAR'S BOUNDARY IS THE NEWEST MESSAGE ON SCREEN, not the last row's identity", async () => {
  // `through` is resolved by the server against its own ordering, and the boundary is INCLUDED.
  // Sending a combined bottom row's identity stops the sweep one message short and leaves the
  // trailing overflow in the queue — which is the one thing bankruptcy must not do.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, splitPost());
  await turnTodoOn(page);

  await page.el("clear-backlog").click();
  // The count it warns with is in MESSAGES, because that is what is about to be cleared.
  assert.match(page.el("status").textContent, /clear 2\b/, "the warning counted rows, not messages");
  await page.el("clear-backlog").click();
  await page.settle();

  assert.equal(page.dismissCalls.length, 1);
  assert.equal(
    page.dismissCalls[0].through,
    "1000000000000000002",
    "the sweep stopped short of the trailing half of the bottom row"
  );
  assert.equal(shownIds(page).length, 0, "something was left in the queue");
});

test("PRESS AND HOLD ON A COMBINED ROW NAMES EVERY ID IT IS SHOWING", async () => {
  // The sheet is the answer to "does that message really exist", and this view exists because the
  // agent has described messages that did not. A combined row naming only its first constituent
  // makes the second unreachable by exactly the affordance built to reach it.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, splitPost());

  await hold(page, rows[0]);

  const shown = rows[0].descendants().find((n) => n.className === "msg-id");
  assert.ok(shown, "the details sheet does not name an id at all");
  assert.match(shown.text(), /1000000000000000001/);
  assert.match(shown.text(), /1000000000000000002/, "the trailing half of the row is unnamed");
});

test("a message COMBINED INTO A ROW still dims the question it answered", async () => {
  // `data-replied` is a census over every reply pointer in the list. Reading only each row's
  // identity would quietly stop dimming a question whose answer happened to be combined.
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, [
    message({
      id: "1000000000000000001",
      author: "alice",
      author_id: "30",
      author_is_bot: false,
      content: "is the runner wedged?",
      timestamp: "2026-08-19T04:31:00.000Z",
    }),
    message({
      id: "1000000000000000002",
      content: "yes — I restarted it and it is",
      timestamp: "2026-08-19T04:35:00.000Z",
    }),
    message({
      id: "1000000000000000003",
      content: "back up now.",
      reply_to: "1000000000000000001",
      timestamp: "2026-08-19T04:35:01.000Z",
    }),
  ]);

  assert.equal(rows.length, 2, "the fixture is not exercising a combined answer");
  assert.equal(
    rowState(page, 0).replied,
    "true",
    "the question stopped being dimmed because its answer was combined into a row"
  );
});

test("a row is FOLDED on its combined length, and stays open across the poll", async () => {
  // Two halves each comfortably short enough to need no fold, whose combination is a wall of text.
  // Deciding foldability on the first alone leaves the reader with the row this feature exists to
  // prevent: everything, unclamped, on every line of the list.
  const page = newPage();
  await signIn(page);
  const half = "a sentence about the overnight run. ".repeat(
    Math.ceil(COLLAPSE_OVER_CHARS / 2 / 36)
  );
  assert.ok(half.length <= COLLAPSE_OVER_CHARS, "each half is long enough to fold on its own");
  assert.ok(half.length * 2 > COLLAPSE_OVER_CHARS, "the two halves together are not long enough");
  const rows = await showDiscord(
    page,
    splitPost({ content: `first: ${half}` }, { content: `second: ${half}` })
  );

  assert.equal(rows.length, 1);
  assert.ok(foldButton(rows[0]), "a combined wall of text got no fold control");
  assert.equal(rows[0].getAttribute("data-collapsed"), "true", "it arrived open");

  await rows[0].dispatch("click", {});
  assert.equal(row(page, 0).getAttribute("data-collapsed"), "false", "the tap did not open it");
  await reReadChannel(page);
  assert.equal(
    row(page, 0).getAttribute("data-collapsed"),
    "false",
    "the poll folded the combined row under the reader"
  );
});

test("NO ID THIS PAGE INVENTED EVER REACHES THE SERVER OR THE STORE", async () => {
  // The whole posture of the feature in one assertion. Combining is a rendering decision; the
  // model underneath stays one-to-one with Discord, so every id on the wire and every id written
  // down has to be one Discord really issued.
  const page = newPage();
  await signIn(page);
  const real = new Set(["1000000000000000001", "1000000000000000002"]);
  const rows = await inReadingMode(page, splitPost());

  // FIRST, while the row is still outstanding: the bookmark legitimately slides on past a row read
  // to its end, so what was set is read back before the read-aloud. `#240 reading-bookmark`.
  await hold(page, rows[0]);
  await rows[0].descendants().find((n) => n.text() === "Bookmark above").click();
  await page.settle();
  assert.equal(page.bookmarkCalls[0].body.message_id, "1000000000000000001", "a combined id was sent as the bookmark");

  // A press-and-hold ends in a `pointerup` and the browser fires a `click` behind it, which the
  // page swallows so the gesture is not also read as a tap. So the first click here is that one.
  await row(page, 0).dispatch("click", {});
  await row(page, 0).dispatch("click", {});
  await page.settle();
  await page.players[0].end();
  await page.players[1].end();
  await page.settle();
  await replyButton(row(page, 0)).click();
  page.el("reply-text").value = "noted";
  await page.el("reply-send").click();
  await page.settle();

  const sent = [
    ...page.speakCalls,
    ...page.prepareCalls.flatMap((call) => call.ids),
    ...page.dismissCalls.flatMap((call) => call.messages || []),
    ...page.repliesPosted.map((call) => call.body.reply_to),
    ...page.bookmarkCalls.filter((call) => call.body).map((call) => call.body.message_id),
  ];
  assert.ok(sent.length >= 5, `nothing was exercised: ${JSON.stringify(sent)}`);
  for (const id of sent) {
    assert.ok(real.has(String(id)), `the page sent ${id}, which is not a Discord message id`);
  }
});

test("linked conversation sources stay in the channel picker when thread timelines are off", async () => {
  const page = newPage();
  const first = { id: "1110000000000000101", label: "first linked conversation", writable: false,
    added: true };
  const second = { id: "1110000000000000102", label: "second linked conversation", writable: false,
    added: true };
  page.channels = [first, second];
  page.threadingSupported = false;
  // This is the misleading child projection from the reported failure. It must not replace the
  // two registered sources merely because one source's history contains thread metadata.
  page.threads = [{ id: "opaque-unrelated-child", title: "unrelated thread", reply_count: 1 }];
  const reads = [];
  page.channelPage = async (path) => {
    reads.push(String(path));
    const selected = page.channels.find((channel) => String(path).includes(channel.id));
    return json(200, { channel: selected, messages: [] });
  };

  await signIn(page);
  await showDiscord(page, []);
  assert.deepStrictEqual(
    optionText(page, "discord-channel"),
    [first.label, second.label],
    "an unrelated child thread displaced the registered source choices"
  );
  assert.equal(page.el("channel-view-tabs").hidden, true, "disabled child timelines became a menu");
  // The picker stays on the bar, inert, offering only Main: there is nothing else to choose.
  assert.deepStrictEqual(threadOptions(page), [["main", "Main"]]);
  assert.equal(page.el("thread-select").disabled, true, "a channel without threads offered thread choices");
  assert.equal(page.el("thread-list").hidden, true, "an unrelated child thread became visible");
  assert.deepStrictEqual(page.timelineCalls, [], "the disabled timeline API was queried");
  assert.match(reads.at(-1), new RegExp(first.id), "the first source was not read as an app channel");

  page.el("discord-channel").value = second.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.match(reads.at(-1), new RegExp(second.id), "the second source was not independently readable");
  assert.deepStrictEqual(optionText(page, "discord-channel"), [first.label, second.label]);
  assert.deepStrictEqual(page.timelineCalls, []);
});

test("each channel uses its own provider's capabilities in a multi-provider deployment", async () => {
  // One bridge without threads and one Slack-like provider with them. The deployment-wide flag says
  // "some provider has threads"; the channel without them must still be read as a flat history,
  // and the threaded one as a timeline, whichever was selected first.
  const page = newPage();
  const space = { id: "1110000000000000201", label: "chat space", writable: false, provider: "gchat" };
  const team = { id: "C0123ABCDE", label: "slack team", writable: true, provider: "slack" };
  page.channels = [space, team];
  page.threadingSupported = true;
  page.selfAuthorId = null;
  page.providers = [
    { key: "gchat", name: "Google Chat", threading_supported: false, live_delivery: "push",
      live_poll_seconds: 0, channel_registration_supported: true, channel_discovery_supported: false,
      upstream_read_mark_supported: true, self_author_id: "900000000000000001", owner_author_id: null },
    { key: "slack", name: "Slack", threading_supported: true, live_delivery: "poll",
      live_poll_seconds: 15, channel_registration_supported: true, channel_discovery_supported: true,
      upstream_read_mark_supported: false, self_author_id: "U0SELF0001", owner_author_id: "U0OWNER001" },
  ];
  const reads = [];
  page.channelPage = async (path) => {
    reads.push(String(path));
    return json(200, { channel: space, messages: [] });
  };

  await signIn(page);
  await showDiscord(page, []);
  assert.deepStrictEqual(page.timelineCalls, [], "the provider without threads was read as a timeline");
  assert.match(reads.at(-1) || "", new RegExp(space.id));
  assert.match(page.el("live-state").textContent, /adapter push/);

  page.el("discord-channel").value = team.id;
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "the threaded provider's channel was not read as a timeline");
  assert.match(page.timelineCalls[0], new RegExp(team.id));
  assert.match(page.el("live-state").textContent, /every 15 seconds/);
});

test("thread views show roots in Main and the bar's selector offers Main, All, and each thread", async () => {
  const page = await threadPage();
  // `#189 restore-ui-state`: a channel whose provider has threads opens in All, read as All.
  assert.equal(page.el("thread-select").value, "flat", "a threaded channel did not open in All");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.match(page.timelineCalls[0], /view=flat/);
  await pickThread(page, "main");
  assert.equal(page.el("channel-view-tabs").hidden, true, "the tabs over the list came back");
  const offered = threadOptions(page);
  // `#281 view-keys`: the thread list after them, the third view; two columns only on a desk wide enough.
  assert.deepStrictEqual(offered.slice(0, 3), [["main", "Main"], ["flat", "All"], ["threads", "Thread list"]]);
  assert.deepStrictEqual(offered.slice(3).map(([value]) => value).sort(),
    ["thread:spaces/A/threads/another", "thread:spaces/A/threads/one & two"]);
  // Age · replies · a short prefix of its first message, so a phone's large picker stays readable.
  // The age is on `briefAge`'s one scale (`#194 thread-picker-polish`): now, 5m, 23h, 1d18h, 12d, 6w, 2y.
  assert.ok(offered.some(([, label]) => /^(now|\d+[mhdwy]|\d+d\d+h) · 1 · first thread root$/.test(label)),
    `a thread was not named by age, replies and its root: ${JSON.stringify(offered)}`);
  assert.equal(page.el("thread-select").value, "main");
  assert.deepEqual(page.el("discord-log").children.map((row) => row.getAttribute("data-id")), ["200", "201", "203"]);
  assert.equal(threadButton(page.el("discord-log").children[1]).textContent, "1 reply");
  const rootRow = page.el("discord-log").children[1];
  assert.ok(rootRow.children.indexOf(threadButton(rootRow)) > rootRow.children.findIndex((child) => child.className === "body"),
    "the thread reply-count decorator must sit underneath the root message");
  assert.equal(threadButton(page.el("discord-log").children[2]).textContent, "about 1 reply");
  // Drawn from All, which holds every row of Main: no read. `#220 view-switch-instant`.
  assert.deepStrictEqual(page.timelineCalls.map(viewOf), ["flat"], "Main was read though All holds it");
  assert.equal(page.pageReads, 0);
  const empty = newPage();
  empty.threadingSupported = true;
  await signIn(empty);
  await showDiscord(empty, [message({ content: "no threads yet" })]);
  assert.equal(empty.el("channel-view-tabs").hidden, true);
  // Always there, even with no threads yet: Main and All are still choices, and the thread list, which
  // says there are none yet (`#281 view-keys`).
  assert.deepStrictEqual(threadOptions(empty), [["main", "Main"], ["flat", "All"], ["threads", "Thread list"]]);
});

test("the bar's thread picker opens a thread, All, and Main, and touching it reads the thread list once", async () => {
  const page = await threadPage();
  const picker = page.el("thread-select");
  const pick = async (value) => {
    picker.value = value;
    await picker.dispatch("change");
    await page.settle();
  };

  await pick("thread:spaces/A/threads/one & two");
  assert.deepStrictEqual(shownIds(page), ["201", "202"], "picking a thread did not open it");
  assert.equal(picker.value, "thread:spaces/A/threads/one & two");
  assert.equal(page.el("channel-navigation").hidden, false, "an open thread lost its heading");

  await pick("flat");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"], "All did not show every message");
  assert.equal(picker.value, "flat");

  await pick("main");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "Main did not come back");
  assert.equal(page.el("channel-navigation").hidden, true);

  // Touching the picker asks for the channel's thread list in the background, and only once a
  // minute: the view on screen does not change.
  const threadReads = () => page.timelineCalls.filter((call) => /view=threads/.test(call)).length;
  const before = threadReads();
  await picker.dispatch("pointerdown");
  await page.settle();
  assert.equal(threadReads(), before + 1, "touching the picker did not read the thread list");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the background read changed the view");
  await picker.dispatch("focus");
  await page.settle();
  assert.equal(threadReads(), before + 1, "a second touch read the thread list again at once");
  // The summaries' own names win once they are known.
  assert.ok(threadOptions(page).some(([, label]) => / · 1 · First discussion$/.test(label)),
    `the read thread list did not name the threads: ${JSON.stringify(threadOptions(page))}`);
});

test("a thread's summarised display name is what the picker calls it, when the server has one", async () => {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads.map((thread, i) => (i === 0 ? { ...thread, display_name: "deploy-rollback" } : thread));
  await signIn(page);
  await showDiscord(page, data.messages);
  await page.el("thread-select").dispatch("pointerdown");
  await page.settle();
  const labels = threadOptions(page).map(([, label]) => label);
  assert.ok(labels.some((label) => / · 1 · deploy-rollback$/.test(label)),
    `the display name did not win over the title and the prefix: ${JSON.stringify(labels)}`);
});

// --- `#375 read-aloud-latency`: one preparation at a time, and a tap takes its place ---------------

/** A channel of three threads' worth of rows in Read, with every preparation held until released. */
async function heldThreadPreparations() {
  const page = newPage();
  const data = threadData();
  data.messages.forEach((m) => { m.content = longMessage(m.id); });
  page.threadingSupported = true;
  page.threads = data.threads;
  const releases = [];
  page.prepareHold = () => new Promise((resolve) => { releases.push(resolve); });
  page.enableAbort();
  await signIn(page);
  await showDiscord(page, data.messages);
  await page.el("channel-view-flat").click();
  await readButton(page).click();
  await page.settle();
  return { page, data, releases };
}

test("THE THREADS ON SCREEN ARE PREPARED ONE AFTER ANOTHER, never all at once", async () => {
  // The owner's lag on 2026-10-19: ten preparations for one channel in flight together, each 46 to
  // 53 s, because one upstream slot served them in turn. One at a time, each is answered as fast as
  // that slot allows, and the first rows are ready while the last are still being asked for.
  const { page, data, releases } = await heldThreadPreparations();
  assert.equal(page.prepareCalls.length, 1, "the threads were not prepared one at a time");
  assert.equal(page.maxPreparesInFlight, 1);
  for (let answered = 1; answered < 3; answered += 1) {
    releases[answered - 1]();
    await page.settle();
    assert.equal(page.prepareCalls.length, answered + 1, `thread ${answered + 1} was never asked for`);
    assert.equal(page.maxPreparesInFlight, 1, "two preparations were in flight at once");
  }
  releases[2]();
  await page.settle();
  assert.deepEqual(page.prepareCalls.map((call) => [call.thread_id || null, call.ids]).sort(), [
    [null, ["200"]], [data.threads[0].id, ["201", "202"]], [data.threads[1].id, ["203", "204"]],
  ].sort());
});

test("a tap that needs preparing CANCELS the batch in flight, reads at once, and the batch resumes", async () => {
  const { page, data, releases } = await heldThreadPreparations();
  assert.equal(page.prepareCalls.length, 1);
  const batchSignal = page.prepareSignals[0];
  assert.ok(batchSignal, "the batch cannot be cancelled");
  // The tap's own preparation is answered at once; only the batch is held.
  page.prepareHold = (asked) => (asked.ids.includes("203") && asked.ids.length === 1
    ? null : new Promise((resolve) => { releases.push(resolve); }));
  const row = [...page.el("discord-log").children].find((li) => li.getAttribute("data-id") === "203");
  await row.dispatch("click", {});
  await page.settle();
  assert.equal(batchSignal.aborted, true, "the batch's request was left ahead of the tap's on the server");
  const tapCall = page.prepareCalls.find((call) => call.ids.length === 1 && call.ids[0] === "203");
  assert.ok(tapCall, `the tap did not prepare its own row: ${JSON.stringify(page.prepareCalls)}`);
  assert.equal(tapCall.thread_id, data.threads[1].id);
  assert.equal(page.players.length, 1, "the read waited for the batch");
  assert.match(page.players[0].url, /ticket-for-203\?selection=tap-/);
  // ...and the batch starts again behind the read, without the row the tap prepared.
  const after = page.prepareCalls.slice(page.prepareCalls.indexOf(tapCall) + 1);
  assert.equal(after.length, 1, "the batch did not resume, or resumed more than once");
  assert.ok(after[0].ids.length > 0 && !after[0].ids.includes("203"),
    `the resumed batch asked again for the row the tap prepared: ${JSON.stringify(after[0])}`);
  assert.ok(page.maxPreparesInFlight <= 2, "more than the tap's and one batch request were in flight");
});

test("a batch cancelled while it was being answered keeps nothing it was answered with", async () => {
  // Changing the pace forgets every ticket; a batch already in flight at the old pace must not put
  // them back.
  const { page, releases } = await heldThreadPreparations();
  const first = page.prepareSignals[0];
  page.prepareHold = null;
  page.el("read-speed-box").value = "150";
  await page.el("read-speed-box").dispatch("change");
  await page.settle();
  assert.equal(first.aborted, true, "the batch at the old pace was left running");
  releases[0]();
  await page.settle();
  const speeds = page.prepareCalls.slice(1).map((call) => call.speed);
  assert.ok(speeds.length > 0 && speeds.every((speed) => speed === 1.5), `re-prepared at ${speeds}`);
});

test("a failed standalone send keeps its text in a retryable row and its composer usable", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = errorResponse(502, "chat_error", "Google Chat returned HTTP 502");
  page.el("channel-compose-text").value = "do not lose this";
  await page.el("channel-send").click();
  assert.equal(page.el("channel-compose-text").value, "");
  assert.match(outgoingRows(page)[0].text(), /do not lose this/);
  assert.match(outgoingRows(page)[0].text(), /Google Chat returned HTTP 502/);
  assert.equal(savedOutgoing(page)[0].remaining, "do not lose this");
  assert.ok(outgoingRetry(outgoingRows(page)[0]));
  assert.equal(page.el("channel-send").disabled, false);
  assert.equal(page.el("channel-compose-text").disabled, false);
});

test("an expired older-thread cursor starts a fresh snapshot and makes older paging usable again", async () => {
  const page = await mainOnlyThreadPage();
  let snapshots = 0;
  page.timeline = async (path) => {
    const before = new URL(path, "http://fixture.test").searchParams.get("before");
    if (before === "expired:cursor") {
      return json(400, { error: "refused", detail: "this timeline cursor has expired; refresh the channel" });
    }
    if (!before) snapshots += 1;
    return json(200, timelineAnswer({
      view: "thread", messages: before ? [page.messages[1]] : [page.messages[2]],
      thread: page.threads[0], has_threads: true, has_more: !before,
      next_before: before ? null : snapshots === 1 ? "expired:cursor" : "fresh:cursor",
    }));
  };
  await threadBadge(page.el("discord-log").children[1]).click();
  await page.el("load-older").click();
  assert.equal(snapshots, 2, "the expired snapshot was never replaced");
  assert.match(page.el("status").textContent, /snapshot expired.*Refreshed/);
  assert.equal(page.el("error").hidden, true);
  assert.equal(page.el("load-older").disabled, false);
  await page.el("load-older").click();
  assert.equal(new URL(page.timelineCalls.at(-1), "http://fixture.test").searchParams.get("before"), "fresh:cursor");
  assert.deepEqual(page.el("discord-log").children.map((row) => row.getAttribute("data-id")), ["201", "202"]);
  assert.equal(page.el("load-older").hidden, true);
});

test("background polling keeps walked thread history but an explicit refresh replaces its snapshot", async () => {
  const page = await threadPage();
  let snapshots = 0;
  page.timeline = async (path) => {
    const before = new URL(path, "http://fixture.test").searchParams.get("before");
    if (!before) snapshots += 1;
    return json(200, timelineAnswer({
      view: "thread", messages: [before ? page.messages[1] : page.messages[2]],
      thread: page.threads[0], has_threads: true, has_more: true,
      next_before: before ? "walked:cursor" : `fresh:${snapshots}`,
    }));
  };
  await threadBadge(page.el("discord-log").children[1]).click();
  await page.el("load-older").click();
  assert.equal(page.el("discord-log").children.length, 2);
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  assert.equal(page.el("discord-log").children.length, 2, "polling threw away the walked history");
  page.el("scroll-area").scrollTop = 0;
  await pullDown(page, PULL_ARM_PX);
  assert.equal(page.el("discord-log").children.length, 1, "explicit refresh retained the old snapshot");
  const fresh = `fresh:${snapshots}`;
  await page.el("load-older").click();
  assert.equal(new URL(page.timelineCalls.at(-1), "http://fixture.test").searchParams.get("before"), fresh);
});

test("a failed fetch from a context already left cannot raise an error over the new view", async () => {
  const page = await threadPage();
  const standard = page.timeline;
  let finish;
  page.timeline = (path) => path.includes("view=threads")
    ? new Promise((resolve) => { finish = resolve; }) : standard(path);
  const pending = page.el("channel-view-threads").click();
  await page.settle();
  await page.el("channel-view-flat").click();
  finish(json(502, { error: "chat_error", detail: "the abandoned request failed" }));
  await pending;
  assert.equal(page.el("channel-view-flat").getAttribute("aria-pressed"), "true");
  assert.equal(page.el("discord-log").children.length, 5);
  assert.equal(page.el("error").hidden, true);
});

test("Send shows a local message and clears the composer before the network answers", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  assertMarkupContains("pane-discord", "outgoing-log");
  assert.ok(markupPlace("outgoing-log").at > markupPlace("discord-log").at);
  assert.ok(markupPlace("outgoing-log").at < markupPlace("channel-composer").at);
  const finish = delayedReply(page);
  await page.el("channel-compose-text").setValue("the first message");
  const submitted = page.el("channel-send").click();
  // No yielding or network completion: this is the responsiveness the user actually experiences.
  assert.equal(page.el("channel-compose-text").value, "");
  assert.equal(outgoingRows(page).length, 1);
  assert.match(outgoingRows(page)[0].text(), /the first message/);
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "sending");
  assert.ok(outgoingSpinner(outgoingRows(page)[0]), "an in-flight message has no progress indicator");
  assert.equal(page.el("channel-send").disabled, false);
  assert.equal(page.el("channel-compose-text").disabled, false);
  await page.el("channel-compose-text").setValue("a newer draft");
  finish(json(200, { posted: message({ id: "9101", content: "the first message" }) }));
  await submitted;
  await page.settle();
  assert.equal(page.el("channel-compose-text").value, "a newer draft",
    "the first response erased text typed while it was on the network");
  assert.equal(outgoingRows(page).length, 0, "the confirmed message still has a second local bubble");
  assert.equal(shownIds(page).filter((id) => id === "9101").length, 1);
});

test("Send on an explicit reply closes its editor immediately and never clears a later reply draft", async () => {
  const page = newPage();
  await signIn(page);
  const messages = [message({ id: "9110", content: "question" })];
  await openReplyOn(page, messages);
  const finish = delayedReply(page);
  await page.el("reply-text").setValue("my answer");
  const submitted = page.el("reply-send").click();
  assert.equal(page.screen(), "main", "reply stayed open waiting for the provider");
  assert.equal(page.el("reply-text").value, "");
  assert.match(outgoingRows(page)[0].text(), /my answer/);
  assert.equal(savedOutgoing(page)[0].replyTo, "9110");
  await openReplyOn(page, messages);
  await page.el("reply-text").setValue("another thought on the same question");
  finish(json(200, { posted: message({ id: "9111", content: "my answer", reply_to: "9110" }) }));
  await submitted;
  await page.settle();
  assert.ok(replyInStream(page), "a completed earlier send closed the newer editor");
  assert.equal(page.el("reply-text").value, "another thought on the same question");
  assert.equal(JSON.parse(page.storage.get("vibe-talk.voice.drafts"))["9110"],
    "another thought on the same question");
});

test("a rejected send stops spinning, keeps its text, and allows an explicit retry", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = errorResponse(403, "refused", "You cannot post to this channel.");
  await page.el("channel-compose-text").setValue("retained after refusal");
  await page.el("channel-send").click();
  await page.settle();
  const failed = outgoingRows(page)[0];
  assert.equal(failed.getAttribute("data-send-state"), "failed");
  assert.equal(outgoingSpinner(failed), undefined);
  assert.match(failed.text(), /You cannot post to this channel/);
  assert.equal(savedOutgoing(page)[0].remaining, "retained after refusal");
  assert.equal(page.el("channel-compose-text").value, "");
  assert.ok(outgoingRetry(failed));
  page.replyResponse = async () => json(200, {
    posted: message({ id: "9121", content: "retained after refusal" }),
  });
  await outgoingRetry(failed).click();
  await page.settle();
  assert.deepEqual(page.repliesPosted.map((call) => sentWithoutKey(call.body)), [
    { text: "retained after refusal" }, { text: "retained after refusal" },
  ]);
  assert.equal(outgoingRows(page).length, 0);
});

test("transport failure says delivery is unconfirmed and never automatically retries", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = async () => { throw new TypeError("Failed to fetch"); };
  await page.el("channel-compose-text").setValue("possibly received upstream");
  await page.el("channel-send").click();
  await page.settle();
  const pending = outgoingRows(page)[0];
  assert.equal(pending.getAttribute("data-send-state"), "unconfirmed");
  assert.equal(outgoingSpinner(pending), undefined);
  assert.match(pending.text(), /check.*history|history.*check/i);
  assert.match(pending.text(), /possibly received upstream/);
  assert.ok(outgoingRetry(pending));
  await reReadChannel(page);
  await page.settle();
  assert.equal(page.repliesPosted.length, 1, "a poll retried an ambiguous delivery");
  assert.equal(outgoingRows(page).length, 1, "a poll lost the only local record of the send");
});

test("partial delivery with zero confirmed parts still requires a deliberate retry", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 0, unsent: "maybe already posted",
    detail: "Google Chat returned HTTP 502",
  });
  await page.el("channel-compose-text").setValue("maybe already posted");
  await page.el("channel-send").click();
  await page.settle();
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "unconfirmed");
  assert.match(outgoingRows(page)[0].text(), /check.*history|history.*check/i);
  assert.equal(savedOutgoing(page)[0].remaining, "maybe already posted");
  assert.equal(page.repliesPosted.length, 1);
});

test("a pending send survives channel refresh, navigation and reload without replay", async () => {
  const page = newPage();
  const other = { id: "2220000000000000002", label: "other", writable: true };
  page.channels.push(other);
  await signIn(page);
  await showDiscord(page, []);
  const finish = delayedReply(page);
  await page.el("channel-compose-text").setValue("still waiting for confirmation");
  const submitted = page.el("channel-send").click();
  await page.settle();
  const localId = outgoingRows(page)[0].getAttribute("data-outgoing-id");
  await reReadChannel(page);
  assert.equal(outgoingRows(page)[0].getAttribute("data-outgoing-id"), localId);
  page.el("discord-channel").value = other.id;
  await page.el("discord-channel").dispatch("change");
  assert.equal(outgoingRows(page).length, 0, "channel A's local message leaked into channel B");
  page.el("discord-channel").value = CHANNEL.id;
  await page.el("discord-channel").dispatch("change");
  assert.equal(outgoingRows(page)[0].getAttribute("data-outgoing-id"), localId);
  // A reload loses the browser's knowledge of the in-flight request. It must not pretend it
  // knows whether the provider accepted it, and must not create a second POST on its own.
  const again = reloadWith(new Map(page.storage), [], (p) => p.channels.push(other));
  await signIn(again);
  await reopenedChannel(again);
  assert.equal(again.repliesPosted.length, 0);
  assert.equal(outgoingRows(again)[0].getAttribute("data-outgoing-id"), localId);
  assert.equal(outgoingRows(again)[0].getAttribute("data-send-state"), "unconfirmed");
  assert.match(outgoingRows(again)[0].text(), /still waiting for confirmation/);
  finish(json(200, { posted: message({ id: "9141", content: "still waiting for confirmation" }) }));
  await submitted;
});

test("retry retains its original channel, thread and reply target after navigation and reload", async () => {
  // From Main, picked over the default All (`#189 restore-ui-state`): All would show the thread's
  // unsent reply after leaving the thread, and Main is the view that must not.
  const page = await threadPage("main");
  await threadBadge(page.el("discord-log").children[1]).click();
  await replyButton(page.el("discord-log").children[1]).click();
  page.replyResponse = errorResponse(403, "refused", "permission changed");
  await page.el("reply-text").setValue("answer only inside this thread");
  await page.el("reply-send").click();
  await page.settle();
  const original = page.repliesPosted[0];
  await page.el("thread-back").click();
  assert.equal(outgoingRows(page).length, 0);
  const again = reloadWith(new Map(page.storage), page.messages, (p) => {
    p.threadingSupported = true;
    p.threads = page.threads;
  });
  await signIn(again);
  await reopenedChannel(again);
  await threadBadge(again.el("discord-log").children[1]).click();
  await outgoingRetry(outgoingRows(again)[0]).click();
  await again.settle();
  assert.equal(again.repliesPosted[0].path, original.path);
  assert.deepEqual(sentWithoutKey(again.repliesPosted[0].body), {
    text: "answer only inside this thread", thread_id: page.threads[0].id, reply_to: "202",
  });
});

// --- messages read automatically -----------------------------------------------------------------
//
// `#196 auto-read-noise`. The server judges every message it serves against the owner's noise rules
// and says so on the message as `noise: true`. Everything below drives the real page against that
// flag. The fixture deliberately sets the flag rather than matching text, because the page must not
// match text either: one predicate, and it is the server's.

/** The owner's evidence in miniature: a question, the agent's placeholder, and the answer. */
const placeholderChannel = () => [
  message({ id: "9400000000000000001", author: "alice", author_is_bot: false, content: "Is the nightly build green?" }),
  message({ id: "9400000000000000002", content: "_Working…_", noise: true }),
  message({ id: "9400000000000000003", content: "Yes. It went green after the fixture was renewed." }),
];

const notNoiseButton = (li) => li.descendants().find((node) => node.className === "not-noise-button");

test("A PLACEHOLDER THE SERVER READS AUTOMATICALLY IS DIMMED, NOT HIDDEN, AND NOT UNREAD", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, placeholderChannel());
  assert.deepStrictEqual(
    shownIds(page),
    ["9400000000000000001", "9400000000000000002", "9400000000000000003"],
    "the channel view hid a placeholder; hiding is Hide read's job, and a hidden row cannot be rescued"
  );
  assert.deepStrictEqual(
    [...rows].map((li) => li.getAttribute("data-noise")),
    ["false", "true", "false"],
    "the row state disagrees with the server's verdict"
  );
  // Quiet, by the stylesheet's own rule for exactly this attribute: dimmed and drained of colour.
  const quiet = cssBlock('#discord-log li.discord-message[data-noise="true"]');
  assert.match(quiet, /opacity:\s*0?\.\d+/, "a noise row is drawn at full strength");
  assert.match(quiet, /grayscale\(1\)/, "a noise row keeps its colour, so it reads as unread");
  // ...and it is not ALSO drawn as something the reader declared, which it is not.
  assert.equal(rows[1].getAttribute("data-archived"), "false");
  // The page decided nothing from the text: a message that merely LOOKS like the placeholder but
  // that the server did not flag is an ordinary row.
  const lookalike = newPage();
  await signIn(lookalike);
  const [plain] = await showDiscord(lookalike, [message({ id: "9400000000000000009", content: "_Working…_" })]);
  assert.equal(plain.getAttribute("data-noise"), "false", "the page matched text on its own");
});

test("Hide read leaves a placeholder out of the to-do list, and an arriving one never joins it", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, placeholderChannel());
  await turnTodoOn(page);
  assert.deepStrictEqual(
    shownIds(page),
    ["9400000000000000001", "9400000000000000003"],
    "a placeholder is in the list of things to deal with"
  );
  // A placeholder arriving on the stream while the filter is on is already read.
  await deliver(page, page.stream(), sseMessage(message({ id: "9400000000000000004", content: "Working...", noise: true })));
  assert.deepStrictEqual(
    shownIds(page),
    ["9400000000000000001", "9400000000000000003"],
    "an arriving placeholder joined the to-do list"
  );
  assert.match(page.el("clear-backlog").textContent, /\(2\)/, "the backlog count includes a placeholder");
  // ...while a real arrival does, so the guard is about the verdict and not about arriving.
  await deliver(page, page.stream(), sseMessage(message({ id: "9400000000000000005", content: "One more question." })));
  assert.deepStrictEqual(shownIds(page).slice(-1), ["9400000000000000005"]);
});

test("Clear the backlog clears exactly the messages it counted, and never records a placeholder", async () => {
  // The control counts the rows the to-do list shows, which leave the placeholder out; the server
  // resolves `through` against its whole window, which holds it. When the server swept it up too,
  // the page asked "clear 2" and reported 3, the undo restored 3, and the placeholder was recorded
  // as dealt with — a record that outlives the rule and would hide it if it were edited into the
  // answer.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, placeholderChannel());
  await turnTodoOn(page);
  await page.el("clear-backlog").click();
  await page.settle();
  assert.match(page.el("status").textContent, /clear 2 /, "the premise is a confirmation of two");
  await page.el("clear-backlog").click();
  await page.settle();
  assert.equal(page.dismissCalls[0].through, "9400000000000000003", "the clear stopped short");
  assert.match(
    page.el("status").textContent,
    /^2 messages marked as dealt with/,
    `asked to clear 2, the page reported "${page.el("status").textContent}"`
  );
  assert.ok(!page.dealtWith.has("9400000000000000002"), "a placeholder was recorded as dealt with");
});

test("a threaded channel hides placeholders under Hide read and dims them otherwise", async () => {
  const page = newPage();
  page.threadingSupported = true;
  await signIn(page);
  await showDiscord(page, placeholderChannel());
  assert.equal(shownIds(page).length, 3, "the threaded channel view dropped a placeholder");
  assert.equal(page.el("discord-log").children[1].getAttribute("data-noise"), "true");
  await turnTodoOn(page);
  assert.deepStrictEqual(
    shownIds(page),
    ["9400000000000000001", "9400000000000000003"],
    "Hide read kept a placeholder in a threaded channel"
  );
  await page.el("todo-filter").click();
  await page.settle();
  assert.equal(shownIds(page).length, 3, "turning Hide read off did not bring the placeholder back");
});

test("a placeholder arriving during a call is shown but never relayed to the agent", async () => {
  // The relay is "read new" during a call. Announcing "a new message: Working…" while the owner
  // drives is the noise he asked to be rid of.
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "full");
  await deliver(page, stream, sseMessage(message({ id: "9400000000000000010", content: "_Working…_", noise: true })));
  await speakRelay(page);
  assert.deepStrictEqual(relayed(page), [], "a placeholder was read into the call");
  assert.equal(
    shownIds(page).filter((id) => id === "9400000000000000010").length,
    1,
    "the placeholder belongs in the channel view; it is the RELAY that is suppressed"
  );
  // The control: the answer that follows IS relayed.
  await deliver(page, stream, sseMessage(message({ id: "9400000000000000011", content: "The deploy finished cleanly." })));
  await speakRelay(page);
  const sent = relayed(page);
  assert.equal(sent.length, 1, "the real message after the placeholder was not relayed");
  assert.match(sent[0].text, /deploy finished cleanly/);
  assert.doesNotMatch(sent[0].text, /Working/, "the placeholder rode along with the real message");
});

test("read-aloud does not prepare a placeholder, and a tap on one still reads it", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await inReadingMode(page, placeholderChannel());
  const prepared = page.prepareCalls.flatMap((call) => call.ids || []);
  assert.ok(prepared.includes("9400000000000000003"), "read-aloud prepared nothing at all");
  assert.ok(
    !prepared.includes("9400000000000000002"),
    "read-aloud spent a preparation on a placeholder nobody is going to ask to hear"
  );
  // An explicit tap is an explicit request: skipping it would be a control that does nothing. The
  // tap prepares that one row itself and reads it. `#191 read-aloud-first-lag`.
  const preparedBefore = page.prepareCalls.length;
  await rows[1].dispatch("click", {});
  await page.settle();
  assert.deepStrictEqual(
    page.prepareCalls[preparedBefore] && page.prepareCalls[preparedBefore].ids,
    ["9400000000000000002"],
    "the tap did not prepare the placeholder it asked for"
  );
  assert.match(
    (page.players[0] || {}).url || "",
    /^\/api\/v1\/speech\/ticket-for-9400000000000000002\?selection=/,
    "a tapped placeholder was not read"
  );
});

test("a placeholder and the answer seconds after it stay two rows", async () => {
  // Combined, the row would be neither read nor quiet, and "Working…" would head the real answer.
  const page = newPage();
  await signIn(page);
  const at = Date.parse("2026-10-04T10:00:00.000Z");
  const rows = await showDiscord(page, [
    message({ id: "9400000000000000020", content: "_Working…_", noise: true, timestamp: new Date(at).toISOString() }),
    message({ id: "9400000000000000021", content: "Done: the cache key now includes the region.", timestamp: new Date(at + 2000).toISOString() }),
  ]);
  assert.equal(rows.length, 2, "a placeholder was combined into the answer that followed it");
  assert.equal(rows[0].getAttribute("data-noise"), "true");
  assert.equal(rows[1].getAttribute("data-noise"), "false");
});

test("NOT NOISE RESCUES ONE MESSAGE: THE SERVER IS TOLD, AND THE ROW IS ORDINARY AT ONCE", async () => {
  const page = newPage();
  await signIn(page);
  const rows = await showDiscord(page, placeholderChannel());
  assert.equal(notNoiseButton(rows[0]).hidden, true, "Not noise is offered on an ordinary row");
  assert.equal(notNoiseButton(rows[1]).hidden, false, "a noise row offers no way to rescue it");

  await notNoiseButton(rows[1]).click();
  await page.settle();
  assert.deepStrictEqual(
    page.notNoiseCalls,
    [{ messages: ["9400000000000000002"] }],
    "the rescue named the wrong message, or was not sent"
  );
  const rescued = page.el("discord-log").children[1];
  assert.equal(rescued.getAttribute("data-noise"), "false", "the rescued row is still drawn as noise");
  assert.equal(notNoiseButton(rescued).hidden, true, "Not noise is still offered on a rescued row");
  assert.match(page.el("status").textContent, /no longer read automatically/);
  // ...and the next read agrees, because the server recorded it rather than this page remembering.
  await reReadChannel(page);
  assert.equal(page.el("discord-log").children[1].getAttribute("data-noise"), "false");
  // With Hide read on it is now something to deal with.
  await turnTodoOn(page);
  assert.ok(shownIds(page).includes("9400000000000000002"), "the rescued message is still hidden");
});

test("SETTINGS LISTS THE SERVER'S RULES, QUOTES HOW THEY MATCH, AND COUNTS WHAT THEY CATCH", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [...placeholderChannel(), message({ id: "9400000000000000030", content: "Thinking…" })]);
  await page.el("open-settings").click();
  await page.settle();
  assert.equal(page.el("noise-editor").hidden, false, "the editor is hidden from a server that sends the rules");
  assert.equal(page.el("noise-matching").textContent, NOISE_MATCHING, "the server's sentence was not quoted");
  const ruleTexts = () =>
    page.el("noise-rule-list").children.map((li) =>
      li.descendants().find((node) => node.className === "noise-rule-text").textContent
    );
  assert.deepStrictEqual(ruleTexts(), ["Working…"]);
  assert.equal(page.el("noise-count").textContent, "Matches 1 of the 4 loaded messages.");

  // ADD: the whole list goes to the server, the editor redraws from its answer, and the channel is
  // re-read so the count is the server's verdict under the new list, not a guess made here.
  page.judgeNoise = (m, rules) => rules.some((rule) => String(m.content).includes(rule.replace(/^_|_$/g, "")));
  const readsBefore = page.pageReads;
  page.el("noise-rule-new").value = "  Thinking…  ";
  await page.el("add-noise-rule").click();
  await page.settle();
  assert.deepStrictEqual(page.noiseRuleCalls.at(-1), { rules: ["Working…", "  Thinking…  ".trim()] });
  assert.deepStrictEqual(ruleTexts(), ["Working…", "Thinking…"]);
  assert.equal(page.el("noise-rule-new").value, "", "the field still holds what was added");
  assert.ok(page.pageReads > readsBefore, "the channel was not re-read under the new rules");
  assert.equal(page.el("noise-count").textContent, "Matches 2 of the 4 loaded messages.");
  assert.match(page.el("noise-state").textContent, /Added/);

  // REMOVE: the list without that rule.
  const removeFirst = page.el("noise-rule-list").children[0].descendants()
    .find((node) => String(node.className).includes("noise-rule-remove"));
  await removeFirst.click();
  await page.settle();
  assert.deepStrictEqual(page.noiseRuleCalls.at(-1), { rules: ["Thinking…"] });
  assert.deepStrictEqual(ruleTexts(), ["Thinking…"]);
  assert.equal(page.el("noise-count").textContent, "Matches 1 of the 4 loaded messages.");

  // A blank add sends nothing and says why.
  const calls = page.noiseRuleCalls.length;
  page.el("noise-rule-new").value = "   ";
  await page.el("add-noise-rule").click();
  await page.settle();
  assert.equal(page.noiseRuleCalls.length, calls, "a blank rule reached the server");
  assert.match(page.el("noise-state").textContent, /Type the message/);
});

test("a refused save leaves the list as the server holds it and says why", async () => {
  const page = newPage();
  page.noiseStatus = 503;
  await signIn(page);
  await page.el("open-settings").click();
  page.el("noise-rule-new").value = "Thinking…";
  await page.el("add-noise-rule").click();
  await page.settle();
  assert.equal(page.el("error").hidden, false, "a refused save said nothing");
  assert.match(page.el("error").textContent, /storage_not_configured/);
  assert.equal(page.el("noise-rule-list").children.length, 1, "the editor showed a rule the server did not keep");
});

test("an older server's missing rules hide the editor rather than show an empty list", async () => {
  const page = newPage();
  page.noiseRules = undefined;
  await signIn(page);
  await page.el("open-settings").click();
  assert.equal(page.el("noise-editor").hidden, true, "an editor for a list the server never sent");
  assert.match(page.el("noise-state").textContent, /does not say/);
});

test("the Mark-read automatically group explains itself in Help, where its ? leads", () => {
  // `#347 settings-prose`: "Mark-read automatically", the owner's name for "Read automatically".
  const group = settingsGroup("Mark-read automatically");
  assert.match(group, /data-help="auto-read"/, "the group offers no way to reach the detail");
  const help = helpEntry("auto-read");
  assert.match(help, /whole text/, "Help does not say a rule must match the whole message");
  assert.match(help, /<code>\*<\/code>/, "Help does not say how a prefix rule is written");
  assert.match(help, /Not noise/, "Help does not say how to rescue a message a rule caught");
  assert.match(help, /voice\s+agent\s+cannot\s+change/, "Help does not say who may change the rules");
});
