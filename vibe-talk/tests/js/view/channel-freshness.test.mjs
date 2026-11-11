// How old what is on screen is, said over the list: its owner, over fakes
// (`#624 voice-page-modularity`).

import assert from "node:assert/strict";
import { test } from "node:test";
import { createFreshness } from "../../../web/src/view/channel/freshness.ts";

test("setting freshness draws the pill with the live words", () => {
  const state = { freshness: "fresh", freshAt: 42, refreshing: false };
  const pill = { textContent: "", hidden: true, attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, getAttribute(k) { return this.attrs[k]; } };
  const gesture = { hidden: true, getAttribute: () => null };
  const owner = createFreshness({
    loadState: () => state,
    side: () => false,
    el: (id) => (id === "channel-freshness" ? pill : gesture),
    leftEl: () => pill,
    stamp: (at) => `at ${at}`,
    liveAttached: () => true,
    liveDelivery: () => "stream",
    staleWords: () => "",
    chatServiceOf: () => "the chat service",
    selectedChannelId: () => "c1",
    view: () => "discord",
    searchOpen: () => false,
    columnsShown: () => false,
    sideColumn: () => ({ mode: "closed" }),
    mainColumn: () => null,
    inColumn: (_column, act) => act(),
    sideColumnUp: () => false,
    threadListFailure: () => "",
    threadListLandedAt: () => 0,
    setTextIfChanged: (node, text) => { node.textContent = text; },
    holdingReader: (act) => act(),
    keepingCardPlace: (act) => act(),
  });
  owner.setChannelFreshness("fresh");
  assert.equal(state.freshness, "fresh");
  assert.equal(pill.textContent, "Live · updated at 42");
  assert.equal(pill.attrs["data-state"], "fresh");
  assert.equal(pill.hidden, false);
});
