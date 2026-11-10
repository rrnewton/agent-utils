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
    filter: () => ({}), list: () => ({}), rows: () => ({}),
    selection: (view) => ({ view: view ?? "thread" }), composer: () => ({}),
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
