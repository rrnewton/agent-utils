// Tests for web/src/view/channel/jumpanchor.ts on its own (`#1224 mention-jump-in-view`): the row a
// jump landed on, held as the scroll anchor, and the lost jump told to the trace. Stand-in rows
// with measured tops, a fake clock and timers, as the other view suites stand their pages in.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const CHANNEL_VIEW = await moduleForTests("src/view/channel/index.ts");

/** A page stand-in: rows by id in a list over a scroller whose place a test moves under the hold. */
function makeAnchor() {
  const { createJumpAnchor, JUMP_LOST_WINDOW_MS } = CHANNEL_VIEW;
  const state = {
    key: "c1|flat",
    now: 1000,
    timers: [],
    lost: [],
    rows: new Map(),
    areaTop: 100,
    areaHeight: 600,
  };
  const area = {
    scrollTop: 500,
    getBoundingClientRect: () => ({ top: state.areaTop, bottom: state.areaTop + state.areaHeight }),
  };
  const row = (id, top) => {
    const one = {
      id,
      top,
      height: 120,
      parentNode: { id: "discord-log" },
      getBoundingClientRect() {
        return { top: this.top, bottom: this.top + this.height };
      },
    };
    state.rows.set(id, one);
    return one;
  };
  const anchor = createJumpAnchor({
    el: () => area,
    contextKey: () => state.key,
    rowById: (id) => state.rows.get(id) ?? null,
    listPlaceTop: (el) => el.getBoundingClientRect().top,
    nudge: (area, delta) => { area.scrollTop += delta; },
    now: () => state.now,
    clock: {
      setTimeout: (run, ms) => {
        const handle = { run, ms, cleared: false };
        state.timers.push(handle);
        return handle;
      },
      clearTimeout: (handle) => {
        if (handle) handle.cleared = true;
      },
    },
    noteJumpLost: (id) => state.lost.push(id),
  });
  /** Run the timers due, as the lost window's checks come round. */
  const tick = (times = 1) => {
    for (let at = 0; at < times; at += 1) {
      const due = state.timers.filter((handle) => !handle.cleared);
      state.timers = [];
      for (const handle of due) handle.run();
    }
  };
  return { anchor, area, row, state, tick, JUMP_LOST_WINDOW_MS };
}

test("`#1224 mention-jump-in-view`: the landed row is the anchor, placed back where the jump put it", () => {
  const { anchor, area, row, state } = makeAnchor();
  const landed = row("260", 400);
  row("261", 520);
  anchor.note(landed, "260");
  assert.equal(anchor.row(), landed, "the landed row is not held");
  // What arrived above grew: the row stands 180px lower, the scroller unmoved, as a summary
  // landing or an older page leaves it before a keeper's pass.
  landed.top += 180;
  assert.equal(anchor.place(), true);
  assert.equal(area.scrollTop, 680, "placing the held row did not move the scroller by its drift");
  // A rebuilt row (a new node for the same message) is re-found by id, not lost with the old node.
  const rebuilt = row("260", 900);
  rebuilt.top = landed.top;
  state.rows.set("260", rebuilt);
  assert.equal(anchor.row(), rebuilt, "a rebuilt row holding the message is not the held row");
});

test("`#1224 mention-jump-in-view`: the page's own correction under the held row is not the reader moving", () => {
  const { anchor, area, row } = makeAnchor();
  const landed = row("260", 400);
  anchor.note(landed, "260");
  // A read landing held by this row moved the scroller under it (the row kept at its
  // landing offset, the scroller elsewhere): the hold follows that place.
  area.scrollTop += 93;
  assert.equal(anchor.row(), landed, "the page correcting under the held row ended the hold");
  // Measured mid-redraw, before the next correction lands, the row stands off its offset
  // while the scroller is where the page left it: still the page's act, not the reader's.
  landed.top += 30;
  assert.equal(anchor.row(), landed, "a mid-redraw measurement ended the hold");
  assert.equal(anchor.place(), true);
  assert.equal(area.scrollTop, 500 + 93 + 30, "placing did not move the scroller by the drift");
});

test("`#1224 mention-jump-in-view`: a step back in flight (scroller reset, row displaced) is not the reader moving", () => {
  const { anchor, area, row } = makeAnchor();
  const landed = row("260", 400);
  anchor.note(landed, "260");
  // Older rows stand in above and the scroller sits reset while the step's settle is
  // still to land: neither measure on its own is the reader's scroll, and placing
  // would chase the row's mid-flight stand.
  area.scrollTop = 0;
  landed.top += 5000;
  assert.equal(anchor.row(), landed, "an in-flight step back ended the hold");
  assert.equal(anchor.place(), false, "placing chased a mid-flight stand");
  // The settle lands: the row back at its offset under a scroller the page chose.
  area.scrollTop = 3800;
  landed.top = 400;
  assert.equal(anchor.row(), landed, "the landed settle did not keep the hold");
  landed.top += 40;
  assert.equal(anchor.place(), true);
  assert.equal(area.scrollTop, 3840, "placing after the settle did not move the scroller by the drift");
});

test("`#1224 mention-jump-in-view`: the reader's own scroll, a new list, or a dismissal ends the hold", () => {
  const { anchor, area, row, state } = makeAnchor();
  const landed = row("260", 400);
  anchor.note(landed, "260");
  area.scrollTop += 240;
  // The reader scrolled and the row went with the list: the hold is theirs to end.
  landed.top -= 240;
  assert.equal(anchor.row(), null, "a reader who scrolled kept the jump's hold");
  assert.equal(anchor.place(), false, "an ended hold still places");

  anchor.note(row("261", 300), "261");
  state.key = "c2|flat";
  assert.equal(anchor.row(), null, "a hold from another channel's list still stands");

  const third = row("262", 300);
  anchor.note(third, "262");
  anchor.clear();
  assert.equal(anchor.row(), null, "a cleared hold still stands");
  assert.equal(anchor.place(), false);
});

test("`#1208` jump-lost: a target pushed off screen inside the window is told to the trace, once", () => {
  const { anchor, area, row, state, tick, JUMP_LOST_WINDOW_MS } = makeAnchor();
  const landed = row("260", 400);
  anchor.note(landed, "260");
  // The target is pushed off the foot with no scroll of the reader's: its top passes the area's
  // foot while the scroller's place is the landing's own (a keeper will move it in a moment;
  // the watch's next check is what says the jump was lost before that).
  landed.top = state.areaTop + state.areaHeight + 40;
  state.now += 500;
  tick();
  assert.deepStrictEqual(state.lost, ["260"], "the lost jump was not told to the trace");
  tick();
  assert.deepStrictEqual(state.lost, ["260"], "the lost jump was told twice");

  // In view the whole window: nothing is told.
  const second = makeAnchor();
  const stayed = second.row("260", 400);
  second.anchor.note(stayed, "260");
  second.state.now += JUMP_LOST_WINDOW_MS + 1000;
  second.tick(50);
  assert.deepStrictEqual(second.state.lost, [], "a jump that stayed in view was reported lost");

  // The reader scrolled the target away themselves: their act, not a lost jump.
  const third = makeAnchor();
  const own = third.row("260", 400);
  third.anchor.note(own, "260");
  third.area.scrollTop += 800;
  own.top -= 800;
  third.state.now += 500;
  third.tick(4);
  assert.deepStrictEqual(third.state.lost, [], "a target the reader scrolled off was reported lost");
});

test("`#1208` jump-lost: outside the window a lost target is the reader's place, not a trace", () => {
  const { anchor, row, state, tick, JUMP_LOST_WINDOW_MS } = makeAnchor();
  const landed = row("260", 400);
  anchor.note(landed, "260");
  state.now += JUMP_LOST_WINDOW_MS + 500;
  landed.top = state.areaTop + state.areaHeight + 40;
  tick(4);
  assert.deepStrictEqual(state.lost, [], "a target lost after the window was reported");
  assert.equal(anchor.checkLost(), false, "a late check still reports");
});
