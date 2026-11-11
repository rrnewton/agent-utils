// Tests for the column state factories and access in web/src/view/channel/columnwork.ts
// (`#624 voice-page-modularity`): fresh records per column, made afresh each time, and the
// working column's records under their page names. The page over the same factories is in
// ../page/.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");

test("fresh column records start as they always did, and share nothing", () => {
  assert.deepEqual(CHANNEL.freshLoadState(), {
    freshness: "fresh", freshAt: 0, refreshing: false, lineTimer: null, generation: 0,
  });
  assert.deepEqual(CHANNEL.freshScrollPlace(), { jumpWanted: false, inScrollback: false });
  assert.deepEqual(CHANNEL.freshScrollTools(), { held: 0, owed: false });
  const one = CHANNEL.freshLoadState();
  one.freshAt = 99;
  assert.equal(CHANNEL.freshLoadState().freshAt, 0, "each record is its own");
});

test("the access reads the working column's records, and follows a switch", () => {
  const makers = {
    older: () => ({}), arrows: () => ({}), load: () => CHANNEL.freshLoadState(),
    entry: () => ({}), pins: () => ({}), tools: () => CHANNEL.freshScrollTools(),
    filter: () => ({}), list: () => ({}), rows: () => CHANNEL.freshTimelineRows(),
    selection: (view) => CHANNEL.freshSelection(view), composer: () => CHANNEL.freshComposerState(),
    done: () => ({}), place: () => CHANNEL.freshScrollPlace(), gather: () => ({}),
  };
  const columns = CHANNEL.createColumns(makers);
  const work = CHANNEL.createColumnWork(columns);
  const access = CHANNEL.createColumnStateAccess(work);
  access.loadState().freshAt = 7;
  access.scrollPlace().inScrollback = true;
  assert.equal(columns.main.load.freshAt, 7);
  assert.equal(columns.side.load.freshAt, 0, "the other column's record is untouched");
  const inSide = work.inColumn(columns.side, () => {
    access.loadState().freshAt = 9;
    return [access.loadState().freshAt, access.scrollPlace().inScrollback, access.scrollTools().held];
  });
  assert.deepEqual(inSide, [9, false, 0]);
  assert.equal(access.loadState().freshAt, 7, "the switch back is restored");
});

test("fresh selection, rows and composer records start as they always did", () => {
  assert.deepEqual(CHANNEL.freshSelection(), { view: "thread", threadId: null, thread: null, origin: "main" });
  assert.deepEqual(CHANNEL.freshSelection("main"), { view: "main", threadId: null, thread: null, origin: "main" });
  assert.deepEqual(CHANNEL.freshTimelineRows(), { messages: [], threads: [] });
  assert.deepEqual(CHANNEL.freshComposerState(), { key: "" });
  const rows = CHANNEL.freshTimelineRows();
  rows.messages.push({ id: "m1" });
  assert.deepEqual(CHANNEL.freshTimelineRows(), { messages: [], threads: [] }, "each record is its own");
});

test("selection, rows, composer and the view key follow the working column", () => {
  const makers = {
    older: () => ({}), arrows: () => ({}), load: () => CHANNEL.freshLoadState(),
    entry: () => ({}), pins: () => ({}), tools: () => CHANNEL.freshScrollTools(),
    filter: () => ({}), list: () => ({}), rows: () => CHANNEL.freshTimelineRows(),
    selection: (view) => CHANNEL.freshSelection(view), composer: () => CHANNEL.freshComposerState(),
    done: () => ({}), place: () => CHANNEL.freshScrollPlace(), gather: () => ({}),
  };
  const columns = CHANNEL.createColumns(makers);
  const work = CHANNEL.createColumnWork(columns);
  const access = CHANNEL.createColumnStateAccess(work);
  assert.equal(access.selection().view, "main", "the left column starts on Main");
  assert.equal(columns.side.selection.view, "thread", "the right column starts on a thread");
  access.selection().threadId = "t9";
  assert.equal(access.viewKey("thread"), "thread:t9");
  assert.equal(access.viewKey(), "main", "no view named is the working column's");
  access.timelineRows().messages.push({ id: "m1" });
  access.composerState().key = "box";
  const inSide = work.inColumn(columns.side, () => [
    access.selection().view, access.timelineRows().messages.length, access.composerState().key,
  ]);
  assert.deepEqual(inSide, ["thread", 0, ""], "the side column's records are its own");
  assert.equal(access.timelineRows().messages.length, 1);
});
