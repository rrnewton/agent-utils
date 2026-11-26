// Tests for web/src/view/shell/clientdebug.ts on its own (`#2276 vibe-talk-client-debug-mode`): the
// module built as the page build builds it, over a fake clock, store, cookie, observers, profiler and
// element prototype. Every case is condition-based: timers run by hand at chosen readings of the fake
// clock, never by waiting (owner rule 346).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const SHELL = await moduleForTests("src/view/shell/index.ts");

/** A stand-in page: every port by hand, every call counted. */
function makeDebug(overrides = {}) {
  const state = {
    now: 0,
    held: false,
    query: "",
    cookie: false,
    cookieWrites: [],
    kept: [],
    types: ["longtask", "long-animation-frame", "event", "layout-shift"],
    observers: new Map(),
    observeCalls: [],
    profilerExists: true,
    profilerAllowed: true,
    profilers: [],
    timers: [],
    measures: [],
    cleared: 0,
    nodes: 4321,
    sent: [],
    sendTakes: true,
    postable: true,
    ...overrides,
  };
  const proto = {
    getBoundingClientRect() { return { top: 0 }; },
    getClientRects() { return []; },
  };
  state.proto = proto;
  state.originals = { gbcr: proto.getBoundingClientRect, gcr: proto.getClientRects };
  const debug = SHELL.createClientDebug({
    now: () => state.now,
    held: () => state.held,
    keep: (on) => state.kept.push(on),
    query: () => state.query,
    cookieSet: () => state.cookie,
    setCookie: (on) => { state.cookie = on; state.cookieWrites.push(on); },
    supportedEntryTypes: () => state.types,
    observe: (type, note, options) => {
      state.observeCalls.push({ type, options });
      state.observers.set(type, note);
      return () => state.observers.delete(type);
    },
    profilerExists: () => state.profilerExists,
    startProfiler: (sampleInterval, maxBufferSize) => {
      if (!state.profilerAllowed) throw new Error("NotAllowedError: no document policy");
      const profiler = {
        sampleInterval, maxBufferSize, stopped: false, trace: null,
        stop() {
          this.stopped = true;
          return Promise.resolve(this.trace ?? { resources: [], frames: [], stacks: [], samples: [] });
        },
      };
      state.profilers.push(profiler);
      return profiler;
    },
    elementPrototype: () => proto,
    nodeCount: () => state.nodes,
    ...(state.sizes ? { sizes: () => state.sizes } : {}),
    measure: (name, start, end) => state.measures.push({ name, start, end }),
    clearMeasures: () => { state.cleared += 1; },
    clock: {
      setTimeout: (run, ms) => {
        const handle = { run, at: state.now + ms };
        state.timers.push(handle);
        return handle;
      },
      clearTimeout: (handle) => { state.timers = state.timers.filter((one) => one !== handle); },
    },
    send: async (text) => { state.sent.push(JSON.parse(text)); return state.sendTakes; },
    canPost: () => state.postable,
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
  /** A span from `from` to `to` on the fake clock, with `reads` layout reads inside it. */
  const spanAt = (name, from, to, reads = 0) => {
    state.now = from;
    debug.span(name, () => {
      for (let i = 0; i < reads; i += 1) proto.getBoundingClientRect();
      state.now = to;
    });
  };
  const settle = async () => {
    for (let i = 0; i < 4; i += 1) await Promise.resolve();
  };
  return { state, debug, tick, spanAt, settle, proto };
}

test("#2276: off is wholly off: no observer, profiler, wrapped method, timer, mark or cookie", () => {
  const { state, debug, proto } = makeDebug();
  debug.start();
  assert.equal(debug.enabled(), false);
  assert.deepEqual(state.observeCalls, [], "an observer was registered while off");
  assert.equal(state.profilers.length, 0, "a profiler started while off");
  assert.equal(proto.getBoundingClientRect, state.originals.gbcr, "a layout read was wrapped while off");
  assert.equal(proto.getClientRects, state.originals.gcr);
  assert.equal(state.timers.length, 0, "a timer was armed while off");
  assert.deepEqual(state.cookieWrites, [], "the cookie was written while off and unset");
  let ran = 0;
  assert.equal(debug.span("key", () => { ran += 1; state.now = 500; return "answer"; }), "answer");
  assert.equal(ran, 1);
  assert.deepEqual(state.measures, [], "a mark was made while off");
  const wrapped = debug.spanned("rows", (a, b) => a + b);
  assert.equal(wrapped(2, 3), 5);
  assert.equal(debug.attribute(0, 500, null, () => {}), null);
  assert.equal(debug.summary(), null);
  assert.equal(debug.pending(), false);
});

test("#2276: a cookie left set by an earlier switch is cleared when the switch reads off", () => {
  const { state, debug } = makeDebug({ cookie: true });
  debug.start();
  assert.deepEqual(state.cookieWrites, [false]);
  assert.equal(debug.enabled(), false);
});

test("#2276: on, it watches long frames and events, counts layout reads, samples, and keeps the cookie", () => {
  const { state, debug, proto } = makeDebug({ held: true });
  debug.start();
  assert.equal(debug.enabled(), true);
  assert.deepEqual(state.cookieWrites, [true], "the cookie follows the kept choice");
  assert.deepEqual(state.observeCalls.map((one) => one.type).sort(), ["event", "long-animation-frame"]);
  assert.deepEqual(state.observeCalls.find((one) => one.type === "event").options, { durationThreshold: 48 });
  assert.notEqual(proto.getBoundingClientRect, state.originals.gbcr);
  assert.equal(state.profilers.length, 1);
  assert.equal(state.profilers[0].sampleInterval, SHELL.DEBUG_SAMPLE_MS);
  const caps = debug.caps();
  assert.deepEqual(caps, {
    loaf: true, longtask: true, event: true, profiler: true, policy: true, sample_ms: SHELL.DEBUG_SAMPLE_MS,
  });
  // Off again: everything put back, the profiler stopped, the marks cleared.
  state.now = 10;
  debug.span("key", () => { state.now = 30; });
  debug.setOn(false);
  assert.deepEqual(state.kept, [false]);
  assert.equal(state.cookie, false);
  assert.equal(debug.enabled(), false);
  assert.equal(state.observers.size, 0, "an observer stayed on");
  assert.equal(proto.getBoundingClientRect, state.originals.gbcr, "a wrapped method stayed");
  assert.equal(proto.getClientRects, state.originals.gcr);
  assert.equal(state.profilers[0].stopped, true);
  assert.equal(state.cleared, 1);
  assert.equal(state.timers.length, 0, "a timer stayed armed");
});

test("#2276: the one-load URL flag turns it on without keeping it or setting the cookie", () => {
  const { state, debug } = makeDebug({ query: "?a=1&client-debug=1" });
  debug.start();
  assert.equal(debug.enabled(), true);
  assert.deepEqual(state.cookieWrites, []);
  assert.deepEqual(state.kept, []);
});

test("#2276: without the document policy the profiler is refused and the caps say so", () => {
  const { debug } = makeDebug({ held: true, profilerAllowed: false, types: ["longtask"] });
  debug.start();
  assert.deepEqual(debug.caps(), {
    loaf: false, longtask: true, event: false, profiler: true, policy: false, sample_ms: 0,
  });
});

test("#2276: a stall names the spans in it, the ones open at its start, the last ends before it, and what no span covered", () => {
  const { state, debug, spanAt } = makeDebug({ held: true, profilerExists: false });
  debug.start();
  spanAt("rows", 100, 112);
  spanAt("search", 150, 160, 2);
  // `key` opens at 200 and runs to 500 with `search-fast` (210-400, 5 reads) inside it.
  state.now = 200;
  debug.span("key", () => {
    spanAt("search-fast", 210, 400, 5);
    state.now = 500;
  });
  spanAt("scroll-follow", 600, 610);
  // The stall: 205 to 705 (500 ms).
  const got = debug.attribute(205, 705, null, () => {});
  assert.deepEqual(got.open, [{ name: "key", age_ms: 5 }]);
  assert.deepEqual(got.recent, [
    { name: "search", dur_ms: 10, ago_ms: 45 },
    { name: "rows", dur_ms: 12, ago_ms: 93 },
  ]);
  assert.deepEqual(got.spans, [
    { name: "key", n: 1, ms: 295, reads: 5, nodes: 4321 },
    { name: "search-fast", n: 1, ms: 190, reads: 5, nodes: 4321 },
    { name: "scroll-follow", n: 1, ms: 10, reads: 0 },
  ]);
  // Covered: 205-500 by key (search-fast inside it) and 600-610: 305 of 500.
  assert.equal(got.unattributed_ms, 195);
  assert.equal(got.frame, undefined);
  assert.ok(state.measures.some((one) => one.name === "vt:search-fast" && one.start === 210 && one.end === 400));
});

test("#2276: a long span says the sizes it ran at, and counted cache characters ride the span that read them", () => {
  const { state, debug, spanAt } = makeDebug({ held: true, profilerExists: false, sizes: { s: 812, d: 140 } });
  debug.start();
  // A save that reads 400 000 and writes 410 000 characters, its cache read and write nested in it.
  state.now = 100;
  debug.span("save-scope", () => {
    state.now = 105;
    debug.span("cache-read", () => { debug.note("chars", 400000); state.now = 140; });
    debug.span("cache-write", () => { debug.note("chars", 410000); state.now = 190; });
    debug.note("rows", 0);
    state.now = 200;
  });
  spanAt("key", 210, 215);
  const got = debug.attribute(100, 220, null, () => {});
  assert.deepEqual(got.spans, [
    { name: "save-scope", n: 1, ms: 100, reads: 0, nodes: 4321, s: 812, d: 140 },
    { name: "cache-write", n: 1, ms: 50, reads: 0, nodes: 4321, s: 812, d: 140, chars: 410000 },
    { name: "cache-read", n: 1, ms: 35, reads: 0, nodes: 4321, s: 812, d: 140, chars: 400000 },
    { name: "key", n: 1, ms: 5, reads: 0 },
  ]);
  // Off, a note counts nothing and costs one test.
  debug.setOn(false);
  debug.note("chars", 5);
});

test("#2276: a long frame's detail is capped and carries no URL query, hash, foreign id or src", () => {
  const { debug } = makeDebug({ held: true, profilerExists: false });
  debug.start();
  const script = (n, extra = {}) => ({
    duration: n, invokerType: "event-listener", invoker: "DIV#discord-log.onkeydown",
    sourceURL: "https://host.example/assets/voice.0123456789abcdef0123456789abcdef.js?token=secret#x",
    sourceFunctionName: "applySearchFast", sourceCharPosition: 123456, forcedStyleAndLayoutDuration: 3, ...extra,
  });
  const entry = {
    startTime: 1000, duration: 260, blockingDuration: 210, renderStart: 1230, styleAndLayoutStart: 1240,
    scripts: [
      script(20, { invoker: "IMG[src=https://host.example/a.png?token=secret].onload", invokerType: "event-listener" }),
      script(180),
      script(30, { invoker: "DIV#m-1732600000123456.onclick", sourceFunctionName: "owner@example.com" }),
      script(10, { invokerType: "teleport" }),
      script(5),
    ],
  };
  const got = debug.attribute(1000, 1260, entry, () => {});
  assert.equal(got.frame.blocking_ms, 210);
  assert.equal(got.frame.render_ms, 30);
  assert.equal(got.frame.style_layout_ms, 20);
  assert.equal(got.frame.scripts.length, 4, "more scripts than the server takes");
  assert.deepEqual(got.frame.scripts[0], {
    fn: "applySearchFast", src: "voice.js:123456", invoker_type: "event-listener",
    invoker: "DIV#discord-log.onkeydown", dur_ms: 180, forced_layout_ms: 3,
  });
  assert.equal(got.frame.scripts[1].invoker, "DIV#m-_.onclick", "a row id rode in");
  assert.equal(got.frame.scripts[1].fn, "owner_example.com");
  assert.equal(got.frame.scripts[2].invoker, "IMG[src].onload");
  assert.equal(got.frame.scripts[3].invoker_type, "other");
  for (const one of got.frame.scripts) {
    for (const text of [one.fn, one.src, one.invoker]) {
      assert.ok(!/[?=@&]|secret|token|0123456789abcdef/u.test(text), `leaked: ${text}`);
    }
  }
});

test("#2276: the profiler's samples in a stall come back as its top frames, through `late`", async () => {
  const { state, debug, tick, settle } = makeDebug({ held: true });
  debug.start();
  const first = state.profilers[0];
  // Frames: 0 main (root), 1 renderChannelRowsNow, 2 drawSeams. Stacks: 0 [main], 1 [main>rows], 2 [main>rows>seams].
  first.trace = {
    resources: ["https://host.example/assets/voice.0123456789abcdef0123456789abcdef.js?token=x"],
    frames: [
      { name: "main", resourceId: 0, line: 1, column: 10 },
      { name: "renderChannelRowsNow", resourceId: 0, line: 1, column: 500 },
      { name: "drawSeams", resourceId: 0, line: 1, column: 900 },
    ],
    stacks: [{ frameId: 0 }, { frameId: 1, parentId: 0 }, { frameId: 2, parentId: 1 }],
    samples: [
      { timestamp: 50 },
      { timestamp: 1000, stackId: 2 }, { timestamp: 1016, stackId: 2 }, { timestamp: 1032, stackId: 2 },
      { timestamp: 1048, stackId: 1 }, { timestamp: 1064 },
      { timestamp: 5000, stackId: 0 },
    ],
  };
  let profile = null;
  debug.attribute(1000, 1064, null, (got) => { profile = got; });
  assert.equal(debug.pending(), true, "a report must wait for the profile");
  tick(state.now);
  await settle();
  assert.equal(first.stopped, true);
  assert.equal(state.profilers.length, 2, "the next profiler did not start");
  assert.deepEqual(profile, {
    samples: 5, js_samples: 4, interval_ms: SHELL.DEBUG_SAMPLE_MS,
    self_top: [
      { fn: "drawSeams", src: "voice.js:1:900", n: 3 },
      { fn: "renderChannelRowsNow", src: "voice.js:1:500", n: 1 },
    ],
    total_top: [
      { fn: "main", src: "voice.js:1:10", n: 4 },
      { fn: "renderChannelRowsNow", src: "voice.js:1:500", n: 4 },
      { fn: "drawSeams", src: "voice.js:1:900", n: 3 },
    ],
  });
  assert.equal(debug.pending(), false);
});

test("#2276: the summary totals spans and long frames, is capped, and starts over once posted", () => {
  const { state, debug, spanAt } = makeDebug({ held: true, profilerExists: false });
  debug.start();
  for (let i = 0; i < 30; i += 1) spanAt(`name${String.fromCharCode(97 + (i % 26))}${i >= 26 ? "x" : ""}`, i * 10, i * 10 + 1 + i);
  spanAt("key", 400, 520);
  const frames = state.observers.get("long-animation-frame");
  frames(Array.from({ length: 25 }, (_, i) => ({ startTime: 1000 + i * 100, duration: 50 + i * 5, blockingDuration: i, scripts: [] })));
  const events = state.observers.get("event");
  events([{ name: "keydown", startTime: 2000, duration: 120, processingStart: 2010, processingEnd: 2100 }]);
  const summary = debug.summary();
  assert.equal(summary.spans.length, 16);
  assert.deepEqual(summary.spans[0], { name: "key", n: 1, total_ms: 120, max_ms: 120 });
  assert.ok(summary.spans.some((one) => one.name === "evt:keydown" && one.total_ms === 120));
  assert.ok(summary.spans.some((one) => one.name === "evt-run:keydown" && one.total_ms === 90));
  assert.equal(summary.frames.length, 20);
  assert.equal(summary.frames[0].dur_ms, 170);
  assert.equal(summary.frames_over_50, 25);
  assert.equal(summary.frames_over_100, 15);
  assert.equal(summary.max_frame_ms, 170);
  assert.equal("ua" in summary.caps, false, "the page never reads the user agent");
  assert.ok(JSON.stringify(summary).length < 16 * 1024);
  state.now = 9000;
  debug.posted();
  const fresh = debug.summary();
  assert.equal(fresh.since, 9000);
  assert.deepEqual(fresh.spans, []);
  assert.equal(fresh.frames_over_50, 0);
});

test("#2276: at most one summary-only report a minute, and only after something slow", async () => {
  const { state, debug, tick, spanAt, settle } = makeDebug({ held: true, profilerExists: false });
  debug.start();
  spanAt("key", 100, 120);
  tick(60000);
  await settle();
  assert.equal(state.sent.length, 0, "a quiet minute posted");
  spanAt("rows", 61000, 61080);
  state.postable = false;
  tick(120000);
  await settle();
  assert.equal(state.sent.length, 0, "posted while unpostable");
  state.postable = true;
  tick(180000);
  await settle();
  assert.equal(state.sent.length, 1);
  assert.deepEqual(state.sent[0].events, []);
  assert.equal(state.sent[0].v, 1);
  assert.ok(state.sent[0].debug.spans.some((one) => one.name === "rows"));
  // Posted: the period starts over, and nothing slow since means nothing more.
  tick(240000);
  tick(300000);
  await settle();
  assert.equal(state.sent.length, 1, "a second report without anything slow");
});

test("#2276: names are redacted by the server's own rule: emails, URL queries, Slack ids, long numbers", () => {
  for (const [raw, want] of [
    ["applySearchFast", "applySearchFast"],
    ["voice.js:1:2345678", "voice.js:1:2345678"],
    ["send-owner@example.com", "send-owner_example.com"],
    ["https://chat.example/archives?token=xoxb-123", "https://chat.example/archives_token_xoxb-123"],
    ["thread-C0123ABCDEF", "thread-_"],
    ["m-1732600000.123456", "m-_._"],
    ["x".repeat(200), "x".repeat(96)],
  ]) {
    assert.equal(SHELL.codeName(raw), want, raw);
  }
  // The server's own word list (src/client_debug.rs), case for case.
  for (const id of ["C0123ABCDEF", "1732600000", "m123456", "0123456789abcdef0123"]) assert.equal(SHELL.looksLikeId(id), true, id);
  for (const word of ["UTF8", "renderRows2", "ABCDEFGH", "abc12345"]) assert.equal(SHELL.looksLikeId(word), false, word);
  assert.equal(SHELL.srcName("https://h/assets/voice.0123456789abcdef0123456789abcdef.js?token=x#y", 12), "voice.js:12");
  assert.equal(SHELL.invokerName("https://h/a.js?token=x"), "a.js");
  assert.equal(SHELL.invokerName("Window.requestAnimationFrame"), "Window.requestAnimationFrame");
});

// --- the page's exact bodies, for the real router (`tests/logging.rs`) -------------------------------

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const CHANNEL = await moduleForTests("src/view/channel/index.ts");
const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "fixtures", "client_debug_reports.json");

/** The freeze report and scroll trace the page's own modules send for one scripted stall. */
async function pageBodies() {
  const { state, debug, tick, spanAt, settle } = makeDebug({ held: true, sizes: { s: 812, d: 140 } });
  debug.start();
  state.profilers[0].trace = {
    resources: ["https://host.example/assets/voice.0123456789abcdef0123456789abcdef.js?token=x"],
    frames: [{ name: "onPageKey", resourceId: 0, line: 1, column: 40 }, { name: "applySearchFast", resourceId: 0, line: 1, column: 900 }],
    stacks: [{ frameId: 0 }, { frameId: 1, parentId: 0 }],
    samples: [{ timestamp: 1010, stackId: 1 }, { timestamp: 1026, stackId: 1 }, { timestamp: 1042, stackId: 0 }, { timestamp: 1058 }],
  };
  spanAt("rows", 900, 950, 3);
  state.now = 1000;
  debug.span("key", () => { spanAt("search-fast", 1005, 1240, 7); state.now = 1250; });
  state.observers.get("event")([{ name: "keydown", startTime: 998, duration: 280, processingStart: 1000, processingEnd: 1250 }]);
  state.observers.get("long-animation-frame")([{
    startTime: 1000, duration: 280, blockingDuration: 230, renderStart: 1260, styleAndLayoutStart: 1265,
    scripts: [{ duration: 250, invokerType: "event-listener", invoker: "DOCUMENT.onkeydown", sourceURL: "https://host.example/assets/voice.0123456789abcdef0123456789abcdef.js", sourceFunctionName: "onPageKey", sourceCharPosition: 4567, forcedStyleAndLayoutDuration: 12 }],
  }]);
  // The freeze watch over the same debug mode, posting what a real page posts.
  const sent = [];
  const watchTimers = [];
  let beatRun = null;
  const watch = SHELL.createFreezeWatch({
    now: () => state.now,
    clock: { setTimeout: (run, ms) => { const h = { run, at: state.now + ms }; watchTimers.push(h); return h; }, clearTimeout: () => {} },
    every: (_ms, run) => { beatRun = run; return () => { beatRun = null; }; },
    storage: { getItem: () => null, setItem: () => {}, removeItem: () => {} },
    documentTarget: () => null,
    hidden: () => false,
    observe: (type, note) => { state.watchObservers = { ...(state.watchObservers || {}), [type]: note }; return () => {}; },
    state: () => ({ view: "discord", screen: "main", channelView: "flat", twoColumns: false, reading: "idle", windowed: true }),
    replaysWaiting: () => 0,
    send: async (text) => { sent.push(JSON.parse(text)); return true; },
    canPost: () => true,
    clientDebug: debug,
  });
  state.now = 500;
  watch.start();
  for (const at of [750, 1000]) {
    state.now = at;
    beatRun();
  }
  state.now = 1250;
  beatRun();
  state.watchObservers["long-animation-frame"]([{
    startTime: 1000, duration: 280, blockingDuration: 230, renderStart: 1260, styleAndLayoutStart: 1265,
    scripts: [{ duration: 250, invokerType: "event-listener", invoker: "DOCUMENT.onkeydown", sourceURL: "https://host.example/assets/voice.0123456789abcdef0123456789abcdef.js", sourceFunctionName: "onPageKey", sourceCharPosition: 4567, forcedStyleAndLayoutDuration: 12 }],
  }]);
  tick(1300);
  await settle();
  state.now = 1500;
  beatRun();
  await settle();
  // After the report went the period started over; the minute the trace sends has its own work.
  spanAt("scroll-follow", 1600, 1612, 1);
  spanAt("rows", 1700, 1760, 4);
  const trace = CHANNEL.createScrollTrace({
    lookup: () => null, activeElement: () => null, viewport: () => null, anchor: () => null,
    elementPrototype: () => ({}), now: () => 0, debugSummary: () => debug.summary(),
  });
  return { freeze_report: sent[0], scroll_trace: JSON.parse(trace.report()) };
}

test("#2276: the page's exact freeze report and scroll trace bodies are the router test's fixture", async () => {
  const bodies = await pageBodies();
  assert.ok(bodies.freeze_report, "the freeze watch posted nothing");
  const loaf = bodies.freeze_report.events.find((one) => one.kind === "loaf");
  assert.equal(loaf.debug.profile.samples, 4, "the stall's profile did not ride its event");
  assert.ok(bodies.freeze_report.debug.caps.profiler);
  assert.ok(bodies.scroll_trace.debug.spans.length > 0);
  const text = `${JSON.stringify(bodies, null, 2)}\n`;
  if (process.env.VIBE_TALK_UPDATE_FIXTURES === "1") writeFileSync(FIXTURE, text);
  assert.equal(readFileSync(FIXTURE, "utf8"), text,
    "tests/fixtures/client_debug_reports.json is not what the page sends: VIBE_TALK_UPDATE_FIXTURES=1 node --test tests/js/view/shell-clientdebug.test.mjs");
});
