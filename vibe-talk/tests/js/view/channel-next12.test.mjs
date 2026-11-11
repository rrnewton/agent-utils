// The open-in-view choice, the page's quiet guards, and Settings' way out: their
// owners, over fakes (`#624 voice-page-modularity`).

import assert from "node:assert/strict";
import { test } from "node:test";
import { createPageGuards } from "../../../web/src/app/guards.ts";
import { createOpenInView } from "../../../web/src/view/channel/openinview.ts";
import { createScreenShowing } from "../../../web/src/view/shell/screenshowing.ts";

test("the open-in-view choice is kept and drawn, and links redraw per column", () => {
  const record = [];
  const owner = createOpenInView({
    threading: () => true,
    eachColumn: (act) => { record.push("column"); act(); },
    el: (id) => ({ replaceChildren: (...nodes) => record.push(`${id}<=${nodes.length}`) }),
    renderCachedTimeline: () => record.push("timeline"),
    view: () => "threads",
    threadCards: (threads) => { record.push(`cards=${threads}`); return ["card"]; },
    threads: () => "held",
    renderSideColumn: () => record.push("side"),
    regroupChannelRows: () => record.push("regroup"),
    storedChoice: () => "main",
    keepChoice: (view) => record.push(`keep=${view}`),
    renderChoice: () => record.push("draw"),
  });
  assert.equal(owner.storedOpenInView(), "main");
  owner.chooseOpenInView("flat");
  owner.redrawMessageLinks();
  assert.deepEqual(record, [
    "keep=flat", "draw",
    "column", "discord-log<=0", "timeline",
    "column", "cards=held", "thread-list<=1",
    "side",
  ]);
});

test("the quiet guards say what failed and tear nothing down", async () => {
  const said = [];
  const { guardQuietly, refreshQuietly } = createPageGuards({
    showError: (text) => said.push(text),
    readableChannelFailure: (error) => `words: ${error.message}`,
  });
  await guardQuietly(async () => { throw new Error("boom"); })();
  await refreshQuietly(async () => { throw { message: "slow", shownInPill: true }; })();
  await refreshQuietly(async () => { throw new Error("down"); })();
  assert.deepEqual(said, ["boom", "words: down"]);
});

test("leaving Settings blurs a held field and goes back where it was opened from", () => {
  const record = [];
  const held = { blur: () => record.push("blur") };
  globalThis.document = { activeElement: held };
  const noop = () => {};
  const elements = new Map();
  const showing = createScreenShowing({
    screens: {
      screen: () => "settings",
      arrive: noop,
      beforeSettings: () => "main",
    },
    el: (id) => {
      if (!elements.has(id)) elements.set(id, { hidden: false, textContent: "", setAttribute: noop });
      return elements.get(id);
    },
    closeOverscroll: noop, holdChannelPlace: noop, postCardBackToMain: noop,
    placePostCard: noop, disarmClear: noop, browserReading: () => false,
    stopReading: noop, closeDismissedSends: noop, renderDismissedSends: noop,
    renderJumpNewest: noop, placeChipsOverComposer: noop, renderControlBar: noop,
    renderColumns: noop, pointSettingsAtReading: noop, refreshUpstreamClear: noop,
    refreshChannelHistory: noop, refreshAgentPrefix: noop, renderIdentities: noop,
    focusLost: () => false, settleFocus: noop, scheduleBackSync: noop,
  });
  showing.closeSettings();
  assert.deepEqual(record, ["blur"]);
  assert.equal(elements.get("screen-main").hidden, false);
  assert.equal(elements.get("screen-settings").hidden, true);
});
