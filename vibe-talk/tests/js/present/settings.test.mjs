// Tests for web/src/present/settings/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The lists are held to the markup, and
// the page's folds to them, in ../page/settings.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** What Settings and Help fold, and what their one button says (web/src/present/settings). */
const SETTINGS = await moduleForTests("src/present/settings/index.ts");

test("`#624 voice-page-modularity`: the folds, the help topics and the one button's words, from the module alone", () => {
  const { SETTINGS_FOLDS, FOLDS_HOLDING_DISMISSED, HELP_TOPICS, foldAllWords, helpEntrySlugs } = SETTINGS;
  assert.equal(SETTINGS_FOLDS.length, 34);
  assert.equal(new Set(SETTINGS_FOLDS).size, SETTINGS_FOLDS.length, "a fold is listed twice");
  for (const slug of FOLDS_HOLDING_DISMISSED) assert.ok(SETTINGS_FOLDS.includes(slug), `${slug} holds the pop-out and does not fold`);
  assert.equal(new Set(HELP_TOPICS).size, HELP_TOPICS.length, "a help topic is listed twice");
  for (const list of [SETTINGS_FOLDS, FOLDS_HOLDING_DISMISSED, HELP_TOPICS]) {
    assert.ok(Object.isFrozen(list), "a list the page reads can be changed");
    for (const slug of list) assert.match(slug, /^[a-z]+(?:-[a-z]+)*$/);
  }
  // The two entries read from the screen itself come first, then every topic, in order; a fresh list each time.
  assert.deepStrictEqual(helpEntrySlugs(), ["controls", "channel", ...HELP_TOPICS]);
  assert.notEqual(helpEntrySlugs(), helpEntrySlugs());
  assert.equal(foldAllWords(true), "Expand all");
  assert.equal(foldAllWords(false), "Fold all");
});

test("`#602 history-settings-and-limits`: what the channel box says about the channel's history, and when it reads it again", () => {
  const { historyChoiceOf, historyDefaultWords, historyPrioritizeOffered, historyPrioritizedWords, historyReadFailure,
    historyRecheckWait, historyWaitingWords, CHANNEL_HISTORY_RECHECK_MS, CHANNEL_HISTORY_QUEUED_RECHECK_MS } = SETTINGS;
  const view = (fields = {}) => ({ mode: "recent", server_mode: "recent", complete: false, queue_position: 3, ...fields });
  assert.equal(historyChoiceOf(view()), "default");
  assert.equal(historyChoiceOf(view({ mode_override: "full" })), "full");
  assert.equal(historyDefaultWords(view({ server_mode: "full" })), "Server default (whole history)");
  assert.equal(historyDefaultWords(view({ server_mode: "off" })), "Server default (none)");
  assert.equal(historyDefaultWords(view({ server_mode: "later" })), "Server default (later)", "a mode it has no words for is named as it is");
  assert.equal(historyPrioritizeOffered(view(), true), true);
  for (const [why, shown, writable] of [["no answer", null, true], ["read-only token", view(), false], ["off", view({ mode: "off" }), true],
    ["complete", view({ complete: true }), true], ["already first", view({ queue_position: 0 }), true]]) {
    assert.equal(historyPrioritizeOffered(shown, writable), false, why);
  }
  assert.equal(historyRecheckWait(view({ queue_position: 0 })), CHANNEL_HISTORY_QUEUED_RECHECK_MS);
  assert.equal(historyRecheckWait(view()), CHANNEL_HISTORY_RECHECK_MS);
  assert.equal(historyRecheckWait(null), CHANNEL_HISTORY_RECHECK_MS);
  assert.ok(CHANNEL_HISTORY_QUEUED_RECHECK_MS < CHANNEL_HISTORY_RECHECK_MS);
  assert.equal(historyWaitingWords(""), "Checking with the server…");
  assert.equal(historyWaitingWords("down"), "down");
  assert.equal(historyReadFailure("timed out"), "Could not read this channel's history: timed out");
  assert.equal(historyPrioritizedWords(view({ complete: true })), "Nothing older to read: every message is saved.");
  assert.equal(historyPrioritizedWords(view()), "Its older messages are read next, within the chat service's limits.");
});

test("`#253 upstream-clear-polish`, `#349 channel-settings-all`: what a waiting press came to, and what All channels says", () => {
  const { upstreamClearAttemptKey, upstreamClearPressSettled, upstreamClearAllCommon, upstreamClearAllSentence,
    upstreamClearAllSavedWords, upstreamClearPushingWords, upstreamClearUnansweredWords, upstreamClearReadFailure,
    UPSTREAM_CLEAR_RECHECK_MS, UPSTREAM_CLEAR_RECHECK_LIMIT_MS } = SETTINGS;
  const attempt = (fields = {}) => ({ at_ms: 10, message_id: "m1", message_at_ms: 5, result: "pushed", ...fields });
  assert.equal(upstreamClearAttemptKey(null), "");
  assert.equal(upstreamClearAttemptKey(attempt()), "10 m1 pushed");
  const press = { mode: "on", messageAtMs: 5, before: upstreamClearAttemptKey(attempt()) };
  const state = (fields = {}) => ({ mode: "on", last: attempt(), ...fields });
  assert.equal(upstreamClearPressSettled(press, state()), null, "the attempt recorded at the answer answers nothing");
  assert.equal(upstreamClearPressSettled(press, state({ mode: "off" })).outcome, "changed");
  assert.equal(upstreamClearPressSettled(press, state({ last: attempt({ at_ms: 11 }) })).outcome, "pushed");
  assert.equal(upstreamClearPressSettled(press, state({ last: attempt({ at_ms: 11, result: "skipped" }) })).outcome, "skipped");
  assert.equal(upstreamClearPressSettled(press, state({ last: attempt({ at_ms: 11, message_at_ms: 4, result: "failed" }) })).outcome,
    "retry_waiting", "a newer failure short of the message waits for the retry");
  assert.equal(upstreamClearPressSettled(press, state({ unavailable: "gone" })).outcome, "unavailable");
  assert.equal(upstreamClearPressSettled({ ...press, messageAtMs: null }, state({ last: attempt({ at_ms: 11, message_at_ms: 1 }) })).outcome, "pushed");
  assert.equal(Math.floor(UPSTREAM_CLEAR_RECHECK_LIMIT_MS / UPSTREAM_CLEAR_RECHECK_MS) - 1, 6, "asked seven times, counting the first");
  const held = (modes, failures = []) => ({ ids: Object.keys(modes), views: new Map(Object.entries(modes).filter(([, mode]) => mode).map(([id, mode]) => [id, { mode }])),
    failures: new Map(failures) });
  const nameOf = (id) => `#${id}`;
  assert.equal(upstreamClearAllCommon(null), undefined);
  assert.equal(upstreamClearAllCommon(held({ a: "on", b: null })), undefined);
  assert.equal(upstreamClearAllCommon(held({ a: "on", b: "on" })), "on");
  assert.equal(upstreamClearAllCommon(held({ a: "on", b: "off" })), null);
  assert.equal(upstreamClearAllSentence(null, 2, nameOf), "");
  assert.equal(upstreamClearAllSentence(held({ a: "on", b: "on" }), 2, nameOf), "On in all 2 channels.");
  assert.equal(upstreamClearAllSentence(held({ a: "dry_run" }), 2, nameOf), "Dry run in #a. 1 other channel's chat service cannot clear unread.");
  assert.equal(upstreamClearAllSentence(held({ a: "on", b: "off", c: "on" }), 5, nameOf),
    "Mixed: On in 2, Off in 1. 2 other channels' chat services cannot clear unread.");
  assert.equal(upstreamClearAllSentence(held({ a: "on", b: null }), 2, nameOf), "Checking with the server…");
  assert.equal(upstreamClearAllSentence(held({ a: null, b: null }, [["a", "down"], ["b", "down"]]), 2, nameOf),
    "Could not read this setting for #a (and 1 more): down");
  let asked = 0;
  assert.equal(upstreamClearAllSavedWords([], 2, () => { asked += 1; return "On in all 2 channels."; }), "Saved. On in all 2 channels.");
  assert.equal(upstreamClearAllSavedWords(["#b (refused)"], 2, () => { asked += 1; return ""; }), "Saved in 1 of 2 channels; not in #b (refused).");
  assert.equal(asked, 1, "the status line is worked out only when every channel took the mode");
  assert.equal(upstreamClearPushingWords("dry_run", "Slack"), "Trying it on the newest message in Slack…");
  assert.equal(upstreamClearPushingWords("on", "Slack"), "Marking the newest message read in Slack…");
  assert.match(upstreamClearUnansweredWords("Slack"), /^Slack has not answered yet\./);
  assert.equal(upstreamClearReadFailure("down"), "Could not read this setting: down");
});

test("`#19 channel-browser`, `#371 channel-search-hash`: what the channel browser asks for and says", () => {
  const { directoryEntryTracked, directoryPath, directoryAskingWords, directoryWaitWords, directoryFailureIsFinal,
    directoryFailureSentence, directorySummary, directoryLabel, DIRECTORY_SLOW_MS, CHANNEL_LABEL_MAX } = SETTINGS;
  const known = (id) => id === "42";
  assert.equal(directoryEntryTracked({ tracked: true }, known), true);
  assert.equal(directoryEntryTracked({ channel_id: "42" }, known), true);
  assert.equal(directoryEntryTracked({ channel_id: "43" }, known), false);
  assert.equal(directoryEntryTracked({ channel_id: null }, () => true), false, "no id is no channel, whatever the list says");
  assert.equal(directoryPath("", null, null), "/api/v1/channel-directory?limit=25");
  assert.equal(directoryPath("rel ease", "c/1", "slack"), "/api/v1/channel-directory?limit=25&q=rel%20ease&cursor=c%2F1&provider=slack");
  assert.equal(directoryAskingWords(true, "x"), "Loading more channels…");
  assert.equal(directoryAskingWords(false, "x"), 'Searching for "x"…');
  assert.equal(directoryAskingWords(false, ""), "Loading channels…");
  assert.equal(directoryWaitWords("Loading channels…", 400, "slack"), "Loading channels… 1 s so far.");
  assert.equal(directoryWaitWords("Loading channels…", DIRECTORY_SLOW_MS, "slack"),
    "Loading channels… 3 s so far. Slack can take a while to list its channels; the same search again within two minutes answers at once.");
  assert.deepStrictEqual([401, 403, 501, 500, undefined].map((status) => directoryFailureIsFinal({ status })), [true, true, true, false, false]);
  assert.match(directoryFailureSentence({ code: "channel_directory_denied" }, "slack"), /^Slack does not allow this app to list channels\./);
  assert.match(directoryFailureSentence({ status: 403 }, "slack"), /^This sign-in cannot browse channels/);
  assert.match(directoryFailureSentence({ status: 501 }, "slack"), /^This chat connector cannot list channels\./);
  assert.equal(directoryFailureSentence({ status: 500, message: "boom" }, "slack"), "Could not load channels. boom");
  const shown = (fields) => ({ query: "", entries: [], nextCursor: null, truncated: false, ...fields });
  assert.equal(directorySummary(shown({ query: "x" }), "Slack"), 'No channels match "x".');
  assert.equal(directorySummary(shown({}), "Slack"), "No channels are available in Slack yet.");
  assert.equal(directorySummary(shown({ entries: [1] }), "Slack"), "1 channel shown.");
  assert.equal(directorySummary(shown({ entries: [1, 2], nextCursor: "n" }), "Slack"), "2 channels shown. More are available.");
  assert.equal(directorySummary(shown({ entries: [1, 2], truncated: true }), "Slack"), "2 channels shown. The list stopped early, so search by name to find others.");
  assert.equal(directoryLabel(` ${"é".repeat(CHANNEL_LABEL_MAX + 5)}`).length, CHANNEL_LABEL_MAX - 1, "cut by characters, then trimmed");
});

test("`#348 settings-dynamic-lists`, `#351 settings-polish`: what Recently dismissed says about each send", () => {
  const { dismissedSendMeta, dismissedRowLabel, dismissedListWords, dismissedRestoreLabel, DISMISSED_LABEL_CHARS } = SETTINGS;
  const label = (id) => `#${id}`;
  const held = (entry, ago = 0) => ({ at: Date.now() - ago, entry: { id: "o1", channel: "c1", text: "hello", ...entry } });
  assert.equal(dismissedSendMeta(held({}), label), "#c1 · dismissed just now");
  assert.equal(dismissedSendMeta(held({ replyTo: "m" }, 5 * 60000), label), "#c1 · Reply · dismissed 5m ago");
  assert.equal(dismissedSendMeta(held({ threadId: "t" }, 3 * 3600000), label), "#c1 · Thread · dismissed 3h ago");
  assert.equal(dismissedRowLabel(held({ text: "\n  \nfirst line\nsecond" }), label), "#c1 · dismissed just now: first line");
  assert.ok(dismissedRowLabel(held({ text: "word ".repeat(100) }), label).length < 40 + DISMISSED_LABEL_CHARS, "a row is named by a line");
  assert.equal(dismissedRowLabel(held({ text: "all of it", remaining: "it", postedCount: 1 }), label), "#c1 · dismissed just now: it",
    "what a partial post left unsent");
  assert.match(dismissedListWords(2), /^Messages you dismissed from the outbox unsent, kept on this device for \d+ days \(the newest \d+\)\./);
  assert.match(dismissedListWords(0), /^Nothing dismissed in the last \d+ days\./);
  assert.equal(dismissedRestoreLabel({ text: "bring  it\nback" }), "Put back “bring it back”");
});

test("`#208 persistent-cache`, `#595 cache-full-history`: what the server's message cache group says", () => {
  const { SERVER_CACHE_ASK, SERVER_CACHE_SENDING, serverCacheKeepsWords, serverCacheUnconfirmed, serverHoldsWholeHistory } = SETTINGS;
  const status = (history, fields = {}) => ({ available: true, enabled: true, max_days: 30, history, ...fields });
  assert.equal(serverHoldsWholeHistory(null), false);
  assert.equal(serverHoldsWholeHistory(status({ mode: "full", channels: 2, complete: 2 })), true);
  assert.equal(serverHoldsWholeHistory(status({ mode: "full", channels: 2, complete: 1 })), false, "one channel still gathering");
  assert.equal(serverHoldsWholeHistory(status({ mode: "full", channels: 0, complete: 0 })), false);
  assert.equal(serverHoldsWholeHistory(status({ mode: "recent", channels: 2, complete: 2 })), false);
  assert.equal(serverHoldsWholeHistory(status({ mode: "full", channels: 2, complete: 2 }, { available: false })), false);
  assert.equal(serverCacheKeepsWords(status(null, { max_days: 1 })), "1 day");
  assert.equal(serverCacheKeepsWords(status(null, { max_days: 30 })), "30 days");
  assert.equal(serverCacheKeepsWords(status(null, { max_days: 65535 })), "their whole history");
  assert.deepStrictEqual(Object.keys(SERVER_CACHE_SENDING), ["on", "off", "erase"]);
  assert.equal(SERVER_CACHE_ASK.off.ask("12 messages"), "Turn saving off and erase 12 messages now?");
  assert.equal(SERVER_CACHE_ASK.erase.ask("12 messages"), "Erase 12 messages now? Saving carries on in an empty file.");
  assert.equal(serverCacheUnconfirmed("store_unavailable"), "The server did not confirm that: store_unavailable");
});

test("`#402 who-is-who-channel-box`: the order and the words of the list of who is who", () => {
  const { BUCKET_LABELS, identityOrder, identityNobodyWords, identityChannelWords, identityPickerLabel, IDENTITY_PICK_ONE, IDENTITY_SAVED } = SETTINGS;
  assert.deepStrictEqual(Object.keys(BUCKET_LABELS), ["me", "coder", "human", "bot"]);
  const seen = { b1: { name: "zed", bot: true }, p2: { name: "bea", bot: false }, b2: { name: "ann", bot: true }, p1: { name: "al", bot: false } };
  assert.deepStrictEqual(identityOrder(seen), ["p1", "p2", "b2", "b1"], "people first, then bots, then by name");
  assert.deepStrictEqual(identityOrder({}), []);
  assert.equal(identityNobodyWords("release"), "Nobody yet in release. Read that channel, and everyone who has spoken in it appears here.");
  assert.equal(identityChannelWords("release"), "In release:");
  assert.equal(identityPickerLabel("", "U1"), "what U1 is");
  assert.equal(identityPickerLabel("alice", "U1"), "what alice is");
  assert.equal(IDENTITY_PICK_ONE, "Pick one channel to see who is who.");
  assert.match(IDENTITY_SAVED, /^Saved\./);
});

test("`#369 pairing-link`: what the sign-in screen says about a pairing link", () => {
  const { PAIRING_EXPIRED, pairingRefusedWords, pairingCodeShaped, pairingTransferAddress, PAIRING_UNREACHED, PAIRING_WRONG_LINK, PAIRING_PARTIAL } = SETTINGS;
  assert.equal(pairingRefusedWords(410), PAIRING_EXPIRED, "every refusal reads the same");
  assert.equal(pairingRefusedWords(404), PAIRING_EXPIRED);
  assert.match(pairingRefusedWords(429), /^Too many pairing attempts from this network\./);
  assert.equal(pairingCodeShaped("a".repeat(43)), true);
  assert.equal(pairingCodeShaped("A-_9".repeat(10) + "abc"), true);
  assert.equal(pairingCodeShaped("a".repeat(42)), false);
  assert.equal(pairingCodeShaped("a".repeat(42) + "="), false);
  assert.equal(pairingTransferAddress("https://talk.example", "a b"), "https://talk.example/#pair=a%20b");
  assert.equal(pairingTransferAddress("https://talk.example", null), "https://talk.example/#pair=");
  for (const line of [PAIRING_UNREACHED, PAIRING_WRONG_LINK, PAIRING_PARTIAL]) assert.match(line, /^[A-Z].*\.$/);
});

test("`#55 voice-desktop-app`, `#230 text-size-buttons`: the text size's steps, the reading width's keys, and what Settings says of each", () => {
  const { MSG_SCALE_STEP, msgScaleStepTarget, msgScaleWords, readingWidthWords, GRIP_KEYS } = SETTINGS;
  assert.equal(MSG_SCALE_STEP, 5);
  // From a mark, the next mark; from between two (a hand-edited 83), the next mark that way.
  assert.equal(msgScaleStepTarget(100, 1), 105);
  assert.equal(msgScaleStepTarget(100, -1), 95);
  assert.equal(msgScaleStepTarget(83, 1), 85);
  assert.equal(msgScaleStepTarget(83, -1), 80);
  assert.equal(msgScaleWords(110, true), "Saved — message text at 110% of its usual size.");
  assert.match(msgScaleWords(110, false), /^This browser refused to store the text size.* Message text is at 110% until then\.$/);
  assert.equal(readingWidthWords(72, true), "Saved — a column 72 characters wide.");
  assert.match(readingWidthWords(72, false), /The column is 72 characters wide until then\.$/);
  assert.deepStrictEqual(["ArrowLeft", "ArrowRight", "ArrowDown", "ArrowUp", "PageDown", "PageUp"].map((key) => GRIP_KEYS[key](70)),
    [69, 71, 69, 71, 60, 80]);
  assert.ok(GRIP_KEYS.Home(70) < GRIP_KEYS.End(70));
  assert.equal(GRIP_KEYS.Enter, undefined, "a key the handle does not take is left to the page");
});
