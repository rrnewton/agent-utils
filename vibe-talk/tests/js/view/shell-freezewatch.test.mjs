// Tests for web/src/view/shell/freezewatch.ts on its own (`#2066 vibe-talk-page-freeze`): the
// module built as the page build builds it, over a fake clock, store, document and performance
// observers. Every case is condition-based — timers are run by hand at chosen readings of the
// fake clock, never by waiting — per the owner's no-wall-clock rule.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");

/** A stand-in page: fake clock/timers, store, document, observers and poster, all by hand. */
function makeWatch(overrides = {}) {
  const state = {
    now: 0,
    hidden: false,
    timers: [],
    frames: [],
    stored: new Map(),
    sent: [],
    sendTakes: true,
    observers: new Map(),
    listeners: new Map(),
    replays: 0,
    view: { view: "discord", screen: "main", channelView: "thread", twoColumns: true, reading: "pending", windowed: true },
    ...overrides,
  };
  const doc = {
    addEventListener(type, hear) {
      if (!state.listeners.has(type)) state.listeners.set(type, new Set());
      state.listeners.get(type).add(hear);
    },
    removeEventListener(type, hear) {
      state.listeners.get(type)?.delete(hear);
    },
  };
  const watch = SHELL.createFreezeWatch({
    now: () => state.now,
    clock: {
      setTimeout: (run, ms) => {
        const handle = { run, at: state.now + ms };
        state.timers.push(handle);
        return handle;
      },
      clearTimeout: (handle) => {
        state.timers = state.timers.filter((one) => one !== handle);
      },
    },
    every: state.noInterval ? undefined : (_ms, run) => {
      state.beatRun = run;
      return () => { state.beatRun = null; };
    },
    storage: {
      getItem: (key) => (state.stored.has(key) ? state.stored.get(key) : null),
      setItem: (key, value) => state.stored.set(key, value),
      removeItem: (key) => state.stored.delete(key),
    },
    documentTarget: () => doc,
    hidden: () => state.hidden,
    requestFrame: (run) => state.frames.push(run),
    observe: (type, note) => {
      state.observers.set(type, note);
      return () => state.observers.delete(type);
    },
    state: () => state.view,
    replaysWaiting: () => state.replays,
    send: async (text) => {
      state.sent.push(JSON.parse(text));
      return state.sendTakes;
    },
    // As the composition root's: never while hidden either.
    canPost: () => state.postable !== false && !state.hidden,
  });
  /** Move the clock to `to` and run every timer due by then, in due order. */
  const tick = (to) => {
    state.now = to;
    for (;;) {
      const due = state.timers.filter((one) => one.at <= state.now).sort((a, b) => a.at - b.at)[0];
      if (!due) return;
      state.timers = state.timers.filter((one) => one !== due);
      due.run();
    }
  };
  const fire = (type) => {
    for (const hear of [...(state.listeners.get(type) || [])]) hear();
  };
  /** One heartbeat, as the interval delivers it at the clock's present reading. */
  const beat = () => {
    assert.ok(state.beatRun, "no heartbeat was armed");
    state.beatRun();
  };
  const settle = async () => {
    await Promise.resolve();
    await Promise.resolve();
  };
  return { state, watch, tick, fire, beat, settle };
}

test("#2066: an in-sight heartbeat silence is one freeze, posted on recovery", async () => {
  const { state, watch, fire, beat, settle } = makeWatch();
  watch.start();
  state.now = 250;
  beat();
  state.now = 500;
  beat();
  fire("pointerdown");
  // The main thread gives nothing for 4.5 s: the heartbeat runs only at the thaw.
  state.now = 5000;
  beat();
  await settle();
  assert.equal(state.sent.length, 1, "the freeze was not posted on recovery");
  const [freeze] = state.sent[0].events;
  assert.equal(freeze.kind, "freeze");
  assert.equal(freeze.dur_ms, 4500);
  assert.equal(freeze.action, "pointer");
  assert.equal(freeze.action_age_ms, 4500);
  assert.equal(freeze.view, "discord");
  assert.equal(freeze.channel_view, "thread");
  assert.equal(freeze.layout, "two_column");
  assert.equal(freeze.reading, "pending");
  assert.equal(freeze.windowed, true);
  assert.equal(state.stored.size, 0, "a taken report stayed on the device");
  watch.stop();
});

test("#2066: a hidden silence is no freeze, and the show carries its backlog", async () => {
  const { state, watch, tick, fire, beat, settle } = makeWatch();
  watch.start();
  state.now = 250;
  beat();
  state.hidden = true;
  state.now = 1000;
  fire("visibilitychange");
  fire("pointerdown");
  fire("keydown");
  // Two minutes out of sight: the interval throttled flat, and none of it is a freeze.
  state.now = 121000;
  beat();
  state.replays = 7;
  state.hidden = false;
  fire("visibilitychange");
  // The zero timer set at the show runs 900 ms late; the first frame 1100 ms late.
  state.now = 121900;
  tick(121900);
  state.now = 122100;
  for (const run of state.frames.splice(0)) run();
  await settle();
  assert.equal(state.sent.length, 1, "the show did not post once measured");
  const kinds = state.sent[0].events.map((one) => one.kind);
  assert.deepEqual(kinds, ["hidden", "visible"], `a hidden gap froze: ${kinds}`);
  const shown = state.sent[0].events[1];
  assert.equal(shown.dur_ms, 120000);
  assert.equal(shown.replays_waiting, 7);
  assert.equal(shown.inputs_hidden, 2);
  assert.equal(shown.timer_delay_ms, 900);
  assert.equal(shown.raf_delay_ms, 1100);
  watch.stop();
});

test("#2066: long tasks and long frames over the threshold are kept, with the frame's code", async () => {
  const { state, watch, settle } = makeWatch();
  watch.start();
  state.now = 2000;
  state.observers.get("longtask")([{ startTime: 100, duration: 80 }, { startTime: 1500, duration: 640 }]);
  state.observers.get("long-animation-frame")([{
    startTime: 1800,
    duration: 900,
    scripts: [
      { duration: 10, invokerType: "event-listener", invoker: "DIV#x.onclick", sourceFunctionName: "small" },
      { duration: 700, invokerType: "event-listener", invoker: "BUTTON#read-next.onclick", sourceFunctionName: "renderRows" },
    ],
  }]);
  watch.flush();
  await settle();
  assert.equal(state.sent.length, 1);
  const [task, frame] = state.sent[0].events;
  assert.equal(task.kind, "longtask");
  assert.equal(task.t, 1500);
  assert.equal(task.dur_ms, 640);
  assert.equal(frame.kind, "loaf");
  assert.equal(frame.script, "event-listener BUTTON#read-next.onclick renderRows");
  watch.stop();
});

test("#2066: the first stalls after a show say how soon after it they came", async () => {
  const { state, watch, fire, settle } = makeWatch();
  watch.start();
  state.hidden = true;
  state.now = 1000;
  fire("visibilitychange");
  state.hidden = false;
  state.now = 4000;
  fire("visibilitychange");
  state.now = 5200;
  state.observers.get("longtask")([{ startTime: 4600, duration: 300 }]);
  const events = JSON.parse(watch.report()).events;
  const task = events.find((one) => one.kind === "longtask");
  assert.equal(task.resume_age_ms, 1200);
  watch.stop();
  await settle();
});

test("#2066: the ring is bounded, kept on the device, and a refused posting keeps it", async () => {
  const held = makeWatch();
  held.state.sendTakes = false;
  held.watch.start();
  for (let at = 0; at < 45; at += 1) {
    held.state.observers.get("longtask")([{ startTime: at * 10, duration: 300 }]);
  }
  held.watch.flush();
  await held.settle();
  const kept = JSON.parse(held.watch.report()).events;
  assert.equal(kept.length, SHELL.FREEZE_MAX_EVENTS, "the ring outgrew its bound");
  assert.equal(kept[0].t, 50, "the oldest were not the ones dropped");
  const storedText = held.state.stored.get(SHELL.FREEZE_STORAGE_KEY);
  assert.ok(storedText, "an unposted ring was not kept on the device");
  held.watch.stop();

  // The next load posts what was left, under its own clock, and forgets it once taken.
  const next = makeWatch();
  next.state.stored.set(SHELL.FREEZE_STORAGE_KEY, storedText);
  next.watch.start();
  await next.settle();
  assert.equal(next.state.sent.length, 1, "the leftovers were not posted on load");
  assert.equal(next.state.sent[0].events.length, SHELL.FREEZE_MAX_EVENTS);
  assert.equal(next.state.stored.size, 0, "taken leftovers stayed on the device");
  next.watch.stop();
});

test("#2066: with no interval timer the watch does not run at all", async () => {
  const { state, watch, fire, settle } = makeWatch({ noInterval: true });
  state.stored.set(SHELL.FREEZE_STORAGE_KEY, JSON.stringify({
    v: 1, startedAt: 0,
    events: [{ t: 1, kind: "freeze", dur_ms: 3000 }],
  }));
  watch.start();
  fire("visibilitychange");
  await settle();
  assert.equal(state.sent.length, 0, "a watch that cannot beat posted");
  assert.equal(state.observers.size, 0, "a watch that cannot beat observed");
  assert.equal(JSON.parse(watch.report()).events.length, 0, "a watch that cannot beat kept a record");
  assert.ok(state.stored.has(SHELL.FREEZE_STORAGE_KEY), "the leavings were not left for a load that can watch");
  watch.stop();
});

test("#2066: nothing is posted while unpostable, and the heartbeat retries on recovery", async () => {
  // Offline, hidden, or a read token (the composition root's canPost): an artificial
  // long task is recorded and kept, no request is attempted — the offline walks count
  // a refused request against the page — and once postable again the next heartbeat
  // sends what was kept.
  const { state, watch, beat, settle } = makeWatch({ postable: false });
  watch.start();
  state.observers.get("longtask")([{ startTime: 100, duration: 900 }]);
  watch.flush();
  await settle();
  assert.equal(state.sent.length, 0, "a report went out while unpostable");
  assert.equal(JSON.parse(watch.report()).events.length, 1, "the stall was not kept");
  assert.ok(state.stored.has(SHELL.FREEZE_STORAGE_KEY), "the kept stall was not stored");
  state.postable = true;
  state.now = 10000;
  beat();
  await settle();
  assert.equal(state.sent.length, 1, "the kept stall was not posted on recovery");
  assert.equal(state.sent[0].events[0].kind, "longtask");
  watch.stop();
});

test("#2066: damaged leavings are dropped, and unknown page words read as other", async () => {
  const { state, watch, settle } = makeWatch();
  state.stored.set(SHELL.FREEZE_STORAGE_KEY, "{not json");
  state.view = { view: "hologram", screen: "attic", channelView: "spiral", twoColumns: false, reading: "shouting", windowed: false };
  watch.start();
  await settle();
  assert.equal(state.sent.length, 0, "damage was posted");
  state.observers.get("longtask")([{ startTime: 5, duration: 300 }]);
  const [task] = JSON.parse(watch.report()).events;
  assert.equal(task.view, "other");
  assert.equal(task.screen, "other");
  assert.equal(task.channel_view, "none");
  assert.equal(task.reading, "other");
  assert.equal(task.layout, "single");
  watch.stop();
});
