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
