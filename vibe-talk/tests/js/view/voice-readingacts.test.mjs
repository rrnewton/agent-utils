// The pace popover down, and read-aloud reading mode on, off and left: their owner,
// over fakes (`#624 voice-page-modularity`).

import assert from "node:assert/strict";
import { test } from "node:test";
import { createReadingModeActs } from "../../../web/src/view/voice/readingacts.ts";

test("reading mode turns on, prepares speech and says so; Stop leaves it", () => {
  const record = [];
  const owner = {
    readingMode: false,
    readAloudPlayback: "server",
    readAloudLabel: "Read aloud",
    stopReading: () => record.push("stop"),
    setReadState: (state) => record.push(`state=${state}`),
    browserSpeechProblem: () => "",
    prepareSpeech: () => record.push("prepare"),
    forgetPreparedSpeech: () => record.push("forget"),
  };
  const readButton = { hidden: false, focus: () => record.push("focus-read") };
  const stopButton = {};
  globalThis.document = { activeElement: stopButton };
  const acts = createReadingModeActs({
    el: (id) =>
      id === "read-aloud" ? readButton : id === "read-aloud-stop" ? stopButton :
      { hidden: false, setAttribute: (name, value) => record.push(`${id} ${name}=${value}`) },
    scheduleBackSync: () => record.push("back"),
    readAloudOwner: () => owner,
    clearError: () => record.push("clear"),
    showError: (text) => record.push(`error=${text}`),
    guardQuietly: (fn) => fn,
    setStatus: (text) => record.push(`status=${text}`),
    renderControls: () => record.push("controls"),
    renderChannelRows: () => record.push("rows"),
  });
  acts.setReadingMode(true);
  assert.equal(owner.readingMode, true);
  assert.deepEqual(record, [
    "state=ready",
    "clear",
    "prepare",
    "status=reading mode: tap a message to hear it, and it archives when it finishes.",
    "controls",
    "rows",
  ]);
  acts.leaveReadingMode();
  assert.equal(owner.readingMode, false);
  assert.equal(record.at(-1), "focus-read");
  acts.closeSpeedPopover();
  assert.equal(record.at(-1), "back");
});
