// Tests for web/src/view/catchup/ on its own (`catchup-side-agent`, slice 1): the Discuss
// view module held directly with no page around it, in the style of documents.test.mjs.
//   1. construction is inert; opening POSTs {thread_id, view} for the channel, Main and
//      thread scopes, and says "Reading…" before the answer lands;
//   2. the model text is drawn as text only (markup in it stays inert), the footer names
//      the covered count and links each covered message to onJump's ref;
//   3. Mark read / Mark unread call the Done acts with exactly markable_ids; no covered
//      messages hides the footer; an unmarkable remainder is footnoted;
//   4. the stamp line names tokens where reported and chars where not;
//   5. a late answer after a scope change (or a close) is ignored; an error offers Retry.
// Deterministic: injected promises only, no network and no timers.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { moduleForTests } from "../../../web/build.mjs";

const VIEW = await moduleForTests("src/view/catchup/index.ts");
const {
  createDiscussView, createDiscussPanel, formatStampLine, DISCUSS_UNAVAILABLE_HINT,
  DISCUSS_BINDING_RULES, WIDEN_OR_SIDEWAYS_ACTION, discussScopeKey, isSameDiscussScope,
  linkDecision, normalizeDiscussScope, viewChangeDecision,
  applyStrikes, citationIdsOf, hasWord, STRUCK_BULLET_CLASS, STRUCK_SECTION_CLASS, FRESH_CLASS,
  createStrikeIndex, DELETED_CITE_CLASS,
  hoursSinceStart, countForHours, boundForHours, hoursFloor1, formatHoursFloor1, summaryTimeLabel, withoutTimestampCitations, uncoveredInScope,
  summarySearchWords, summarySearchMatches, summarySearchRows, summarySearchHits,
  splitLatestPair, timelineEntryKey, releasedTimeline, timelineKeyAct, timelineNewHint, createTimelineSplit,
  TIMELINE_MIN_PX, TIMELINE_COLLAPSE_BELOW_PX, TIMELINE_STEP_PX,
} = VIEW;

// ---- fakes -----------------------------------------------------------------------------------

function fakeElement(tagName = "div") {
  const styleProps = {};
  const el = {
    tagName: tagName.toUpperCase(),
    children: [],
    listeners: {},
    attributes: {},
    style: {
      props: styleProps,
      setProperty(name, value) { styleProps[name] = String(value); },
    },
    hidden: false,
    disabled: false,
    className: "",
    type: "",
    innerHTMLSet: false,
    focusCalls: [],
    focus(options) { el.focusCalls.push(options); },
    appendChild(kid) { el.children.push(kid); kid.parentNode = el; return kid; },
    append(...kids) { for (const kid of kids) el.appendChild(kid); },
    prepend(...kids) { el.children.unshift(...kids); for (const kid of kids) kid.parentNode = el; },
    removeChild(kid) { el.children.splice(el.children.indexOf(kid), 1); return kid; },
    replaceChildren(...kids) { el.children = []; for (const kid of kids) el.appendChild(kid); },
    addEventListener(type, listener) { (el.listeners[type] ||= []).push(listener); },
    removeEventListener(type, listener) { el.listeners[type] = (el.listeners[type] || []).filter((one) => one !== listener); },
    setAttribute(name, value) { el.attributes[name] = String(value); },
    getAttribute(name) { return name in el.attributes ? el.attributes[name] : null; },
    hasAttribute(name) { return name in el.attributes; },
    removeAttribute(name) { delete el.attributes[name]; },
  };
  let textContent = "";
  let innerHTML = "";
  Object.defineProperties(el, {
    textContent: {
      get: () => textContent + el.children.map((kid) => kid?.textContent ?? "").join(""),
      set: (value) => { textContent = String(value ?? ""); if (value) el.children = []; },
    },
    innerHTML: {
      get: () => innerHTML,
      set: (value) => { el.innerHTMLSet = true; innerHTML = String(value ?? ""); },
    },
  });
  return el;
}

function click(el) {
  const ev = { target: el, preventDefault() {}, stopPropagation() {} };
  for (const listener of [...(el.listeners.click || [])]) listener(ev);
}

function fireInput(el) {
  const ev = { target: el, preventDefault() {}, stopPropagation() {} };
  for (const listener of [...(el.listeners.input || [])]) listener(ev);
}

// Open, let the preview land, then press Summarize: the only way anything is asked.
async function openAndSummarize(view, fx) {
  view.open();
  await flush();
  click(fx.els["discuss-summarize"]);
}

async function flush() {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const IDS = [
  "discuss-open", "discuss-menu-open", "discuss-panel", "discuss-scope", "discuss-close", "discuss-status",
  "discuss-answer", "discuss-footer", "discuss-covers", "discuss-links", "discuss-processed",
  "discuss-mark-read", "discuss-mark-unread", "discuss-unmarkable",
  "discuss-stamp", "discuss-retry", "discuss-terms", "discuss-terms-summary", "discuss-terms-list",
  "discuss-stale", "discuss-stale-text", "discuss-update", "discuss-new-summary", "discuss-timeline", "discuss-timeline-head", "discuss-current-covers", "discuss-current-head", "discuss-current-section",
  "discuss-followup", "discuss-send", "discuss-uncovered", "discuss-uncovered-go", "discuss-uncovered-text",
  "discuss-actions", "discuss-resize", "discuss-head", "discuss-title",
  "discuss-search", "discuss-search-toggle", "discuss-search-input", "discuss-search-results", "discuss-search-status",
  "discuss-dock-toggle", "discuss-full-toggle", "screen-main",
  "discuss-summarize", "discuss-count", "discuss-since", "discuss-hours", "discuss-hours-unit",
  // `#1531 discuss-top-bar-slim-fixed`: no #discuss-read in the top bar; the
  // bottom toolbar's #read-aloud (with #read-steer as its standing strip).
  "read-aloud", "read-steer",
];

/** Fire a non-click listener with a plain event carrying `props`. */
function fire(el, type, props = {}) {
  const ev = { target: el, preventDefault() {}, stopPropagation() {}, ...props };
  for (const listener of [...(el.listeners[type] || [])]) listener(ev);
}

// The preview's unread times: two local clock times the bound links over.
const T1 = new Date(2026, 8, 20, 9, 51).getTime();
const T2 = new Date(2026, 8, 20, 9, 52).getTime();

function preview(overrides = {}) {
  return {
    window: { count: 12, unread_count: 2 },
    unread: [
      { id: "m1", at_ms: T1, label: "09:51 · Ada" },
      { id: "m2", at_ms: T2, label: "09:52 · Ben" },
    ],
    ...overrides,
  };
}

function answer(overrides = {}) {
  return {
    window: { count: 12, unread_count: 2 },
    unread_ids: ["m1", "m2"],
    markable_ids: ["m1", "m2"],
    footer_links: [
      { id: "m1", label: "09:51 · Ada", thread_id: null },
      { id: "m2", label: "09:52 · Ben", thread_id: "t9" },
    ],
    text: "Two things happened.",
    backend: "fake",
    model: "fake-model",
    stamp: {
      context_ms: 120, model_ms: 3900, first_token_ms: null, total_ms: 4050,
      input_tokens: null, output_tokens: null, input_chars: 51234, output_chars: 812,
    },
    ...overrides,
  };
}

function fixture({ selection = { view: "flat", threadId: null, thread: null }, respond, previewBody, previewBodies, nowMs } = {}) {
  const els = {};
  for (const id of IDS) els[id] = fakeElement(id.includes("mark") || id.endsWith("open") || id.endsWith("close") || id.endsWith("retry") ? "button" : "div");
  els["discuss-panel"].hidden = true;
  els["discuss-footer"].hidden = true;
  els["discuss-retry"].hidden = true;
  els["read-steer"].hidden = true;
  const calls = { posts: [], previews: [], records: [], followups: [], updates: [], newSummaries: [], dismiss: [], restore: [], jumps: [], errors: [], statuses: [], templates: [], bound: [], history: [] };
  // `#1344`: the device's kept layout, in memory; the clamp mirrors the device setting's.
  const kept = { width: null, keeps: [], sheetFull: false, sheetKeeps: [], mode: "overlay", modeKeeps: [], dockedHeight: null, dockedKeeps: [] };
  const clampW = (value) => {
    const px = Math.round(Number(value));
    return Number.isFinite(px) ? Math.min(960, Math.max(320, px)) : 560;
  };
  let currentSelection = selection;
  let inDocs = false;
  const deps = {
    byId: (id) => {
      const found = els[id];
      if (!found) throw new Error(`no element ${id}`);
      return found;
    },
    catchupPost: (channelId, body) => {
      calls.posts.push({ channelId, body });
      return respond ? respond(channelId, body) : Promise.resolve(answer());
    },
    catchupPreview: (channelId, scope) => {
      calls.previews.push({ channelId, scope });
      if (previewBodies) return Promise.resolve(previewBodies[Math.min(calls.previews.length - 1, previewBodies.length - 1)]);
      return Promise.resolve(previewBody ?? preview());
    },
    ...(nowMs === undefined ? {} : { now: () => nowMs }),
    catchupRecord: (channelId, scope) => {
      calls.records.push({ channelId, scope });
      return Promise.resolve({ exists: false, current_summary: null, timeline: [], staleness: null });
    },
    catchupUpdate: (channelId, scope) => {
      calls.updates.push({ channelId, scope });
      return Promise.resolve(answer({ fresh_ids: ["m2"], text: "old and new" }));
    },
    catchupNewSummary: (channelId, scope) => {
      calls.newSummaries.push({ channelId, scope });
      return Promise.resolve(answer({ text: "fresh point", unread_ids: ["m2"], footer_links: [{ id: "m2", label: "09:52 · Ben", thread_id: null }] }));
    },
    catchupFollowup: (channelId, scope, text) => {
      calls.followups.push({ channelId, scope, text });
      return Promise.resolve({ you: { speaker: "you", text, text_html: "", at_ms: T2 }, agent: { speaker: "agent", text: "followed up", text_html: "", at_ms: T2 }, stamp: null });
    },
    selectedChannelId: () => "chan-1",
    selection: () => currentSelection,
    inDocumentsView: () => inDocs,
    serviceName: () => "discord",
    channelLabel: (id) => `Channel ${id}`,
    onJump: (ref) => calls.jumps.push(ref),
    navigateToBound: (scope, ref) => calls.bound.push({ scope, ref }),
    pushDiscussState: (state) => calls.history.push(state),
    dismissMessages: async (body) => { calls.dismiss.push(body); },
    restoreMessages: async (ids) => { calls.restore.push(ids); },
    showError: (text) => calls.errors.push(text),
    setStatus: (text) => calls.statuses.push(text),
    storedWidth: () => (kept.width === null ? 560 : kept.width),
    keepWidth: (px) => { kept.width = px; kept.keeps.push(px); },
    clampWidth: clampW,
    defaultWidth: 560,
    storedSheetFull: () => kept.sheetFull,
    keepSheetFull: (full) => { kept.sheetFull = full; kept.sheetKeeps.push(full); },
    storedMode: () => kept.mode,
    keepMode: (mode) => { kept.mode = mode; kept.modeKeeps.push(mode); },
    storedDockedHeight: () => (kept.dockedHeight === null ? 50 : kept.dockedHeight),
    keepDockedHeight: (pct) => { kept.dockedHeight = pct; kept.dockedKeeps.push(pct); },
  };
  const savedDocument = globalThis.document;
  globalThis.document = {
    documentElement: fakeElement("html"),
    createTextNode: (text) => ({ nodeType: 3, textContent: String(text), children: [] }),
    createElement: (tag) => {
      if (tag !== "template") return fakeElement(tag);
      // The dom sink's one parser, faked: it records what it was handed and
      // answers one parsed child for it.
      const template = {
        _html: "",
        content: null,
        set innerHTML(value) {
          this._html = String(value);
          const nodes = [fakeElement("span")];
          // Anchors in the handed markup, as elements with their attributes:
          // a drawn link's own attributes are observable behaviour (`#1911`).
          const anchorsIn = (markup) => [...markup.matchAll(/<a\s+([^>]*)>([^<]*)<\/a>/g)].map((match) => {
            const anchor = fakeElement("a");
            for (const attr of match[1].matchAll(/([\w-]+)="([^"]*)"/g)) anchor.setAttribute(attr[1], attr[2]);
            anchor.textContent = match[2];
            return anchor;
          });
          // `#2265`: a list item as an `li` holding its own anchors, so a
          // drawn bullet's citations are there when the draw indexes them.
          for (const item of this._html.matchAll(/<li>(.*?)<\/li>/g)) {
            const li = fakeElement("li");
            for (const anchor of anchorsIn(item[1])) li.appendChild(anchor);
            nodes.push(li);
          }
          for (const anchor of anchorsIn(this._html.replace(/<li>.*?<\/li>/g, ""))) nodes.push(anchor);
          this.content = { childNodes: nodes };
        },
        get innerHTML() { return this._html; },
      };
      calls.templates.push(template);
      return template;
    },
  };
  return {
    els, calls, deps, kept,
    setSelection(next) { currentSelection = next; },
    setInDocuments(next) { inDocs = next; },
    restore() {
      if (savedDocument === undefined) delete globalThis.document;
      else globalThis.document = savedDocument;
    },
  };
}

// ---- 1. construction, the request, Reading ----------------------------------------------------

test("catchup: construction is inert (nothing looked up, nothing posted)", () => {
  const view = createDiscussView({
    byId: () => { throw new Error("looked up at construction"); },
    catchupPost: () => { throw new Error("posted at construction"); },
    selectedChannelId: () => { throw new Error("read at construction"); },
    selection: () => { throw new Error("read at construction"); },
    onJump: () => {},
    dismissMessages: async () => {},
    restoreMessages: async () => {},
  });
  assert.equal(typeof view.mountControls, "function");
  assert.equal(typeof view.open, "function");
});

test("catchup: open asks nothing; Summarize posts the bound and says Reading… first", async () => {
  const pending = deferred();
  const fx = fixture({ respond: () => pending.promise });
  try {
    const view = createDiscussView(fx.deps);
    const mounted = view.mountControls();
    view.open();
    assert.equal(fx.els["discuss-panel"].hidden, false, "the panel did not open");
    assert.equal(view.isOpen(), true);
    await flush();
    assert.equal(fx.calls.previews.length, 1, "open did not read the preview");
    assert.deepEqual(fx.calls.posts, [], "open asked the model");
    assert.equal(fx.els["discuss-status"].textContent, "2 unread ready.");
    assert.equal(fx.els["discuss-summarize"].textContent, "Summarize", "the button repeated the bound");
    assert.equal(fx.els["discuss-count"].value, "2");
    assert.equal(fx.els["discuss-since"].value, "09:51");
    assert.equal(fx.els["discuss-scope"].textContent, "Channel chan-1");
    click(fx.els["discuss-summarize"]);
    assert.equal(fx.els["discuss-status"].textContent, "Reading…", "no Reading line before the answer");
    assert.deepEqual(fx.calls.posts, [{ channelId: "chan-1", body: { thread_id: null, view: "flat", since_ms: T1 } }]);
    pending.resolve(answer());
    await flush();
    assert.equal(fx.els["discuss-status"].textContent, "Read 2 unread of 12");
    mounted.unmount();
  } finally { fx.restore(); }
});

test("catchup: the bound's ends move together (N to time, time to N)", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const count = fx.els["discuss-count"];
    const since = fx.els["discuss-since"];
    count.value = "1";
    fireInput(count);
    assert.equal(since.value, "09:52", "N=1 did not move the time to the newest unread");
    assert.equal(fx.els["discuss-summarize"].textContent, "Summarize");
    since.value = "09:51";
    fireInput(since);
    assert.equal(count.value, "2", "the time did not move N back over both unread");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.deepEqual(fx.calls.posts.at(-1).body, { thread_id: null, view: "flat", since_ms: T1 });
  } finally { fx.restore(); }
});

test("catchup: hours helpers link K, N and since with an injected now (#1524)", () => {
  const now = new Date(2026, 8, 20, 12, 0).getTime();
  const unread = [
    { id: "a", at_ms: now - (5 * 60 + 30) * 60_000, label: "a" },
    { id: "b", at_ms: now - (2 * 60 + 15) * 60_000, label: "b" },
    { id: "c", at_ms: now - 30 * 60_000, label: "c" },
  ];
  // Rounding: K is whole hours from the start to now, rounded up.
  assert.equal(hoursSinceStart(unread[0].at_ms, now), 6);
  assert.equal(hoursSinceStart(now - 2 * 3_600_000, now), 2);
  assert.equal(hoursSinceStart(null, now), 1);
  assert.equal(countForHours(unread, 2, now), 1);
  assert.equal(countForHours(unread, 3, now), 2);
  assert.deepEqual(boundForHours(unread, 3, now), { count: 2, sinceMs: now - 3 * 3_600_000, hours: 3 });
  // `#1817`: K past the oldest is kept exactly as typed (no 99 cap, no
  // restating to the oldest); the count simply covers everything in range.
  assert.deepEqual(boundForHours(unread, 99, now), { count: 3, sinceMs: now - 99 * 3_600_000, hours: 99 });
  assert.deepEqual(boundForHours(unread, 150, now), { count: 3, sinceMs: now - 150 * 3_600_000, hours: 150 });
  assert.deepEqual(boundForHours([], 4, now), { count: 0, sinceMs: null, hours: 4 });
  // `#1817`: a window covering no unread is valid and accepted (count 0),
  // and fractional hours are used as typed.
  assert.deepEqual(boundForHours(unread, 0.1, now), { count: 0, sinceMs: now - 0.1 * 3_600_000, hours: 0.1 });
  assert.equal(countForHours(unread, 2.5, now), 2);
  // `#1817`: the dependent hours floor to one decimal (6.81 -> 6.8).
  assert.equal(hoursFloor1(now - 6.81 * 3_600_000, now), 6.8);
  assert.equal(formatHoursFloor1(now - 6.81 * 3_600_000, now), "6.8");
  assert.equal(formatHoursFloor1(unread[0].at_ms, now), "5.5");
});

test("catchup: the hours end moves together with N and since (#1524)", async () => {
  const now = new Date(2026, 8, 20, 12, 0).getTime();
  const unread = [
    { id: "a", at_ms: now - (5 * 60 + 30) * 60_000, label: "06:30 · A" },
    { id: "b", at_ms: now - (2 * 60 + 15) * 60_000, label: "09:45 · B" },
    { id: "c", at_ms: now - 30 * 60_000, label: "11:30 · C" },
  ];
  const fx = fixture({ nowMs: now, previewBody: { window: { count: 12, unread_count: 3 }, unread } });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const count = fx.els["discuss-count"];
    const hours = fx.els["discuss-hours"];
    const since = fx.els["discuss-since"];
    assert.equal(count.value, "3");
    assert.equal(hours.value, "5.5", "all unread did not show floor-1-decimal hours to the oldest");
    assert.equal(since.value, "06:30");
    // Editing K moves the start to now-K, and N and HH:MM follow.
    hours.value = "3";
    fireInput(hours);
    assert.equal(count.value, "2");
    assert.equal(since.value, "09:00");
    assert.equal(hours.value, "3", "editing K rewrote K to another value");
    // Editing N updates K (floored to one decimal from that message to now).
    count.value = "1";
    fireInput(count);
    assert.equal(hours.value, "0.5");
    assert.equal(since.value, "11:30");
    // Editing HH:MM updates K.
    since.value = "06:30";
    fireInput(since);
    assert.equal(count.value, "3");
    assert.equal(hours.value, "5.5");
    // `#1817`: K past the oldest is kept as typed, not clamped or restated.
    hours.value = "99";
    fireInput(hours);
    assert.equal(count.value, "3");
    assert.equal(hours.value, "99");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.calls.posts.at(-1).body.since_ms, now - 99 * 3_600_000);
  } finally { fx.restore(); }
});

test("catchup: past two days the bound reads in days, whole from ten, and the field is as wide as its value (#2244)", async () => {
  const now = new Date(2026, 8, 20, 12, 0).getTime();
  const MIN = 60_000;
  const HOUR = 60 * MIN;
  // [oldest unread age, field text, unit]: minutes, hours, just past two days, ~42 days, ~417 days,
  // and the years-old screenshot fixture's 37920.6 hours.
  const cases = [
    [30 * MIN, "0.5", "hours"],
    [5.5 * HOUR, "5.5", "hours"],
    [47.9 * HOUR, "47.9", "hours"],
    [60 * HOUR, "2.5", "days"],
    [(42 * 24 + 3) * HOUR, "42", "days"],
    [(417 * 24 + 23) * HOUR, "417", "days"],
    [37920.6 * HOUR, "1580", "days"],
  ];
  for (const [age, text, unit] of cases) {
    const unread = [{ id: "a", at_ms: now - age, label: "A" }, { id: "b", at_ms: now - 10 * MIN, label: "B" }];
    const fx = fixture({ nowMs: now, previewBody: { window: { count: 12, unread_count: 2 }, unread } });
    try {
      const view = createDiscussView(fx.deps);
      view.mountControls();
      view.open();
      await flush();
      const hours = fx.els["discuss-hours"];
      assert.equal(hours.value, text, `an oldest unread ${age / HOUR} h old shows ${hours.value}`);
      assert.equal(fx.els["discuss-hours-unit"].textContent, unit, `${text} is not said in ${unit}`);
      assert.equal(hours.getAttribute("aria-label"), unit === "days" ? "Days since the bound" : "Hours since the bound");
      assert.equal(hours.style.props["--discuss-hours-chars"], String(text.length), "the field is not sized to its value");
    } finally { fx.restore(); }
  }
});

test("catchup: typed in days or in hours, the bound is the same one (#2244)", async () => {
  const now = new Date(2026, 8, 20, 12, 0).getTime();
  const HOUR = 3_600_000;
  const unread = [
    { id: "a", at_ms: now - 42 * 24 * HOUR, label: "A" },
    { id: "b", at_ms: now - 9 * 24 * HOUR, label: "B" },
    { id: "c", at_ms: now - 2 * HOUR, label: "C" },
  ];
  const fx = fixture({ nowMs: now, previewBody: { window: { count: 12, unread_count: 3 }, unread } });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const count = fx.els["discuss-count"];
    const hours = fx.els["discuss-hours"];
    assert.equal(hours.value, "42");
    assert.equal(fx.els["discuss-hours-unit"].textContent, "days");
    // Typed in the unit shown (days): 10 days takes b and c, and the typed text stays.
    hours.value = "10";
    fireInput(hours);
    assert.equal(count.value, "2", "10 days did not reach the 9-day-old message");
    assert.equal(hours.value, "10", "the typed days were rewritten");
    assert.equal(fx.els["discuss-hours-unit"].textContent, "days", "the unit flipped under the reader's value");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.calls.posts.at(-1).body.since_ms, now - 10 * 24 * HOUR, "10 days is not 240 hours back");
    // The count end owns it again: the dependent end restates in its own unit.
    count.value = "1";
    fireInput(count);
    assert.equal(hours.value, "2.0", "one message 2 h old is not 2.0 hours");
    assert.equal(fx.els["discuss-hours-unit"].textContent, "hours");
    // Typed in hours now: 240 hours is the same bound as 10 days.
    hours.value = "240";
    fireInput(hours);
    assert.equal(count.value, "2");
    assert.equal(fx.els["discuss-hours-unit"].textContent, "hours", "the unit flipped under the reader's value");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.calls.posts.at(-1).body.since_ms, now - 240 * HOUR, "240 hours is not the bound 10 days set");
  } finally { fx.restore(); }
});

test("catchup: typing the bound is free — no clamp, snap or rewrite (#1817)", async () => {
  const now = new Date(2026, 8, 20, 12, 0).getTime();
  const unread = [
    { id: "a", at_ms: now - (5 * 60 + 30) * 60_000, label: "06:30 · A" },
    { id: "b", at_ms: now - (2 * 60 + 15) * 60_000, label: "09:45 · B" },
    { id: "c", at_ms: now - 30 * 60_000, label: "11:30 · C" },
  ];
  const fx = fixture({ nowMs: now, previewBody: { window: { count: 12, unread_count: 3 }, unread } });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const count = fx.els["discuss-count"];
    const hours = fx.els["discuss-hours"];
    // Typing "1" gives 1 (the old feedback loop rewrote it to 2).
    hours.value = "1";
    fireInput(hours);
    assert.equal(hours.value, "1", "typing 1 in hours was rewritten");
    assert.equal(count.value, "1", "the count did not follow the typed hour");
    // Typing past every cap gives the typed value: no 99 cap.
    hours.value = "150";
    fireInput(hours);
    assert.equal(hours.value, "150", "typing 150 in hours was capped");
    assert.equal(count.value, "3");
    // Typing a count beyond the held unread keeps the typed count; the
    // dependent hours floor to one decimal from the oldest in the bound.
    count.value = "150";
    fireInput(count);
    assert.equal(count.value, "150", "typing 150 in count was capped");
    assert.equal(hours.value, "5.5", "dependent hours were not floored to one decimal");
    // Hours covering no messages are accepted: the field keeps the typed
    // value, the count follows as 0, and Summarize offers nothing.
    hours.value = "0.1";
    fireInput(hours);
    assert.equal(hours.value, "0.1", "empty-range hours were snapped away");
    assert.equal(count.value, "0", "the count did not follow empty-range hours");
    assert.equal(fx.els["discuss-summarize"].disabled, true, "Summarize stayed live over an empty bound");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.deepEqual(fx.calls.posts, [], "Summarize posted over an empty bound");
  } finally { fx.restore(); }
});

test("catchup: a thread preview polls while older pages fetch, and the count caps at the whole thread (#1761)", async () => {
  const fx = fixture({
    selection: { view: "thread", threadId: "t1", thread: { title: "Long thread" } },
    previewBodies: [
      preview({ window: { count: 99, unread_count: 2, truncated: true } }),
      preview({ window: { count: 198, unread_count: 2, truncated: true } }),
      preview({ window: { count: 211, unread_count: 2, truncated: false } }),
    ],
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    await flush();
    assert.equal(fx.calls.previews.length, 3, "the preview did not poll the fetch");
    assert.equal(fx.els["discuss-status"].textContent, "2 unread ready.");
    // `#1817`: the count no longer carries a cap; typing is free.
    assert.ok(!fx.els["discuss-count"].max, "the count still carries a max cap");
  } finally { fx.restore(); }
});

test("catchup: a blocked thread fetch says so and stops polling (#1761)", async () => {
  const fx = fixture({
    selection: { view: "thread", threadId: "t1", thread: { title: "Long thread" } },
    previewBodies: [preview({ window: { count: 99, unread_count: 2, truncated: true, fetch_blocked: true } })],
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    await flush();
    assert.equal(fx.calls.previews.length, 1, "a blocked fetch polled on");
    assert.match(fx.els["discuss-status"].textContent, /could not be fetched/);
  } finally { fx.restore(); }
});

test("catchup: while a request is in flight nothing starts another", async () => {
  const pending = deferred();
  const fx = fixture({ respond: () => pending.promise });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    assert.equal(fx.calls.posts.length, 1);
    assert.equal(fx.els["discuss-summarize"].disabled, true, "Summarize stayed live while pending");
    assert.equal(fx.els["discuss-open"].getAttribute("aria-disabled"), "true", "the bar button stayed live while pending");
    click(fx.els["discuss-summarize"]);
    click(fx.els["discuss-open"]);
    click(fx.els["discuss-menu-open"]);
    assert.equal(fx.calls.posts.length, 1, "a second request started while one was in flight");
    assert.equal(view.isOpen(), true, "a press closed the panel mid-request");
    pending.resolve(answer());
    await flush();
    assert.equal(fx.els["discuss-summarize"].disabled, false, "Summarize did not come back");
    assert.equal(fx.els["discuss-open"].getAttribute("aria-disabled"), null);
  } finally { fx.restore(); }
});

test("catchup: Main posts view main; a thread posts its thread_id", async () => {
  const fx = fixture({ selection: { view: "main", threadId: null, thread: null } });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.deepEqual(fx.calls.posts.at(-1).body, { thread_id: null, view: "main", since_ms: T1 });
    fx.setSelection({ view: "thread", threadId: "t1", thread: { title: "Deploy thread" } });
    await openAndSummarize(view, fx);
    await flush();
    assert.deepEqual(fx.calls.posts.at(-1).body, { thread_id: "t1", view: "flat", since_ms: T1 });
    assert.equal(fx.els["discuss-scope"].textContent, "Deploy thread");
  } finally { fx.restore(); }
});

test("catchup: the bar button opens and closes; entering a thread re-scopes Discuss to it (#1811)", async () => {
  // #1811: an open channel Discuss entered into a thread must count and
  // summarize that thread alone, titled for it; it closes only from its button.
  const fx = fixture({
    previewBodies: [
      preview(),
      preview({ window: { count: 3, unread_count: 1 }, unread: [{ id: "m9", at_ms: T2, label: "09:52 · Ben" }] }),
    ],
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    click(fx.els["discuss-open"]);
    assert.equal(view.isOpen(), true);
    assert.equal(fx.calls.posts.length, 0, "opening asked the model");
    await flush();
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: main channel");
    assert.equal(fx.els["discuss-count"].value, "2");
    fx.setSelection({ view: "thread", threadId: "t2", thread: { display_name: "foo-bar-baz", title: "Other thread" } });
    view.sync();
    await flush();
    assert.equal(view.isOpen(), true, "entering a thread closed the panel");
    assert.equal(view.boundScope().kind, "thread", "entering a thread did not rebind Discuss");
    assert.equal(fx.calls.previews.at(-1).scope.threadId, "t2", "the preview did not re-read for the thread");
    assert.equal(fx.els["discuss-scope"].textContent, "foo-bar-baz");
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread foo-bar-baz");
    assert.equal(fx.els["discuss-count"].value, "1", "the unread count stayed channel-wide");
    assert.equal(fx.els["discuss-status"].textContent, "1 unread ready.");
    click(fx.els["discuss-summarize"]);
    assert.deepEqual(fx.calls.posts.at(-1).body.thread_id, "t2", "Summarize did not scope to the thread");
    await flush();
    click(fx.els["discuss-open"]);
    assert.equal(view.isOpen(), false);
    assert.equal(fx.els["discuss-open"].getAttribute("aria-expanded"), "false");
  } finally { fx.restore(); }
});


test("catchup: A -> B -> main -> A switches Discuss windows; header and body match the scope (#1841)", async () => {
  const fx = fixture({
    selection: { view: "thread", threadId: "tA", thread: { display_name: "thread-a-slug", title: "Thread A first line" } },
    respond: (channelId, body) => Promise.resolve(answer({
      text: body.thread_id === "tA" ? "Summary of A." : body.thread_id === "tB" ? "Summary of B." : "Summary of main.",
    })),
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread thread-a-slug");
    assert.equal(fx.els["discuss-answer"].textContent, "Summary of A.");
    // Thread A -> thread B: B has no summary yet, so a blank pane with its
    // own preview and Summarize — never A's summary under B's title.
    fx.setSelection({ view: "thread", threadId: "tB", thread: { display_name: "thread-b-slug", title: "Thread B first line" } });
    view.sync();
    await flush();
    assert.equal(view.isOpen(), true, "A -> B closed the panel");
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread thread-b-slug");
    assert.equal(fx.els["discuss-answer"].textContent, "", "B showed A's summary");
    assert.equal(fx.els["discuss-status"].textContent, "2 unread ready.");
    // B gets its own summary.
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, "Summary of B.");
    // Thread -> main: main has none yet, so blank again under the main title.
    fx.setSelection({ view: "main", threadId: null, thread: null });
    view.sync();
    await flush();
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: main channel");
    assert.equal(fx.els["discuss-answer"].textContent, "", "main showed B's summary");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, "Summary of main.");
    // Back to A: A's own state returns, with no new model call.
    const postsBefore = fx.calls.posts.length;
    fx.setSelection({ view: "thread", threadId: "tA", thread: { display_name: "thread-a-slug", title: "Thread A first line" } });
    view.sync();
    await flush();
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread thread-a-slug");
    assert.equal(fx.els["discuss-answer"].textContent, "Summary of A.", "returning to A did not restore A's summary");
    assert.equal(fx.calls.posts.length, postsBefore, "returning to A asked the model again");
    // And main's state is still its own on the way back.
    fx.setSelection({ view: "main", threadId: null, thread: null });
    view.sync();
    await flush();
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: main channel");
    assert.equal(fx.els["discuss-answer"].textContent, "Summary of main.");
  } finally { fx.restore(); }
});

test("catchup: a late answer for A arriving after the switch to B is dropped; an unnamed thread never takes the channel name (#1841)", async () => {
  const pending = deferred();
  const fx = fixture({
    selection: { view: "thread", threadId: "tA", thread: { display_name: "thread-a-slug", title: "Thread A first line" } },
    respond: () => pending.promise,
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    click(fx.els["discuss-summarize"]);
    assert.equal(fx.els["discuss-status"].textContent, "Reading…");
    // Switch to B before A's answer lands; B's thread record is not named yet.
    fx.setSelection({ view: "thread", threadId: "tB", thread: null });
    view.sync();
    await flush();
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread", "an unnamed thread took the channel's name");
    pending.resolve(answer({ text: "Late answer for A." }));
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, "", "the late A answer landed in B's pane");
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread");
  } finally { fx.restore(); }
});


test("catchup: the default bound starts where the last summary in this scope ended (#1842)", async () => {
  const T3 = new Date(2026, 8, 20, 9, 53).getTime();
  const withM3 = preview({ unread: [
    { id: "m1", at_ms: T1, label: "09:51 · Ada" },
    { id: "m2", at_ms: T2, label: "09:52 · Ben" },
    { id: "m3", at_ms: T3, label: "09:53 · Cy" },
  ] });
  // Thread scope: first-ever summary starts at the thread's start; the
  // second starts exactly where the first ended.
  const fx = fixture({
    selection: THREAD_T1,
    nowMs: T2 + 90 * 60_000,
    previewBodies: [preview(), preview(), withM3],
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.deepEqual(fx.calls.posts.at(-1).body, { thread_id: "t1", view: "flat", since_ms: T1 },
      "the first-ever thread summary did not start at the thread start");
    // Away to main and back: the thread's next preview offers only what the
    // first summary did not cover, from the moment it ended.
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.sync();
    await flush();
    fx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    assert.equal(fx.els["discuss-since"].value, "09:52", "the second bound did not start where the first ended");
    assert.equal(fx.els["discuss-hours"].value, "1.5", "the hours field did not restate the bound from the last summary");
    assert.equal(fx.els["discuss-count"].value, "2");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.calls.posts.at(-1).body.since_ms, T2, "the second summary did not start exactly at the first's end");
  } finally { fx.restore(); }

  // Main scope: the same rule on the channel's own window.
  const mainFx = fixture({ nowMs: T2 + 90 * 60_000, previewBodies: [preview(), preview(), withM3] });
  try {
    const view = createDiscussView(mainFx.deps);
    view.mountControls();
    await openAndSummarize(view, mainFx);
    await flush();
    assert.equal(mainFx.calls.posts.at(-1).body.since_ms, T1, "the first main summary did not cover all unread");
    mainFx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    mainFx.setSelection({ view: "flat", threadId: null, thread: null });
    view.sync();
    await flush();
    assert.equal(mainFx.els["discuss-title"].textContent, "Discuss: main channel");
    assert.equal(mainFx.els["discuss-since"].value, "09:52", "main's second bound did not start where its first ended");
    click(mainFx.els["discuss-summarize"]);
    await flush();
    assert.equal(mainFx.calls.posts.at(-1).body.since_ms, T2);
    assert.equal(mainFx.calls.posts.at(-1).body.thread_id, null);
  } finally { mainFx.restore(); }
});

test("catchup: a scope's covered ranges survive a reload through the kept coverage (#1842)", async () => {
  const keptCoverage = new Map();
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const deps = {
      ...fx.deps,
      storedCoverage: (key) => keptCoverage.get(key) ?? null,
      keepCoverage: (key, coverage) => keptCoverage.set(key, coverage),
    };
    const view = createDiscussView(deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    const entries = [...keptCoverage.entries()];
    assert.equal(entries.length, 1, "the summary kept no coverage for its scope");
    assert.equal(entries[0][0], "chan-1:t1");
    assert.deepEqual(entries[0][1], { ranges: [{ start: T1, end: T2 }] }, "the covered range was not the summary's window");
    // A fresh view (the reload) over the same kept coverage offers the
    // thread's next bound from the last summary's end, not all unread.
    const reloadedFx = fixture({ selection: THREAD_T1 });
    try {
      const reloaded = createDiscussView({
        ...reloadedFx.deps,
        storedCoverage: (key) => keptCoverage.get(key) ?? null,
        keepCoverage: (key, coverage) => keptCoverage.set(key, coverage),
      });
      reloaded.mountControls();
      reloaded.open();
      await flush();
      assert.equal(reloadedFx.els["discuss-since"].value, "09:52", "the reloaded default did not start at the last summary's end");
    } finally { reloadedFx.restore(); }
  } finally { fx.restore(); }
});



test("catchup: a cut-short window says it covers the newest part only and what to do (#1875)", async () => {
  const fx = fixture({
    respond: () => Promise.resolve(answer({ window: { count: 99, unread_count: 2, truncated: true } })),
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(
      fx.els["discuss-status"].textContent,
      "Read 2 unread of 99. Older messages weren't fetched, so this covers the newest part only — summarize again later to include more of them.");
  } finally { fx.restore(); }
});

test("catchup: a gap in a scope's summaries shows the uncovered line and its press cleans up exactly the missed messages (#1843)", async () => {
  const T25 = new Date(2026, 8, 20, 9, 52, 30).getTime();
  const T3 = new Date(2026, 8, 20, 9, 53).getTime();
  const early = preview();
  const later = preview({ unread: [
    { id: "m1", at_ms: T1, label: "09:51 · Ada" },
    { id: "m2", at_ms: T2, label: "09:52 · Ben" },
    { id: "mx", at_ms: T25, label: "09:52 · Cy" },
    { id: "m3", at_ms: T3, label: "09:53 · Di" },
  ] });
  const fx = fixture({
    selection: THREAD_T1,
    previewBodies: [early, early, later, later, later],
    respond: (channelId, body) => Promise.resolve(answer({
      unread_ids: body.message_ids ?? (body.since_ms === T3 ? ["m3"] : body.since_ms === T25 ? ["mx"] : ["m1", "m2"]),
      markable_ids: body.message_ids ?? ["m1", "m2"],
    })),
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(fx.els["discuss-uncovered"].hidden, true, "the line showed before any summary");
    // First summary covers the early pair; the second covers only the newest.
    click(fx.els["discuss-summarize"]);
    await flush();
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.sync();
    await flush();
    fx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    fx.els["discuss-count"].value = "1";
    fireInput(fx.els["discuss-count"]);
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.calls.posts.at(-1).body.since_ms, T3, "the second summary did not take only the newest");
    // Back again: the message between the two ranges is reported, dated by
    // the first summary.
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.sync();
    await flush();
    fx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    assert.equal(fx.els["discuss-uncovered"].hidden, false, "the gap showed no uncovered line");
    assert.equal(fx.els["discuss-uncovered-text"].textContent, "1 message not included since 9/20");
    // Pressing it asks for exactly the missed message; afterwards none remain.
    click(fx.els["discuss-uncovered-go"]);
    await flush();
    assert.deepEqual(fx.calls.posts.at(-1).body.message_ids, ["mx"], "the cleanup did not name exactly the missed messages");
    assert.equal(fx.calls.posts.at(-1).body.since_ms, T25);
    assert.equal(fx.els["discuss-uncovered"].hidden, true, "the line stayed after the cleanup");
  } finally { fx.restore(); }
  // The pure rule, for a main (global) scope too.
  assert.equal(uncoveredInScope([{ id: "a", at_ms: 10, label: "" }], { ranges: [] }), null);
  assert.deepEqual(
    uncoveredInScope(
      [{ id: "a", at_ms: 15, label: "" }, { id: "b", at_ms: 25, label: "" }, { id: "c", at_ms: 5, label: "" }],
      { ranges: [{ start: 10, end: 20 }, { start: 30, end: 40 }] }),
    { ids: ["b"], firstStartMs: 10, firstMissedMs: 25 });
});

test("catchup: Documents disables Discuss with its tooltip, closes the panel, and switching back restores it (#1812)", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    assert.equal(view.isOpen(), true);
    const postsBefore = fx.calls.posts.length;
    fx.setInDocuments(true);
    view.sync();
    assert.equal(view.isOpen(), false, "switching to Documents left the panel open");
    const button = fx.els["discuss-open"];
    assert.equal(button.getAttribute("aria-disabled"), "true", "Documents did not disable the control");
    assert.equal(button.getAttribute("title"), VIEW.DISCUSS_DOCUMENTS_HINT, "Documents did not say why on the control");
    assert.equal(button.getAttribute("aria-label"), VIEW.DISCUSS_DOCUMENTS_HINT);
    const item = fx.els["discuss-menu-open"];
    assert.equal(item.getAttribute("aria-disabled"), "true", "Documents did not disable the menu Discuss");
    assert.equal(item.getAttribute("title"), VIEW.DISCUSS_DOCUMENTS_HINT);
    view.open();
    view.toggle();
    assert.equal(view.isOpen(), false, "Discuss opened in Documents");
    assert.ok(fx.calls.statuses.includes(VIEW.DISCUSS_DOCUMENTS_HINT), "Documents did not say why in place");
    assert.equal(view.restoreFromHistory({ discussOpen: true, scope: { channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" }, recordKey: "chan-1:global", snapshot: null }), false, "history restored Discuss in Documents");
    assert.equal(fx.calls.posts.length, postsBefore, "Documents asked the model");
    assert.equal(fx.calls.previews.length, 1, "Documents read a preview");
    fx.setInDocuments(false);
    view.sync();
    assert.equal(button.getAttribute("aria-disabled"), null, "switching back left the control disabled");
    assert.equal(button.getAttribute("title"), "Catch up on this channel or thread");
    view.open();
    assert.equal(view.isOpen(), true, "switching back did not restore Discuss");
    assert.equal(fx.calls.posts.length, postsBefore, "reopening after Documents asked the model again");
  } finally { fx.restore(); }
});

// ---- 2. the answer as text, the footer --------------------------------------------------------

test("catchup: model text renders as text only, markup in it inert", async () => {
  const evil = "<script>alert(1)</script><img src=x onerror=alert(2)> plain words";
  const fx = fixture({ respond: () => Promise.resolve(answer({ text: evil })) });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, evil, "the text was not set as text");
    assert.equal(fx.els["discuss-answer"].innerHTMLSet, false, "the answer was assigned as markup");
    assert.equal(fx.els["discuss-answer"].children.length, 0, "the answer gained live elements");
  } finally { fx.restore(); }
});

test("catchup: a rendered answer draws as markup; a plain one never reaches innerHTML", async () => {
  const html = "<h2>Progress</h2><p>Ada shipped it.</p>";
  const fx = fixture({ respond: () => Promise.resolve(answer({ text: "## Progress\nAda shipped it.", text_html: html })) });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.calls.templates.length, 1, "the render did not go through the one parser");
    assert.equal(fx.calls.templates[0]._html, html, "the parser was handed something else");
    assert.equal(fx.els["discuss-answer"].children.length, 1, "the parsed nodes were not drawn");
    assert.equal(fx.els["discuss-answer"].className.includes("discuss-answer--plain"), false, "rendered HTML kept the plain-text class");
  } finally { fx.restore(); }

  const plain = fixture({ respond: () => Promise.resolve(answer({ text: "<b>not markup</b>" })) });
  try {
    const view = createDiscussView(plain.deps);
    view.mountControls();
    await openAndSummarize(view, plain);
    await flush();
    assert.equal(plain.els["discuss-answer"].innerHTMLSet, false, "plain text reached innerHTML");
    assert.equal(plain.els["discuss-answer"].textContent, "<b>not markup</b>");
    assert.equal(plain.els["discuss-answer"].className.includes("discuss-answer--plain"), true, "plain fallback lost the pre-wrap class");
  } finally { plain.restore(); }
});

test("catchup: a citation tap in the answer jumps; unknown terms draw collapsed", async () => {
  const fx = fixture({
    respond: () => Promise.resolve(answer({
      unknown_terms: ["small-elk"],
      text_html: "<p>Shipped <a href=\"/?c=chan-1&v=flat&m=discord%3Achan-1%3Am1\">(09:51)</a>.</p>",
    })),
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-terms"].hidden, false, "the terms block did not show");
    assert.match(fx.els["discuss-terms-summary"].textContent, /Terms I didn't understand \(1\)/);
    assert.equal(fx.els["discuss-terms-list"].children[0].textContent, "small-elk");
    const anchor = fakeElement("a");
    anchor.setAttribute("href", "/?c=chan-1&v=flat&m=discord%3Achan-1%3Am1");
    fx.els["discuss-answer"].appendChild(anchor);
    let prevented = false;
    const ev = { target: anchor, preventDefault() { prevented = true; }, stopPropagation() {} };
    for (const listener of [...(fx.els["discuss-answer"].listeners.click || [])]) listener(ev);
    assert.equal(prevented, true, "the citation tap was not taken in-app");
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m1", thread: null }]);
  } finally { fx.restore(); }
});

test("catchup: a citation tap in the timeline jumps in this window, not a new one (#1911)", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    const anchor = fakeElement("a");
    anchor.setAttribute("href", "?c=chan-1&v=all&m=discord:chan-1:m9");
    anchor.setAttribute("target", "_blank");
    fx.els["discuss-timeline"].appendChild(anchor);
    let prevented = false;
    const ev = { target: anchor, preventDefault() { prevented = true; }, stopPropagation() {} };
    for (const listener of [...(fx.els["discuss-timeline"].listeners.click || [])]) listener(ev);
    assert.equal(prevented, true, "the timeline citation tap was not taken in-app");
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m9", thread: null }]);
  } finally { fx.restore(); }
});

test("catchup: a drawn in-app citation loses target=_blank; an external link keeps it (#1911)", async () => {
  const fx = fixture({
    respond: () => Promise.resolve(answer({
      text_html: "<p>Old <a href=\"?c=chan-1&v=all&m=discord:chan-1:m1\" target=\"_blank\">(09:51)</a>"
        + " and <a href=\"https://example.test/x\" target=\"_blank\">out</a>.</p>",
    })),
  });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    const anchors = fx.els["discuss-answer"].children.filter((kid) => kid.tagName === "A");
    assert.equal(anchors.length, 2, "the drawn links were not found");
    assert.equal(anchors[0].getAttribute("target"), null, "the in-app citation still opens a new window");
    assert.equal(anchors[1].getAttribute("target"), "_blank", "an external link lost its new-window stamp");
  } finally { fx.restore(); }
});

test("catchup: re-summarize marks the standing terms as the previous summary's until the new answer lands (#1787)", async () => {
  const second = deferred();
  let posts = 0;
  const fx = fixture({ respond: () => {
    posts += 1;
    return posts === 1 ? Promise.resolve(answer({ unknown_terms: ["old-term"] })) : second.promise;
  } });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-terms"].hidden, false);
    assert.equal(fx.els["discuss-terms"].getAttribute("data-stale"), null, "fresh terms marked stale");
    click(fx.els["discuss-summarize"]);
    assert.equal(fx.els["discuss-status"].textContent, "Reading…");
    assert.equal(fx.els["discuss-terms"].getAttribute("data-stale"), "true", "stale terms still presented as current");
    assert.match(fx.els["discuss-terms-summary"].textContent, /from the previous summary/);
    assert.equal(fx.els["discuss-terms-list"].children[0].textContent, "old-term", "the old list vanished instead of being marked");
    second.resolve(answer({ unknown_terms: ["new-term"], text: "Fresh point." }));
    await flush();
    assert.equal(fx.els["discuss-terms"].getAttribute("data-stale"), null, "new terms stayed marked stale");
    assert.equal(fx.els["discuss-terms-summary"].textContent, "Terms I didn't understand (1)");
    assert.equal(fx.els["discuss-terms-list"].children[0].textContent, "new-term");
  } finally { fx.restore(); }
});

test("catchup: a failed re-summarize restores the previous summary and its terms unmarked (#1787)", async () => {
  let posts = 0;
  const fx = fixture({ respond: () => {
    posts += 1;
    return posts === 1
      ? Promise.resolve(answer({ unknown_terms: ["old-term"] }))
      : Promise.reject(new Error("HTTP 502 chat_error: upstream slow"));
  } });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.match(fx.els["discuss-status"].textContent, /upstream slow/);
    assert.equal(fx.els["discuss-retry"].hidden, false, "no retry was offered");
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.", "the failed ask dropped the standing summary");
    assert.equal(fx.els["discuss-terms"].getAttribute("data-stale"), null, "restored terms stayed marked stale");
    assert.equal(fx.els["discuss-terms-summary"].textContent, "Terms I didn't understand (1)");
    assert.equal(fx.els["discuss-terms-list"].children[0].textContent, "old-term");
    assert.equal(fx.els["discuss-footer"].hidden, false, "the restored summary lost its footer");
  } finally { fx.restore(); }
});

test("catchup: the footer covers N and each link jumps with its ref", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-footer"].hidden, false);
    assert.equal(fx.els["discuss-covers"].textContent, "Other messages processed:");
    const items = fx.els["discuss-links"].children;
    assert.equal(items.length, 2);
    assert.equal(items[0].children[0].textContent, "09:51");
    // `#1540`: the times are comma-separated in the footer's text flow.
    assert.equal(fx.els["discuss-links"].textContent, "09:51, 09:52");
    click(items[1].children[0]);
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m2", thread: "t9" }]);
  } finally { fx.restore(); }
});

test("catchup: cited messages leave the footer; Mark still targets all processed", async () => {
  const fx = fixture({ respond: () => Promise.resolve(answer({ cited_ids: ["m1"] })) });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    const items = fx.els["discuss-links"].children;
    assert.equal(items.length, 1);
    assert.equal(items[0].children[0].textContent, "09:52");
    assert.equal(fx.els["discuss-links"].textContent, "09:52", "a lone footer time carries no comma");
    click(fx.els["discuss-mark-read"]);
    await flush();
    assert.deepEqual(fx.calls.dismiss, [{ messages: ["m1", "m2"] }]);
  } finally { fx.restore(); }
});

// ---- 3. marking -------------------------------------------------------------------------------

test("catchup: Mark read dismisses exactly markable_ids; Mark unread restores the same", async () => {
  const fx = fixture({ respond: () => Promise.resolve(answer({
    unread_ids: ["m1", "m2", "m3"],
    markable_ids: ["m1", "m2"],
  })) });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-unmarkable"].hidden, false, "the unmarkable remainder was not footnoted");
    assert.equal(fx.els["discuss-unmarkable"].textContent, "1 can't be marked here");
    click(fx.els["discuss-mark-read"]);
    await flush();
    assert.deepEqual(fx.calls.dismiss, [{ messages: ["m1", "m2"] }]);
    click(fx.els["discuss-mark-unread"]);
    await flush();
    assert.deepEqual(fx.calls.restore, [["m1", "m2"]]);
  } finally { fx.restore(); }
});

test("catchup: no covered messages hides the footer and its buttons", async () => {
  const fx = fixture({ respond: () => Promise.resolve(answer({
    window: { count: 5, unread_count: 0 },
    unread_ids: [], markable_ids: [], footer_links: [], text: "All read.",
  })) });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-status"].textContent, "Read 0 unread of 5");
    assert.equal(fx.els["discuss-footer"].hidden, true, "the footer showed with nothing covered");
    click(fx.els["discuss-mark-read"]);
    await flush();
    assert.deepEqual(fx.calls.dismiss, [], "a hidden Mark read still dismissed");
  } finally { fx.restore(); }
});

// ---- 4. the stamp line ------------------------------------------------------------------------

test("catchup: the stamp line names chars without tokens and tokens with them", async () => {
  assert.equal(
    formatStampLine({ context_ms: 120, model_ms: 3900, first_token_ms: null, total_ms: 4050, input_tokens: null, output_tokens: null, input_chars: 51234, output_chars: 812 }),
    "cache 120 ms · model 3900 ms · total 4050 ms · in 51234 chars · out 812 chars",
  );
  assert.equal(
    formatStampLine({ context_ms: 9, model_ms: 80, first_token_ms: 12, total_ms: 95, input_tokens: 1400, output_tokens: 210, input_chars: 51234, output_chars: 812 }),
    "cache 9 ms · model 80 ms · total 95 ms · in 1400 tokens · out 210 tokens",
  );
  assert.equal(formatStampLine(null), "");
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(fx.els["discuss-stamp"].hidden, false);
    assert.match(fx.els["discuss-stamp"].textContent, /in 51234 chars/);
  } finally { fx.restore(); }
});

// ---- 5. late answers and errors ---------------------------------------------------------------

test("catchup: a late channel answer after entering a thread is ignored; after widening too (#1811)", async () => {
  // #1811: entering a thread re-scopes Discuss, so a channel answer still
  // in flight for the old scope no longer lands. Returning to main
  // re-scopes the same way, so a late thread answer is ignored too.
  const pending = deferred();
  const fx = fixture({ respond: () => pending.promise });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    fx.setSelection({ view: "thread", threadId: "t7", thread: { title: "Elsewhere" } });
    view.sync();
    pending.resolve(answer());
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, "", "the stale channel answer landed in the thread scope");
    assert.equal(view.isOpen(), true);
  } finally { fx.restore(); }

  const widePending = deferred();
  const wide = fixture({
    selection: { view: "thread", threadId: "t1", thread: { title: "First thread" } },
    respond: () => widePending.promise,
  });
  try {
    const view = createDiscussView(wide.deps);
    view.mountControls();
    await openAndSummarize(view, wide);
    wide.setSelection({ view: "flat", threadId: null, thread: null });
    view.sync();
    assert.equal(view.isOpen(), true, "returning to main closed the panel");
    assert.equal(view.boundScope().kind, "global", "returning to main did not rebind to the channel");
    assert.equal(wide.els["discuss-title"].textContent, "Discuss: main channel");
    widePending.resolve(answer());
    await flush();
    assert.equal(wide.els["discuss-answer"].textContent, "", "the stale thread answer was drawn in the main scope");
  } finally { wide.restore(); }
});

test("catchup: a late answer after close is ignored", async () => {
  const pending = deferred();
  const fx = fixture({ respond: () => pending.promise });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    view.close();
    pending.resolve(answer());
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, "");
    assert.equal(fx.els["discuss-panel"].hidden, true);
  } finally { fx.restore(); }
});

test("catchup: an error is shown and Retry asks again", async () => {
  let attempts = 0;
  const fx = fixture({ respond: () => {
    attempts += 1;
    return attempts === 1 ? Promise.reject(new Error("HTTP 502 chat_error: upstream slow")) : Promise.resolve(answer());
  } });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.deepEqual(fx.calls.errors, ["HTTP 502 chat_error: upstream slow"]);
    assert.equal(fx.els["discuss-retry"].hidden, false, "no retry was offered");
    assert.match(fx.els["discuss-status"].textContent, /upstream slow/);
    click(fx.els["discuss-retry"]);
    await flush();
    assert.equal(attempts, 2, "Retry did not ask again");
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.");
    assert.equal(fx.els["discuss-retry"].hidden, true);
  } finally { fx.restore(); }
});

// ---- 6. no summarizer on this deployment ------------------------------------------------------

test("catchup: unavailable draws the button disabled with the hint and a click posts nothing", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView({ ...fx.deps, discussAvailable: () => false });
    view.mountControls();
    const button = fx.els["discuss-open"];
    assert.equal(button.getAttribute("aria-disabled"), "true", "the button is not drawn disabled");
    assert.equal(button.getAttribute("title"), DISCUSS_UNAVAILABLE_HINT);
    assert.equal(button.getAttribute("aria-label"), DISCUSS_UNAVAILABLE_HINT);
    click(button);
    await openAndSummarize(view, fx);
    await flush();
    assert.deepEqual(fx.calls.posts, [], "an unavailable Discuss still posted");
    assert.equal(fx.els["discuss-panel"].hidden, true, "an unavailable Discuss opened the panel");
    assert.equal(view.isOpen(), false);
  } finally { fx.restore(); }
});

test("catchup: a 503 summarizer refusal says the hint in the panel, with no retry", async () => {
  const refusal = new Error("HTTP 503 summarizer_not_configured: no [summaries.llm]");
  refusal.status = 503;
  refusal.code = "summarizer_not_configured";
  const fx = fixture({ respond: () => Promise.reject(refusal) });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.deepEqual(fx.calls.errors, [DISCUSS_UNAVAILABLE_HINT]);
    assert.equal(fx.els["discuss-status"].textContent, DISCUSS_UNAVAILABLE_HINT);
    assert.equal(fx.els["discuss-retry"].hidden, true, "a refusal offered a retry");
    // From then on the button is drawn unavailable too.
    assert.equal(fx.els["discuss-open"].getAttribute("aria-disabled"), "true");
  } finally { fx.restore(); }
});

// ---- 8. the saved record (discuss-transcript-persist) -------------------------------------------

function savedRecord(overrides = {}) {
  return {
    exists: true,
    current_summary: {
      view: "flat", since_ms: null,
      window: { count: 12, unread_count: 2, read_count: 10 },
      covered_ids: ["m1", "m2"], cited_ids: [], markable_ids: ["m1", "m2"],
      footer_links: [
        { id: "m1", label: "09:51 · Ada", thread_id: null },
        { id: "m2", label: "09:52 · Ben", thread_id: "t9" },
      ],
      text: "saved point", text_html: "", unknown_terms: [],
      backend: "meta-gateway", model: "fake",
      stamp: { context_ms: 5, model_ms: 900, first_token_ms: null, total_ms: 920, input_tokens: null, output_tokens: null, input_chars: 100, output_chars: 10, unknown_term_count: 0 },
      created_ms: T2,
    },
    timeline: [
      { kind: "summary", summary: { view: "flat", window: { count: 9, unread_count: 1 }, covered_ids: ["m0"], markable_ids: ["m0"], footer_links: [], text: "older point", text_html: "", unknown_terms: [], stamp: null, created_ms: T1 } },
      { kind: "turn", turn: { speaker: "you", text: "and the deploy?", text_html: "", at_ms: T2 } },
    ],
    staleness: { new_unread: 3, marked_read: 1 },
    ...overrides,
  };
}

test("catchup: reopening draws the saved record and asks nothing", async () => {
  const fx = fixture();
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(savedRecord());
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(fx.calls.records.length, 1);
    assert.deepEqual(fx.calls.posts, [], "reopening asked the model");
    assert.equal(fx.els["discuss-answer"].textContent, "saved point");
    assert.match(fx.els["discuss-status"].textContent, /Saved summary/);
    assert.equal(fx.els["discuss-footer"].hidden, false);
    assert.equal(fx.els["discuss-stale"].hidden, false, "the staleness banner did not show");
    assert.equal(fx.els["discuss-stale-text"].textContent, "3 new unread · 1 marked read since summary");
    const timeline = fx.els["discuss-timeline"].children;
    assert.equal(timeline.length, 2, "the timeline did not draw its old summary and Q/A pair");
    assert.match(timeline[1].children[0].textContent, /Follow-up question/, "the turn did not draw as a collapsible pair");
    assert.ok(timeline[1].children[0].textContent.includes("and the deploy?"), "the folded header does not carry the verbatim question");
    assert.match(timeline[1].children[1].textContent, /You\s*and the deploy\?/, "the question paragraph is not labelled You");
    assert.equal(fx.els["discuss-current-covers"].textContent, "Covers 2 messages");
    // [Update] is the incremental delta (`#1388` landing 2): one update
    // call, no full re-summarize POST.
    click(fx.els["discuss-update"]);
    await flush();
    assert.equal(fx.calls.updates.length, 1);
    assert.deepEqual(fx.calls.posts, [], "Update refetched a full summary");
  } finally { fx.restore(); }
});

test("catchup: Update marks the standing terms stale and restores them on failure (#1787)", async () => {
  const fx = fixture();
  const record = savedRecord();
  record.current_summary.unknown_terms = ["old-term"];
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(record);
  };
  const attempt = deferred();
  fx.deps.catchupUpdate = (channelId, scope) => {
    fx.calls.updates.push({ channelId, scope });
    return attempt.promise;
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(fx.els["discuss-terms"].hidden, false);
    assert.equal(fx.els["discuss-terms"].getAttribute("data-stale"), null, "fresh terms marked stale");
    click(fx.els["discuss-update"]);
    assert.equal(fx.els["discuss-status"].textContent, "Updating…");
    assert.equal(fx.els["discuss-terms"].getAttribute("data-stale"), "true", "stale terms still presented as current");
    assert.match(fx.els["discuss-terms-summary"].textContent, /from the previous summary/);
    attempt.reject(new Error("HTTP 502 chat_error: upstream slow"));
    await flush();
    assert.equal(fx.els["discuss-terms"].getAttribute("data-stale"), null, "restored terms stayed marked stale");
    assert.equal(fx.els["discuss-terms-summary"].textContent, "Terms I didn't understand (1)");
    assert.equal(fx.els["discuss-terms-list"].children[0].textContent, "old-term");
  } finally { fx.restore(); }
});

test("catchup: summary time labels name the day away from today (#1787)", () => {
  const now = new Date(2026, 10, 17, 15, 0).getTime();
  const today = summaryTimeLabel(new Date(2026, 10, 17, 9, 15).getTime(), now);
  assert.match(today, /^\d{1,2}:\d{2}\s?(AM|PM)$/, `today's label named a day: ${today}`);
  assert.equal(summaryTimeLabel(new Date(2026, 10, 16, 15, 42).getTime(), now).startsWith("Mon "), true, "yesterday kept the time alone");
  assert.equal(summaryTimeLabel(new Date(2026, 10, 11, 15, 42).getTime(), now).startsWith("Wed "), true, "six days back lost its weekday");
  assert.equal(summaryTimeLabel(new Date(2026, 10, 10, 15, 42).getTime(), now).startsWith("Nov 10, "), true, "seven days back lost its date");
  assert.equal(summaryTimeLabel(new Date(2025, 11, 31, 15, 42).getTime(), now).startsWith("Dec 31, 2025, "), true, "another year lost its year");
});

test("catchup: timeline and saved labels date a previous day's summary (#1787)", async () => {
  const now = new Date(2026, 8, 22, 12, 0).getTime();
  const fx = fixture({ nowMs: now });
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(savedRecord());
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.match(fx.els["discuss-status"].textContent, /Saved summary · Sun \d{1,2}:\d{2}\s?(AM|PM)/);
    const timeline = fx.els["discuss-timeline"].children;
    assert.match(timeline[0].children[0].textContent, /Earlier summary · Sun \d{1,2}:\d{2}\s?(AM|PM) · 1 unread/);
  } finally { fx.restore(); }
});

test("catchup: timeline pairs follow-ups, earlier summaries open headings-first, the one toggle acts (#1912, #1960)", async () => {
  const HTML = "<h2>Old Alpha</h2><p>old a</p><h2>Old Beta</h2><p>old b</p>";
  const scope = { channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" };
  const savedDocument = globalThis.document;
  globalThis.document = {
    createTextNode: (text) => ({ nodeType: 3, textContent: String(text), children: [] }),
    createElement: (tag) => {
      if (tag !== "template") return foldEl(tag);
      const template = { _html: "", content: null };
      Object.defineProperty(template, "innerHTML", {
        set(value) {
          template._html = String(value);
          const nodes = [];
          for (const match of template._html.matchAll(/<(h[1-6]|p)>([^<]*)<\/\1>/g)) nodes.push(foldEl(match[1], match[2]));
          template.content = { childNodes: nodes };
        },
        get() { return template._html; },
      });
      return template;
    },
  };
  try {
    const els = {};
    for (const id of IDS) els[id] = foldEl(id.endsWith("open") || id.endsWith("close") || id.includes("mark") ? "button" : "div");
    els["discuss-panel"].hidden = true;
    const panel = createDiscussPanel({
      byId: (id) => {
        const found = els[id];
        if (!found) throw new Error(`no element ${id}`);
        return found;
      },
      catchupPost: () => Promise.resolve(answer()),
      catchupPreview: () => Promise.resolve(preview()),
      catchupRecord: () => Promise.resolve({
        exists: true,
        current_summary: {
          window: { count: 12, unread_count: 2 }, covered_ids: ["m1", "m2"], markable_ids: ["m1", "m2"],
          cited_ids: [], footer_links: [], text: "saved point", text_html: "", unknown_terms: [], stamp: null, created_ms: T2,
        },
        timeline: [
          { kind: "summary", summary: { view: "flat", window: { count: 9, unread_count: 1 }, covered_ids: ["m0"], markable_ids: ["m0"], footer_links: [], text: "older point", text_html: HTML, unknown_terms: [], stamp: null, created_ms: T1 } },
          { kind: "turn", turn: { speaker: "you", text: "and the deploy?", text_html: "", at_ms: T2 } },
          { kind: "turn", turn: { speaker: "agent", text: "It shipped.", text_html: "", at_ms: T2 } },
        ],
        staleness: null,
      }),
      catchupFollowup: () => Promise.resolve({}),
      serviceName: () => "discord",
      onJump: () => {},
      dismissMessages: async () => {},
      restoreMessages: async () => {},
    });
    panel.mount();
    panel.openWith(scope);
    await flush();
    await flush();
    assert.equal(els["discuss-current-covers"].textContent, "Covers 2 messages");
    const timeline = els["discuss-timeline"].children;
    // The earlier summary, then ONE collapsible Q/A pair; the toggle stands
    // on the Timeline header row (`#2134`), not in a bar of its own.
    assert.equal(timeline.length, 2);
    const timelineToggle = els["discuss-timeline-head"].children.find((kid) => String(kid.className).includes("discuss-toggle"));
    assert.ok(timelineToggle, "timeline toggle missing from the header row");
    assert.equal(timelineToggle.textContent, "Expand", "timeline toggle did not start at Expand");
    assert.equal(els["discuss-timeline"].textContent.includes("Expand all"), false, "an Expand all label remains");
    const old = timeline[0];
    assert.match(old.children[0].children[0].textContent, /Earlier summary/);
    const oldBody = old.children[1];
    const oldFolds = () => oldBody.children.filter((kid) => kid.tagName === "DETAILS");
    assert.equal(oldFolds().length, 2, "earlier summary did not fold by heading");
    assert.deepEqual(oldFolds().map((fold) => fold.open), [false, false], "earlier summary did not open headings-first");
    const oldToggle = old.children[0].children.find((kid) => String(kid.className).includes("discuss-toggle"));
    assert.ok(oldToggle, "the shared toggle is missing on the earlier summary header");
    assert.equal(oldToggle.textContent, "Expand", "earlier summary toggle did not start at Expand");
    click(oldToggle);
    assert.deepEqual(oldFolds().map((fold) => fold.open), [true, true], "the earlier summary toggle did not expand its folds");
    assert.equal(oldToggle.textContent, "Collapse", "all-open did not read Collapse");
    assert.equal(old.open, true, "the earlier summary toggle closed its own row");
    click(oldToggle);
    assert.deepEqual(oldFolds().map((fold) => fold.open), [false, false], "Collapse did not close the earlier summary folds");
    assert.equal(old.open, true, "Collapse closed the earlier summary row itself");
    assert.equal(oldToggle.textContent, "Expand", "closed folds did not read Expand again");
    const pair = timeline[1];
    assert.match(pair.children[0].textContent, /Follow-up question/);
    assert.equal(pair.children[1].children.length, 2, "question and answer do not collapse together");
    assert.ok(pair.children[0].textContent.includes("and the deploy?"), "the folded header does not carry the verbatim question");
    assert.match(pair.children[1].children[0].textContent, /You\s*and the deploy\?/, "the question paragraph is not labelled You");
    assert.match(pair.children[1].children[1].textContent, /Discuss\s*It shipped\./, "the answer paragraph is not labelled Discuss");
    // The timeline toggle opens and closes every timeline collapsible at once.
    click(timelineToggle);
    assert.equal(pair.open, true, "timeline Expand did not open the pair");
    assert.equal(old.open, true, "timeline Expand did not open the earlier summary");
    assert.equal(timelineToggle.textContent, "Collapse", "timeline all-open did not read Collapse");
    click(timelineToggle);
    assert.equal(pair.open, false, "timeline Collapse did not close the pair");
    assert.equal(old.open, false, "timeline Collapse did not close the earlier summary");
    assert.equal(timelineToggle.textContent, "Expand", "timeline closed did not read Expand");
  } finally {
    if (savedDocument === undefined) delete globalThis.document;
    else globalThis.document = savedDocument;
  }
});

test("catchup header: dense lines, toggle to the right, one colour variable (#1960)", () => {
  const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
  const html = read("../../../web/voice.html");
  assert.ok(html.includes('id="discuss-current-head" class="discuss-section-head"'), "the current head lost its id");
  assert.ok(html.includes('class="discuss-section-lines"'), "the two header lines are not packed in one column");
  const linesAt = html.indexOf('class="discuss-section-lines"');
  const titleAt = html.indexOf('id="discuss-current-heading"', linesAt);
  const coversAt = html.indexOf('id="discuss-current-covers"', linesAt);
  assert.ok(linesAt >= 0 && titleAt > linesAt && coversAt > titleAt, "title then Covers do not stand densely in the left column");
  const css = read("../../../web/styles/voice/22-discuss.css");
  assert.ok(css.includes("--discuss-head-bg: var(--panel);"), "the head colour does not default to the panel colour in one variable");
  assert.ok(css.includes("background: var(--discuss-head-bg, var(--panel));"), ".discuss-head is not painted from the one variable");
  assert.match(css, /\.discuss-toggle \{[^}]*min-height: 44px;/, "the toggle is not a 44px phone touch target");
  assert.match(css, /\.discuss-section-head \{[^}]*align-items: center;/, "the toggle is not centred beside the header lines");
  const panel = read("../../../web/src/view/catchup/panel.ts");
  assert.equal(panel.includes("Expand all"), false, "an Expand all label remains in the panel");
  assert.equal(panel.includes("Collapse all"), false, "a Collapse all label remains in the panel");
  assert.equal(panel.includes("expand all"), false, "a lowercase expand all label remains in the panel");
});

test("catchup timeline header: the toggle shares the Timeline row (#2134)", () => {
  const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
  const html = read("../../../web/voice.html");
  const headAt = html.indexOf('id="discuss-timeline-head"');
  const labelAt = html.indexOf('id="discuss-timeline-heading"', headAt);
  const bodyAt = html.indexOf('id="discuss-timeline"', labelAt);
  assert.ok(headAt >= 0 && labelAt > headAt && bodyAt > labelAt, "Timeline label does not stand in its header row above the timeline body");
  const css = read("../../../web/styles/voice/22-discuss.css");
  assert.match(css, /\.discuss-timeline-head \{[^}]*flex-wrap: nowrap;/, "the Timeline header row can wrap onto a second row");
  const panel = read("../../../web/src/view/catchup/panel.ts");
  assert.ok(panel.includes('maybe("discuss-timeline-head")'), "the Timeline toggle is not placed in the header row");
});

test("catchup header: Terms counts toward Collapse, and the toggle opens it too (#1960)", async () => {
  const HTML = "<h2>Alpha</h2><p>a</p><h2>Beta</h2><p>b</p>";
  const scope = { channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" };
  const savedDocument = globalThis.document;
  globalThis.document = {
    createTextNode: (text) => ({ nodeType: 3, textContent: String(text), children: [] }),
    createElement: (tag) => {
      if (tag !== "template") return foldEl(tag);
      const template = { _html: "", content: null };
      Object.defineProperty(template, "innerHTML", {
        set(value) {
          template._html = String(value);
          const nodes = [];
          for (const match of template._html.matchAll(/<(h[1-6]|p)>([^<]*)<\/\1>/g)) nodes.push(foldEl(match[1], match[2]));
          template.content = { childNodes: nodes };
        },
        get() { return template._html; },
      });
      return template;
    },
  };
  try {
    const els = {};
    for (const id of IDS) els[id] = foldEl(id.endsWith("open") || id.endsWith("close") || id.includes("mark") ? "button" : "div");
    els["discuss-panel"].hidden = true;
    const panel = createDiscussPanel({
      byId: (id) => {
        const found = els[id];
        if (!found) throw new Error(`no element ${id}`);
        return found;
      },
      catchupPost: () => Promise.resolve(answer()),
      catchupPreview: () => Promise.resolve(preview()),
      catchupRecord: () => Promise.resolve({
        exists: true,
        current_summary: {
          window: { count: 12, unread_count: 2 }, covered_ids: ["m1", "m2"], markable_ids: ["m1", "m2"],
          cited_ids: [], footer_links: [], text: "saved point", text_html: HTML,
          unknown_terms: ["cache-key rewrite", "flaky quorum"], stamp: null, created_ms: T2,
        },
        timeline: [],
        staleness: null,
      }),
      catchupFollowup: () => Promise.resolve({}),
      serviceName: () => "discord",
      onJump: () => {},
      dismissMessages: async () => {},
      restoreMessages: async () => {},
    });
    panel.mount();
    panel.openWith(scope);
    await flush();
    await flush();
    const folds = () => els["discuss-answer"].children.filter((kid) => kid.tagName === "DETAILS");
    const toggle = () => els["discuss-current-head"].children.find((kid) => String(kid.className).includes("discuss-toggle"));
    const terms = els["discuss-terms"];
    assert.equal(terms.hidden, false, "Terms did not show");
    assert.deepEqual(folds().map((fold) => fold.open), [false, false]);
    assert.equal(toggle().textContent, "Expand", "folded folds and Terms did not read Expand");
    for (const fold of folds()) { fold.open = true; fire(fold, "toggle"); }
    assert.equal(toggle().textContent, "Expand", "Collapse read while Terms was still folded");
    click(toggle());
    assert.equal(terms.open, true, "Expand did not open Terms");
    assert.deepEqual(folds().map((fold) => fold.open), [true, true]);
    assert.equal(toggle().textContent, "Collapse", "everything open did not read Collapse");
    click(toggle());
    assert.equal(terms.open, false, "Collapse did not close Terms");
    assert.deepEqual(folds().map((fold) => fold.open), [false, false]);
    assert.equal(toggle().textContent, "Expand", "everything closed did not read Expand");
  } finally {
    if (savedDocument === undefined) delete globalThis.document;
    else globalThis.document = savedDocument;
  }
});

test("catchup: a follow-up sends from the composer and joins the timeline", async () => {
  const fx = fixture();
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(savedRecord());
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const box = fx.els["discuss-followup"];
    box.value = "and the deploy?";
    click(fx.els["discuss-send"]);
    await flush();
    assert.deepEqual(fx.calls.followups.map((call) => call.text), ["and the deploy?"]);
    assert.equal(box.value, "", "the box did not clear after sending");
    // Enter sends too (the composer's own keys, Enter-to-send on).
    box.value = "second question";
    const enter = { key: "Enter", target: box, preventDefault() {}, stopPropagation() {} };
    for (const listener of [...(box.listeners.keydown || [])]) listener(enter);
    await flush();
    assert.deepEqual(fx.calls.followups.map((call) => call.text), ["and the deploy?", "second question"]);
  } finally { fx.restore(); }
});

// ---- 7. the action menu's Discuss (the phone's way in) -----------------------------------------

test("catchup: the menu item opens the same panel and posts the same body", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    click(fx.els["discuss-menu-open"]);
    await flush();
    assert.equal(fx.els["discuss-panel"].hidden, false, "the menu item did not open the panel");
    assert.equal(fx.calls.posts.length, 0, "opening from the menu asked the model");
    click(fx.els["discuss-summarize"]);
    await flush();
    assert.equal(fx.calls.posts.length, 1, "Summarize did not ask");
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.");
  } finally { fx.restore(); }
});

test("catchup: unavailable draws the menu item disabled too, and its click posts nothing", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView({ ...fx.deps, discussAvailable: () => false });
    view.mountControls();
    const item = fx.els["discuss-menu-open"];
    assert.equal(item.getAttribute("aria-disabled"), "true", "the menu item is not drawn disabled");
    assert.equal(item.getAttribute("aria-label"), DISCUSS_UNAVAILABLE_HINT);
    click(item);
    await flush();
    assert.deepEqual(fx.calls.posts, [], "an unavailable menu Discuss still posted");
    assert.equal(fx.els["discuss-panel"].hidden, true);
  } finally { fx.restore(); }
});

// ---- 8. layout: kept width and sheet height (`#1344 discuss-panel-resize`) ----------------------

test("catchup layout: the wider default stands until dragged; a kept width wins on reopen", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(view.width(), 560, "the wider default did not stand");
    assert.equal(fx.els["discuss-panel"].style.props["--discuss-width"], "560px");
    view.setWidth(700);
    assert.equal(view.width(), 700);
    assert.deepEqual(fx.kept.keeps, [700], "the chosen width was not kept");
    view.close();
    view.open();
    await flush();
    assert.equal(view.width(), 700, "the kept width did not survive close/reopen");
    assert.equal(fx.els["discuss-panel"].style.props["--discuss-width"], "700px");
    view.setWidth(10);
    assert.equal(view.width(), 320, "a width under the floor was not clamped");
    view.setWidth(5000);
    assert.equal(view.width(), 960, "a width over the ceiling was not clamped");
  } finally { fx.restore(); }
});

test("catchup layout: dragging the edge widens live and keeps once, at the end", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const handle = fx.els["discuss-resize"];
    fire(handle, "pointerdown", { clientX: 800, pointerId: 1 });
    fire(handle, "pointermove", { clientX: 700, pointerId: 1 });
    assert.equal(view.width(), 660, "dragging left did not widen the panel");
    assert.deepEqual(fx.kept.keeps, [], "the width was kept mid-drag");
    fire(handle, "pointerup", { pointerId: 1 });
    assert.deepEqual(fx.kept.keeps, [660], "the dragged width was not kept at the end");
    assert.equal(handle.getAttribute("aria-valuenow"), "660");
    fire(handle, "keydown", { key: "ArrowRight" });
    assert.equal(view.width(), 644, "ArrowRight did not narrow the panel");
    assert.equal(fx.kept.keeps.at(-1), 644, "the keyed width was not kept");
    fire(handle, "dblclick", {});
    assert.equal(view.width(), 560, "a double-click did not restore the default");
  } finally { fx.restore(); }
});

test("catchup layout: the sheet starts at half, full height sticks across close/reopen", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(view.sheetFull(), false, "the sheet did not start at half");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-sheet"), "half");
    assert.equal(fx.els["discuss-full-toggle"].getAttribute("aria-pressed"), "false");
    view.setSheetFull(true);
    assert.equal(view.sheetFull(), true, "full height did not stand the sheet full");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-sheet"), "full");
    assert.equal(fx.els["discuss-full-toggle"].getAttribute("aria-pressed"), "true");
    assert.deepEqual(fx.kept.sheetKeeps, [true], "the full height was not kept");
    view.close();
    view.open();
    await flush();
    assert.equal(view.sheetFull(), true, "the full height did not stick across close/reopen");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-sheet"), "full");
    view.setSheetFull(false);
    assert.equal(view.sheetFull(), false, "half height did not return the sheet to half");
  } finally { fx.restore(); }
});

test("catchup docked height: title-bar drag resizes, clamps, snaps, keeps once; keys and buttons are not drags", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(view.dockedHeight(), 50, "the docked pane did not start at half");
    assert.equal(fx.els["discuss-panel"].style.props["--discuss-docked-height"], "50dvh");
    const head = fx.els["discuss-head"];
    // A tap (below the 6px threshold) on the bar is never a drag.
    fire(head, "pointerdown", { clientY: 400, pointerId: 7 });
    fire(head, "pointermove", { clientY: 397, pointerId: 7 });
    fire(head, "pointerup", { pointerId: 7 });
    assert.equal(view.dockedHeight(), 50, "a sub-threshold tap dragged the pane");
    assert.deepEqual(fx.kept.dockedKeeps, [], "a tap kept a height");
    // Drag up (clientY falls) grows the pane; document height is 0 in the fake, so 844 is assumed.
    fire(head, "pointerdown", { clientY: 400, pointerId: 7 });
    fire(head, "pointermove", { clientY: 316, pointerId: 7 });
    assert.equal(view.dockedHeight(), 60, "dragging up did not grow the pane");
    assert.deepEqual(fx.kept.dockedKeeps, [], "the height was kept mid-drag");
    fire(head, "pointerup", { pointerId: 7 });
    assert.deepEqual(fx.kept.dockedKeeps, [60], "the dragged height was not kept at the end");
    assert.equal(head.getAttribute("aria-valuenow"), "60");
    // Clamp at both ends, snap near half on release.
    view.setDockedHeight(1);
    assert.equal(view.dockedHeight(), 20, "a height under the floor was not clamped");
    view.setDockedHeight(999);
    assert.equal(view.dockedHeight(), 85, "a height over the ceiling was not clamped");
    view.setDockedHeight(50);
    fire(head, "pointerdown", { clientY: 400, pointerId: 7 });
    fire(head, "pointermove", { clientY: 383, pointerId: 7 });
    fire(head, "pointerup", { pointerId: 7 });
    assert.equal(view.dockedHeight(), 50, "a release near half did not snap to half");
    // Keyboard: ArrowUp grows, ArrowDown shrinks, kept at once.
    fire(head, "keydown", { key: "ArrowUp" });
    assert.equal(view.dockedHeight(), 52, "ArrowUp did not grow the pane");
    fire(head, "keydown", { key: "ArrowDown" });
    assert.equal(view.dockedHeight(), 50, "ArrowDown did not shrink the pane");
    assert.equal(fx.kept.dockedKeeps.at(-1), 50, "the keyed height was not kept");
    fire(head, "dblclick", {});
    assert.equal(view.dockedHeight(), 50, "a double-click did not restore half");
    // The kept height survives close/reopen; full sheet ignores the drag.
    view.setDockedHeight(65);
    view.close();
    view.open();
    await flush();
    assert.equal(view.dockedHeight(), 65, "the kept height did not survive close/reopen");
    view.setSheetFull(true);
    fire(head, "pointerdown", { clientY: 400, pointerId: 7 });
    fire(head, "pointermove", { clientY: 300, pointerId: 7 });
    fire(head, "pointerup", { pointerId: 7 });
    assert.equal(view.dockedHeight(), 65, "dragging the full sheet's bar resized the docked height");
  } finally { fx.restore(); }
});

// ---- 9. layout modes (`#1354 discuss-layout-modes`) ---------------------------------------------

test("catchup modes: overlay stands until chosen; a chosen mode is kept and survives reopen", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(view.mode(), "overlay", "the panel did not start as overlay");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-mode"), "overlay");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null);
    click(fx.els["discuss-dock-toggle"]);
    assert.equal(view.mode(), "docked");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "docked");
    assert.equal(fx.els["discuss-dock-toggle"].getAttribute("aria-pressed"), "true");
    assert.deepEqual(fx.kept.modeKeeps, ["docked"], "the docked choice was not kept");
    view.close();
    view.open();
    await flush();
    assert.equal(view.mode(), "docked", "the mode did not survive close/reopen");
    click(fx.els["discuss-full-toggle"]);
    assert.equal(view.mode(), "maximized");
    assert.equal(fx.els["discuss-full-toggle"].getAttribute("aria-pressed"), "true");
    click(fx.els["discuss-full-toggle"]);
    assert.equal(view.mode(), "docked", "fullscreen did not restore the mode before it");
    view.setMode("overlay");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "overlay left the screen marked");
  } finally { fx.restore(); }
});

test("catchup modes: closing a docked panel frees the column; reopening docks again", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    click(fx.els["discuss-dock-toggle"]);
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "docked");
    // `#2283`: the same facts on the root, where the pane-end and strip rules read them.
    assert.equal(globalThis.document.documentElement.getAttribute("data-discuss"), "docked", "docking did not name the mode on the root");
    assert.equal(globalThis.document.documentElement.hasAttribute("data-discuss-open"), true, "opening did not mark the root open");
    // Programmatic close: no reserved column, remembered mode kept.
    view.close();
    assert.equal(globalThis.document.documentElement.getAttribute("data-discuss"), null, "close left the mode on the root");
    assert.equal(globalThis.document.documentElement.hasAttribute("data-discuss-open"), false, "close left the root marked open");
    assert.equal(view.isOpen(), false);
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "close left the docked column reserved");
    assert.equal(view.mode(), "docked", "close forgot the remembered mode");
    view.open();
    await flush();
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "docked", "reopen did not dock again");
    // The X icon goes through the same path.
    click(fx.els["discuss-close"]);
    assert.equal(view.isOpen(), false);
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "the X left the docked column reserved");
    view.open();
    await flush();
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "docked", "reopen after the X did not dock again");
    // Escape too.
    fire(fx.els["discuss-panel"], "keydown", { key: "Escape" });
    assert.equal(view.isOpen(), false);
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "Escape left the docked column reserved");
    // The bar button toggles closed through panel.close().
    view.open();
    await flush();
    click(fx.els["discuss-open"]);
    assert.equal(view.isOpen(), false);
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "the bar toggle left the docked column reserved");
    // Overlay never marks the screen, open or closed.
    view.open();
    await flush();
    view.setMode("overlay");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null);
    view.close();
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "overlay close marked the screen");
    // Maximized likewise frees the screen on close and returns on reopen.
    view.open();
    await flush();
    view.setMode("maximized");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "maximized");
    view.close();
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "close left maximized marked on the screen");
    view.open();
    await flush();
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "maximized", "reopen did not maximize again");
  } finally { fx.restore(); }
});


// ---- 10. scope binding (`#1395 discuss-scope-binding`) ----------------------------------------

// Tap a citation anchor carrying `href` inside the answer, as a reader does.
function tapCitation(fx, href) {
  const anchor = fakeElement("a");
  anchor.setAttribute("href", href);
  fx.els["discuss-answer"].appendChild(anchor);
  let prevented = false;
  const ev = { target: anchor, preventDefault() { prevented = true; }, stopPropagation() {} };
  for (const listener of [...(fx.els["discuss-answer"].listeners.click || [])]) listener(ev);
  assert.equal(prevented, true, "the citation tap was not taken in-app");
}

const GLOBAL = { channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" };
const THREAD_T1 = { view: "thread", threadId: "t1", thread: { title: "First thread" } };

test("binding: the rules table names every rule and rule 5 is close", () => {
  assert.equal(WIDEN_OR_SIDEWAYS_ACTION, "close");
  assert.deepEqual(DISCUSS_BINDING_RULES.map((rule) => rule.name), [
    "bound-to-open-scope",
    "global-unifies-all-and-main",
    "narrowing-rescopes-to-thread",
    "in-thread-link-jumps-in-place",
    "out-of-thread-link-returns-to-bound",
    "widening-rescopes-to-main",
    "sideways-rescopes-to-thread",
    "widen-or-sideways-closes",
    "fallback-closes-and-jumps",
    "out-of-window-cite-links-out",
  ]);
  // All and Main are one global scope; a thread is its own.
  assert.equal(discussScopeKey({ ...GLOBAL, view: "main" }), discussScopeKey(GLOBAL));
  assert.equal(isSameDiscussScope({ ...GLOBAL, view: "main" }, GLOBAL), true);
  assert.equal(
    viewChangeDecision(normalizeDiscussScope(GLOBAL), normalizeDiscussScope({ ...GLOBAL, view: "main" })).action,
    "stay",
  );
  assert.equal(linkDecision({
    bound: normalizeDiscussScope(GLOBAL), current: normalizeDiscussScope(GLOBAL),
    linkChannel: "chan-1", linkThread: "t9", outOfScope: true,
  }).rule, "out-of-window-cite-links-out");
});

test("binding: entering a thread re-scopes an open Discuss; All/Main are one scope", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    assert.equal(view.boundScope().kind, "global");
    // All -> Main: same global scope, nothing moves.
    fx.setSelection({ view: "main", threadId: null, thread: null });
    view.sync();
    assert.equal(view.isOpen(), true);
    assert.equal(view.isSameScopeAsCurrent(), true);
    // Main -> thread (#1811): the panel stays open but re-scopes to the
    // thread — bound, labelled and titled for it, with no model call yet.
    fx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    assert.equal(view.isOpen(), true, "entering a thread closed the panel");
    assert.equal(view.boundScope().kind, "thread", "entering a thread did not rebind the panel");
    assert.equal(fx.els["discuss-scope"].textContent, "First thread");
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread First thread");
    assert.equal(fx.calls.posts.length, 1, "re-scoping asked the model");
    assert.deepEqual(fx.calls.history, [], "re-scoping pushed a history entry");
  } finally { fx.restore(); }
});

test("binding: an in-thread citation jumps in place", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    tapCitation(fx, "/?c=chan-1&v=thread&t=t1&m=discord%3Achan-1%3Am1%3At1");
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m1", thread: "t1" }]);
    assert.equal(view.isOpen(), true, "an in-thread jump closed the panel");
    assert.deepEqual(fx.calls.history, [], "an in-thread jump pushed history");
  } finally { fx.restore(); }
});

test("binding: after entering a thread, its citations jump in place and others close (#1811)", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    fx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    // Discuss is bound to t1 now: its own citation jumps in place.
    tapCitation(fx, "/?c=chan-1&v=thread&t=t1&m=discord%3Achan-1%3Am1%3At1");
    assert.equal(view.isOpen(), true, "an in-thread tap closed the panel");
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m1", thread: "t1" }]);
    // Another thread's citation falls back: close and jump, Back restores.
    tapCitation(fx, "/?c=chan-1&v=thread&t=t2&m=discord%3Achan-1%3Am9%3At2");
    assert.equal(view.isOpen(), false, "an out-of-thread tap left the panel open");
    assert.equal(fx.calls.jumps.length, 2);
  } finally { fx.restore(); }
});

test("binding: returning to main re-scopes to the channel; no model call, no history (#1811)", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    const postsBefore = fx.calls.posts.length;
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.sync();
    await flush();
    assert.equal(view.isOpen(), true, "returning to main closed the panel");
    assert.equal(view.boundScope().kind, "global", "returning to main did not rebind to the channel");
    assert.equal(fx.els["discuss-scope"].textContent, "Channel chan-1");
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: main channel");
    assert.equal(fx.calls.previews.at(-1).scope.threadId, null, "the preview did not re-read for the channel");
    assert.equal(fx.calls.posts.length, postsBefore, "re-scoping asked the model");
    assert.deepEqual(fx.calls.history, [], "re-scoping pushed a history entry");
    // Opening fresh in All binds global too.
    view.close();
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.open();
    await flush();
    assert.equal(view.boundScope().kind, "global");
  } finally { fx.restore(); }
});

test("binding: sideways switches windows (#1841); a fallback tap closes and jumps; an out-of-scope tap follows it", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    // Sideways: thread t1 -> thread t2 switches Discuss windows, as the
    // owner asked (#1841): the panel stays open on t2's own (blank) state.
    fx.setSelection({ view: "thread", threadId: "t2", thread: { title: "Second thread" } });
    view.sync();
    await flush();
    assert.equal(view.isOpen(), true, "a sideways switch closed the panel");
    assert.equal(view.boundScope().threadId, "t2", "a sideways switch did not rebind to t2");
    assert.equal(fx.els["discuss-title"].textContent, "Discuss: thread Second thread");
    assert.equal(fx.els["discuss-answer"].textContent, "", "t2 showed t1's summary");
    assert.deepEqual(fx.calls.history, [], "a sideways switch pushed a history entry");
    // Back to t1: its own summary returns, still with no history entry.
    fx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.", "returning to t1 lost its summary");
    // A citation for the other thread: fallback close + jump, Back restores.
    tapCitation(fx, "/?c=chan-1&v=thread&t=t2&m=discord%3Achan-1%3Am9%3At2");
    assert.equal(view.isOpen(), false, "a fallback tap left the panel open");
    assert.equal(fx.calls.history.length, 1);
    fx.setSelection(THREAD_T1);
    assert.equal(view.restoreFromHistory(fx.calls.history[0]), true);
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m9", thread: "t2" }]);
    // Back again, then an out-of-scope (scope=out) citation for this very
    // thread: the marker wins, same fallback route.
    assert.equal(view.restoreFromHistory(fx.calls.history[0]), true);
    tapCitation(fx, "/?c=chan-1&v=thread&t=t1&m=discord%3Achan-1%3Am1%3At1&scope=out");
    assert.equal(view.isOpen(), false, "an out-of-scope tap left the panel open");
    assert.equal(fx.calls.jumps.length, 2);
    assert.equal(fx.calls.posts.length, 1, "routing taps asked the model");
  } finally { fx.restore(); }
});

test("binding: reopening in the same scope restores; in another scope it starts fresh", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    view.close();
    view.open();
    await flush();
    assert.equal(view.isOpen(), true);
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.", "a same-scope reopen lost the answer");
    assert.equal(fx.calls.posts.length, 1, "a same-scope reopen asked the model");
    assert.equal(fx.calls.previews.length, 1, "a same-scope reopen read a preview");
    // A different scope starts fresh: preview read, bound moved, no answer yet.
    view.close();
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.open();
    await flush();
    assert.equal(view.boundScope().kind, "global");
    assert.equal(fx.calls.previews.length, 2, "a fresh open read no preview");
    assert.equal(view.isSameScopeAsCurrent(), true);
  } finally { fx.restore(); }
});

test("catchup modes: open puts focus on Close; Escape closes and returns it to the opener", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(fx.els["discuss-close"].focusCalls.length, 1, "open did not put focus on Close");
    fire(fx.els["discuss-panel"], "keydown", { key: "Escape" });
    assert.equal(view.isOpen(), false, "Escape did not close the panel");
    assert.equal(fx.els["discuss-open"].focusCalls.length, 1, "Escape did not return focus to the opener");
    fire(fx.els["discuss-panel"], "keydown", { key: "x" });
    assert.equal(view.isOpen(), false, "an unrelated key reopened or moved the panel");
  } finally { fx.restore(); }
});

test("catchup toggle: toggle opens over the scope, closes, and reopens with no model call", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.toggle();
    await flush();
    assert.equal(view.isOpen(), true, "toggle did not open the panel");
    assert.equal(fx.calls.previews.length, 1, "toggle open did not read the preview");
    assert.deepEqual(fx.calls.posts, [], "toggle open asked the model");
    view.toggle();
    assert.equal(view.isOpen(), false, "toggle did not close the panel");
    view.toggle();
    await flush();
    assert.equal(view.isOpen(), true, "toggle did not reopen the panel");
    assert.deepEqual(fx.calls.posts, [], "reopen asked the model");
  } finally { fx.restore(); }
});

// ---- #1497 discuss-fold-state-persist -----------------------------------------------------------

function foldEl(tagName = "div", text = "") {
  const el = {
    tagName: tagName.toUpperCase(),
    children: [],
    listeners: {},
    attributes: {},
    style: { setProperty() {} },
    hidden: false,
    disabled: false,
    className: "",
    type: "",
    open: undefined,
    parentNode: null,
    focus() {},
    appendChild(kid) { el.children.push(kid); kid.parentNode = el; return kid; },
    append(...kids) { for (const kid of kids) el.appendChild(kid); },
    prepend(...kids) { el.children.unshift(...kids); for (const kid of kids) kid.parentNode = el; },
    insertBefore(kid, before) {
      const at = before === null ? -1 : el.children.indexOf(before);
      if (at < 0) el.children.push(kid); else el.children.splice(at, 0, kid);
      kid.parentNode = el;
      return kid;
    },
    removeChild(kid) { el.children.splice(el.children.indexOf(kid), 1); return kid; },
    replaceChildren(...kids) { el.children = []; for (const kid of kids) el.appendChild(kid); },
    addEventListener(type, listener) { (el.listeners[type] ||= []).push(listener); },
    removeEventListener(type, listener) { el.listeners[type] = (el.listeners[type] || []).filter((one) => one !== listener); },
    setAttribute(name, value) { el.attributes[name] = String(value); },
    getAttribute(name) { return name in el.attributes ? el.attributes[name] : null; },
    hasAttribute(name) { return name in el.attributes; },
    removeAttribute(name) { delete el.attributes[name]; },
  };
  let own = text;
  Object.defineProperty(el, "textContent", {
    get: () => own + el.children.map((kid) => kid?.textContent ?? "").join(""),
    set: (value) => { own = String(value ?? ""); if (value) el.children = []; },
  });
  return el;
}

test("catchup folds: open sections survive close/reopen and a reload; a new summary resets (#1497)", async () => {
  const HTML = "<h2>Alpha</h2><p>a</p><h2>Beta</h2><p>b</p>";
  const scope = { channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" };
  let generation = 1000;
  const stored = { value: null, keeps: [] };
  const recordBody = () => ({
    exists: true,
    current_summary: {
      window: { count: 12, unread_count: 2 },
      covered_ids: ["m1", "m2"],
      markable_ids: ["m1", "m2"],
      cited_ids: [],
      footer_links: [
        { id: "m1", label: "09:51 · Ada", thread_id: null },
        { id: "m2", label: "09:52 · Ben", thread_id: null },
      ],
      text: "Alpha a. Beta b.",
      text_html: HTML,
      unknown_terms: [],
      stamp: null,
      created_ms: generation,
    },
    timeline: [],
    staleness: null,
  });
  const savedDocument = globalThis.document;
  globalThis.document = {
    createTextNode: (text) => ({ nodeType: 3, textContent: String(text), children: [] }),
    createElement: (tag) => {
      if (tag !== "template") return foldEl(tag);
      const template = { _html: "", content: null };
      Object.defineProperty(template, "innerHTML", {
        set(value) {
          template._html = String(value);
          const nodes = [];
          for (const match of template._html.matchAll(/<(h[1-6]|p)>([^<]*)<\/\1>/g)) nodes.push(foldEl(match[1], match[2]));
          template.content = { childNodes: nodes };
        },
        get() { return template._html; },
      });
      return template;
    },
  };
  try {
    const makePanel = () => {
      const els = {};
      for (const id of IDS) els[id] = foldEl(id.endsWith("open") || id.endsWith("close") || id.includes("mark") ? "button" : "div");
      els["discuss-panel"].hidden = true;
      const panel = createDiscussPanel({
        byId: (id) => {
          const found = els[id];
          if (!found) throw new Error(`no element ${id}`);
          return found;
        },
        catchupPost: () => Promise.resolve({
          window: { count: 12, unread_count: 2 },
          unread_ids: ["m1", "m2"],
          markable_ids: ["m1", "m2"],
          cited_ids: [],
          footer_links: [
            { id: "m1", label: "09:51 · Ada", thread_id: null },
            { id: "m2", label: "09:52 · Ben", thread_id: null },
          ],
          text: "Alpha a. Beta b.",
          text_html: HTML,
          unknown_terms: [],
          stamp: null,
        }),
        catchupPreview: () => Promise.resolve(preview()),
        catchupRecord: () => Promise.resolve(recordBody()),
        catchupFollowup: () => Promise.resolve({}),
        serviceName: () => "discord",
        onJump: () => {},
        dismissMessages: async () => {},
        restoreMessages: async () => {},
        storedFolds: () => stored.value,
        keepFolds: (state) => { stored.value = state; stored.keeps.push(state); },
      });
      const folds = () => els["discuss-answer"].children.filter((kid) => kid.tagName === "DETAILS");
      const toggle = () => els["discuss-current-head"].children.find((kid) => String(kid.className).includes("discuss-toggle"));
      return { els, panel, folds, toggle };
    };

    const first = makePanel();
    first.panel.mount();
    first.panel.openWith(scope);
    await flush();
    await flush();
    assert.deepEqual(first.folds().map((fold) => fold.open), [false, false], "sections did not start collapsed");
    assert.ok(first.toggle(), "the one header toggle is missing");
    assert.equal(first.toggle().textContent, "Expand", "all-closed did not read Expand");
    const beta = first.folds()[1];
    beta.open = true;
    fire(beta, "toggle");
    assert.deepEqual(stored.value, { record: "chan-1||1000", open: ["Beta"] }, "the toggle was not kept for the record");
    assert.equal(first.toggle().textContent, "Expand", "a mixed state did not bias to Expand");
    click(first.toggle());
    assert.deepEqual(first.folds().map((fold) => fold.open), [true, true], "Expand did not open every section");
    assert.equal(first.toggle().textContent, "Collapse", "all-open did not read Collapse");
    assert.deepEqual(stored.value, { record: "chan-1||1000", open: ["Alpha", "Beta"] }, "Expand was not kept");
    click(first.toggle());
    assert.deepEqual(first.folds().map((fold) => fold.open), [false, false], "Collapse did not close every section");
    assert.equal(first.toggle().textContent, "Expand", "all-closed did not read Expand again");
    assert.deepEqual(stored.value, { record: "chan-1||1000", open: [] }, "Collapse was not kept");
    const betaAgain = first.folds()[1];
    betaAgain.open = true;
    fire(betaAgain, "toggle");
    first.panel.close();
    first.panel.openWith(scope);
    await flush();
    await flush();
    assert.deepEqual(first.folds().map((fold) => fold.open), [false, true], "close/reopen reset the folds");

    const reloaded = makePanel();
    reloaded.panel.mount();
    reloaded.panel.openWith(scope);
    await flush();
    await flush();
    assert.deepEqual(reloaded.folds().map((fold) => fold.open), [false, true], "a reload reset the folds");

    stored.value = { record: "chan-1||999", open: ["Alpha", "Beta"] };
    const stale = makePanel();
    stale.panel.mount();
    stale.panel.openWith(scope);
    await flush();
    await flush();
    assert.deepEqual(stale.folds().map((fold) => fold.open), [false, false], "a stale record's folds opened sections");

    generation = 2000;
    click(reloaded.els["discuss-summarize"]);
    await flush();
    await flush();
    await flush();
    assert.deepEqual(reloaded.folds().map((fold) => fold.open), [false, false], "a new summary inherited folds");
    const alpha = reloaded.folds()[0];
    alpha.open = true;
    fire(alpha, "toggle");
    assert.deepEqual(stored.value, { record: "chan-1||2000", open: ["Alpha"] }, "the new summary kept folds under the old record");
  } finally {
    if (savedDocument === undefined) delete globalThis.document;
    else globalThis.document = savedDocument;
  }
});

// ---- #1501 discuss-read-aloud-parity ------------------------------------------------------------

test("catchup read: Read hands the owner all blocks under the record key; tap, highlight, stop (#1501)", async () => {
  const HTML = "<h2>Alpha</h2><p>first words</p><h2>Beta</h2><p>second words</p>";
  const scope = { channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" };
  const savedDocument = globalThis.document;
  globalThis.document = {
    createTextNode: (text) => ({ nodeType: 3, textContent: String(text), children: [] }),
    createElement: (tag) => {
      if (tag !== "template") return foldEl(tag);
      const template = { _html: "", content: null };
      Object.defineProperty(template, "innerHTML", {
        set(value) {
          template._html = String(value);
          const nodes = [];
          for (const match of template._html.matchAll(/<(h[1-6]|p)>([^<]*)<\/\1>/g)) nodes.push(foldEl(match[1], match[2]));
          template.content = { childNodes: nodes };
        },
        get() { return template._html; },
      });
      return template;
    },
  };
  try {
    const els = {};
    for (const id of IDS) els[id] = foldEl(id.endsWith("open") || id.endsWith("close") || id.includes("mark") || id === "read-aloud" ? "button" : "div");
    els["discuss-panel"].hidden = true;
    els["read-steer"].hidden = true;
    const reads = [];
    const stops = [];
    const panel = createDiscussPanel({
      byId: (id) => {
        const found = els[id];
        if (!found) throw new Error(`no element ${id}`);
        return found;
      },
      catchupPost: () => Promise.resolve({
        window: { count: 12, unread_count: 2 }, unread_ids: ["m1", "m2"], markable_ids: ["m1", "m2"],
        cited_ids: [], footer_links: [], text: "Alpha first words. Beta second words.", text_html: HTML,
        unknown_terms: [], stamp: null,
      }),
      catchupPreview: () => Promise.resolve(preview()),
      catchupRecord: () => Promise.resolve({
        exists: true,
        current_summary: {
          window: { count: 12, unread_count: 2 }, covered_ids: ["m1", "m2"], markable_ids: ["m1", "m2"],
          cited_ids: [], footer_links: [], text: "Alpha first words. Beta second words.", text_html: HTML,
          unknown_terms: [], stamp: null, created_ms: 4242,
        },
        timeline: [], staleness: null,
      }),
      catchupFollowup: () => Promise.resolve({}),
      serviceName: () => "discord",
      onJump: () => {},
      dismissMessages: async () => {},
      restoreMessages: async () => {},
      storedFolds: () => null,
      keepFolds: () => {},
      readDocument: (doc, from) => { reads.push({ doc, from }); },
      stopSpeaking: (key) => { stops.push(key); },
    });
    panel.mount();
    panel.openWith(scope);
    await flush();
    await flush();
    // Read: ONE owner call, the record key, ALL blocks (folded sections read whole).
    // `#1531`: the toolbar's Read, not a top-bar button, starts it now.
    assert.equal(els["discuss-read"], undefined, "the top bar still carries a Read button");
    click(els["read-aloud"]);
    assert.equal(reads.length, 1, "Read did not hand the summary to the owner once");
    assert.equal(reads[0].doc.key, "discuss:chan-1||4242", "the read key is not the saved record identity");
    assert.deepEqual([...reads[0].doc.texts], ["Alpha", "first words", "Beta", "second words"],
      "the read texts are not all drawn blocks in order");
    assert.deepEqual([...reads[0].doc.kinds], ["heading", "paragraph", "heading", "paragraph"],
      "the read kinds are not the drawn blocks' kinds, aligned with the texts");
    assert.equal(typeof reads[0].doc.onReadingBlock, "function", "the read carries no block report");
    // A second toolbar Read while this read stands does not restart it (it is its Pause).
    click(els["read-aloud"]);
    assert.equal(reads.length, 1, "a second toolbar Read restarted the standing summary read");
    assert.equal(panel.readSummary(), true, "readSummary did not report the standing summary read");
    // Highlight: the reported block lights, a null report clears it.
    const blocks = [];
    const collect = (el) => {
      for (const kid of el.children || []) {
        if (["P", "H1", "H2", "H3", "H4", "H5", "H6"].includes(kid.tagName)) blocks.push(kid);
        collect(kid);
      }
    };
    collect(els["discuss-answer"]);
    assert.equal(blocks.length, 4, "the drawn blocks were not found for the highlight check");
    reads[0].doc.onReadingBlock(1, 1);
    assert.equal(blocks[1].getAttribute("data-reading-block"), "current", "block 1 was not lit");
    reads[0].doc.onReadingBlock(null);
    assert.equal(blocks[1].getAttribute("data-reading-block"), null, "the light stayed on after the read stopped");
    // `#1543`: inactive — tapping a block starts no read.
    reads.length = 0;
    const ev = { target: blocks[3], preventDefault() {}, stopPropagation() {} };
    for (const listener of [...(els["discuss-answer"].listeners.click || [])]) listener(ev);
    assert.equal(reads.length, 0, "tapping a block started a read while the summary reader was inactive");
    // Active again: Read restarts it, then a tap jumps to that block.
    click(els["read-aloud"]);
    assert.equal(reads.length, 1, "Read did not restart the summary read");
    reads.length = 0;
    for (const listener of [...(els["discuss-answer"].listeners.click || [])]) listener(ev);
    assert.equal(reads.length, 1, "tapping a block started no read while reading");
    assert.deepEqual(reads[0].from, { part: 0, block: 3 }, "the tap did not name its block index");
    assert.equal(reads[0].doc.key, "discuss:chan-1||4242", "the tap read under another key");
    // Stop scoping: closing stops only this key; a new Summarize stops it too.
    stops.length = 0;
    click(els["discuss-close"]);
    assert.deepEqual(stops, ["discuss:chan-1||4242"], "closing did not stop the Discuss read by its key");
    panel.openWith(scope);
    await flush();
    await flush();
    click(els["read-aloud"]);
    stops.length = 0;
    click(els["discuss-summarize"]);
    await flush();
    assert.deepEqual(stops, ["discuss:chan-1||4242"], "a new Summarize did not stop the Discuss read by its key");
  } finally {
    if (savedDocument === undefined) delete globalThis.document;
    else globalThis.document = savedDocument;
  }
});

// ---- #1388 landing 1: live strike-through -------------------------------------------------------

function strikeEl(tag, kids = [], href = null) {
  const el = fakeElement(tag);
  el.open = false;
  if (href !== null) el.attributes.href = href;
  for (const kid of kids) el.appendChild(kid);
  return el;
}

const cite = (id) => strikeEl("a", [], `?c=C9&v=all&m=discord:C9:${id}`);
const fresh = new Set();

test("catchup strike: citations map to their bullet and section; uncited never struck (#1388)", () => {
  const struck = strikeEl("li", [cite("m1")]);
  const partial = strikeEl("li", [cite("m1"), cite("m2")]);
  const uncited = strikeEl("li", []);
  const issueOnly = strikeEl("li", [strikeEl("a", [], "https://github.com/rrnewton/agent-utils/issues/1388")]);
  const list = strikeEl("ul", [struck, partial, uncited, issueOnly]);
  const summary = strikeEl("summary", [strikeEl("h2", [])]);
  const body = strikeEl("div", [strikeEl("p", [cite("m1")]), list]);
  const section = strikeEl("details", [summary, body]);
  const root = strikeEl("div", [section]);
  assert.deepEqual(citationIdsOf(struck), ["m1"], "a bullet's citations are its links' messages");
  assert.deepEqual(citationIdsOf(section), ["m1", "m2"], "a section's citations span its bullets");
  const read = new Set(["m1"]);
  const counts = applyStrikes(root, (id) => read.has(id), fresh);
  assert.equal(hasWord(struck, STRUCK_BULLET_CLASS), true, "an all-read bullet was not struck");
  assert.equal(hasWord(partial, STRUCK_BULLET_CLASS), false, "a partly-read bullet was struck");
  assert.equal(hasWord(uncited, STRUCK_BULLET_CLASS), false, "an uncited bullet was struck");
  assert.equal(hasWord(issueOnly, STRUCK_BULLET_CLASS), false, "an issue-only bullet was struck");
  assert.equal(hasWord(section, STRUCK_SECTION_CLASS), false, "a partly-read section was struck");
  assert.deepEqual(counts, { bullets: 1, sections: 0 });
  // Live recompute: once m2 is read too, bullet and section strike without a redraw.
  read.add("m2");
  applyStrikes(root, (id) => read.has(id), fresh);
  assert.equal(hasWord(partial, STRUCK_BULLET_CLASS), true, "the strike did not recompute live");
  assert.equal(hasWord(section, STRUCK_SECTION_CLASS), true, "the section did not strike when all read");
  assert.equal(hasWord(uncited, STRUCK_BULLET_CLASS), false, "an uncited bullet struck late");
  // And back: unreading unstrikes.
  read.delete("m1");
  applyStrikes(root, (id) => read.has(id), fresh);
  assert.equal(hasWord(struck, STRUCK_BULLET_CLASS), false, "unreading did not unstrike");
});

test("catchup strike: folds survive a strike; fresh is green until read (#1388)", () => {
  const bullet = strikeEl("li", [cite("m9")]);
  const section = strikeEl("details", [strikeEl("summary", [strikeEl("h2", [])]), strikeEl("div", [strikeEl("ul", [bullet])])]);
  section.open = true;
  const root = strikeEl("div", [section]);
  const read = new Set();
  applyStrikes(root, (id) => read.has(id), new Set(["m9"]));
  assert.equal(hasWord(bullet, FRESH_CLASS), true, "the Update's bullet was not fresh");
  assert.equal(section.open, true, "striking moved a fold");
  read.add("m9");
  applyStrikes(root, (id) => read.has(id), new Set(["m9"]));
  assert.equal(hasWord(bullet, FRESH_CLASS), false, "a read bullet stayed green");
  assert.equal(hasWord(bullet, STRUCK_BULLET_CLASS), true, "a read fresh bullet was not struck");
  assert.equal(section.open, true, "the fold moved on recompute");
});

/** Every `li` under a fake element, in tree order. */
function bulletsUnder(root) {
  const out = [];
  const walk = (node) => {
    if (String(node.tagName ?? "").toLowerCase() === "li") out.push(node);
    for (const kid of Array.from(node.children ?? [])) walk(kid);
  };
  walk(root);
  return out;
}

test("catchup strike: the panel strikes live from isReadNow and its mark buttons (#1388)", async () => {
  const read = new Set();
  // The answer's bullets carry their citation links through the one parser,
  // so the draw maps them (`#2265`: indexed at draw time, as the page's).
  const html = '<h2>S</h2><ul><li><a href="?c=C9&v=all&m=discord:C9:m1">m1</a></li><li>one</li></ul>';
  const fx = fixture({ respond: () => Promise.resolve(answer({ cited_ids: ["m1", "m2"], text: "x", text_html: html })) });
  fx.deps.isReadNow = (id) => read.has(id);
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    const [li, plain] = bulletsUnder(fx.els["discuss-answer"]);
    assert.ok(li && plain, "the answer's two bullets were not drawn");
    view.refreshStrikes();
    assert.equal(hasWord(li, STRUCK_BULLET_CLASS), false, "an unread bullet was struck");
    read.add("m1");
    view.refreshStrikes();
    assert.equal(hasWord(li, STRUCK_BULLET_CLASS), true, "a read bullet was not struck live");
    assert.equal(hasWord(plain, STRUCK_BULLET_CLASS), false, "an uncited bullet was struck");
    // The Discuss mark-read button marks every markable id; with the page's
    // read set following (as the store does), the strike follows on settle.
    read.add("m2");
    click(fx.els["discuss-mark-read"]);
    await flush();
    assert.deepEqual(fx.calls.dismiss, [{ messages: ["m1", "m2"] }], "Mark read sent another set");
    assert.equal(hasWord(li, STRUCK_BULLET_CLASS), true, "the mark did not keep the strike");
  } finally { fx.restore(); }
});

test("catchup update: the banner's Update is one delta call; new parts green until read (#1388)", async () => {
  const read = new Set();
  const fx = fixture();
  fx.deps.isReadNow = (id) => read.has(id);
  // The merged answer's fresh bullet comes through the one parser, so the
  // draw maps it (`#2265`: indexed at draw time).
  fx.deps.catchupUpdate = (channelId, scope) => {
    fx.calls.updates.push({ channelId, scope });
    return Promise.resolve(answer({ fresh_ids: ["m2"], text: "old and new", text_html: '<ul><li><a href="?c=C9&v=all&m=discord:C9:m2">m2</a></li></ul>' }));
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    click(fx.els["discuss-update"]);
    await flush();
    assert.equal(fx.calls.updates.length, 1, "Update did not make its one delta call");
    assert.equal(fx.calls.posts.length, 1, "Update refetched a full summary");
    assert.equal(fx.els["discuss-status"].textContent, "Updated.");
    // The merged answer's fresh bullet draws green; read, it strikes instead.
    const [li] = bulletsUnder(fx.els["discuss-answer"]);
    assert.ok(li, "the Update's bullet was not drawn");
    view.refreshStrikes();
    assert.equal(hasWord(li, FRESH_CLASS), true, "the Update's bullet was not green");
    read.add("m2");
    view.refreshStrikes();
    assert.equal(hasWord(li, FRESH_CLASS), false, "a read bullet stayed green");
    assert.equal(hasWord(li, STRUCK_BULLET_CLASS), true, "a read fresh bullet was not struck");
  } finally { fx.restore(); }
});

// ---- `#2265 vibe-talk-summary-bullet-restyle-everywhere` ----------------------------------------

/** A tiny element tree for the strike index: `li`/`details`/`a` with an href, connected unless said. */
function strikeTree(spec) {
  const make = (node) => {
    const el = { tagName: node.tag.toUpperCase(), className: node.className ?? "", children: [], attributes: {} };
    if (node.cite !== undefined) el.attributes.href = `?c=C9&v=all&m=discord:C9:${node.cite}`;
    el.getAttribute = (name) => (name in el.attributes ? el.attributes[name] : null);
    el.isConnected = true;
    for (const kid of node.kids ?? []) el.children.push(make(kid));
    return el;
  };
  return make(spec);
}

const citeLi = (...ids) => ({ tag: "li", kids: ids.map((id) => ({ tag: "a", cite: id })) });

/** Every element of `tag` under `root`, in tree order. */
function allTagged(root, tag) {
  const out = [];
  const walk = (node) => {
    if (String(node.tagName).toLowerCase() === tag) out.push(node);
    for (const kid of Array.from(node.children ?? [])) walk(kid);
  };
  walk(root);
  return out;
}

test("strike index: one lookup restyles every bullet citing the id, in every group (#2265)", () => {
  const read = new Set();
  const facts = { read: (id) => read.has(id), deleted: () => false };
  const index = createStrikeIndex();
  const answer = strikeTree({ tag: "div", kids: [{ tag: "ul", kids: [citeLi("m1"), citeLi("m2"), { tag: "li" }] }] });
  const timeline = strikeTree({ tag: "div", kids: [
    { tag: "div", className: "discuss-old-body", kids: [{ tag: "ul", kids: [citeLi("m1"), citeLi("m3")] }] },
    // A follow-up turn's bullets are not a summary's: never indexed.
    { tag: "div", className: "discuss-qa-body", kids: [{ tag: "ul", kids: [citeLi("m1")] }] },
  ] });
  index.replace("answer", answer);
  index.strikeGroup("answer", facts);
  index.replace("timeline", timeline, { within: (el) => hasWord(el, "discuss-old-body") });
  index.strikeGroup("timeline", facts);
  const [curM1, curM2, plain] = allTagged(answer, "li");
  const [oldM1, oldM3, turnM1] = allTagged(timeline, "li");
  assert.equal(index.citing("m1").length, 2, "m1's bullets in the answer and the earlier summary are not both indexed");
  assert.deepEqual(index.size(), { ids: 3, entries: 4, links: 4 });
  read.add("m1");
  const tally = index.strikeIds(["m1"], facts);
  assert.equal(hasWord(curM1, STRUCK_BULLET_CLASS), true, "the current summary's bullet did not strike");
  assert.equal(hasWord(oldM1, STRUCK_BULLET_CLASS), true, "the earlier summary's bullet did not strike");
  assert.equal(hasWord(turnM1, STRUCK_BULLET_CLASS), false, "a follow-up turn's bullet was struck");
  assert.equal(hasWord(curM2, STRUCK_BULLET_CLASS) || hasWord(oldM3, STRUCK_BULLET_CLASS) || hasWord(plain, STRUCK_BULLET_CLASS), false, "a bullet not citing m1 moved");
  assert.equal(tally.restyled, 4, "the lookup restyled more than m1's two bullets and two links");
  read.delete("m1");
  index.strikeIds(["m1"], facts);
  assert.equal(hasWord(curM1, STRUCK_BULLET_CLASS) || hasWord(oldM1, STRUCK_BULLET_CLASS), false, "a put back left a bullet struck");
});

test("strike index: a change with no ids restyles only what cites an id that moved (#2265)", () => {
  const read = new Set();
  const facts = { read: (id) => read.has(id), deleted: () => false };
  const index = createStrikeIndex();
  const root = strikeTree({ tag: "ul", kids: [citeLi("m1"), citeLi("m2"), citeLi("m1", "m2")] });
  index.replace("answer", root);
  index.strikeGroup("answer", facts);
  assert.equal(index.strikeChanged(facts).restyled, 0, "nothing moved, yet something restyled");
  read.add("m2");
  const tally = index.strikeChanged(facts);
  const [one, two, both] = allTagged(root, "li");
  assert.equal(hasWord(two, STRUCK_BULLET_CLASS), true);
  assert.equal(hasWord(one, STRUCK_BULLET_CLASS) || hasWord(both, STRUCK_BULLET_CLASS), false, "a bullet with an unread citation was struck");
  assert.equal(tally.restyled, 4, "m2's two bullets and two links are all that should restyle");
  read.add("m1");
  index.strikeChanged(facts);
  assert.equal(hasWord(both, STRUCK_BULLET_CLASS), true, "a bullet whose citations are all read did not strike");
});

test("strike index: a redraw replaces its group; a detached element is dropped when an event reaches it (#2265)", () => {
  const read = new Set(["m1"]);
  const facts = { read: (id) => read.has(id), deleted: () => false };
  const index = createStrikeIndex();
  const first = strikeTree({ tag: "ul", kids: [citeLi("m1")] });
  index.replace("timeline", first);
  index.strikeGroup("timeline", facts);
  const second = strikeTree({ tag: "ul", kids: [citeLi("m1")] });
  index.replace("timeline", second);
  index.strikeGroup("timeline", facts);
  assert.equal(hasWord(allTagged(second, "li")[0], STRUCK_BULLET_CLASS), true, "the redrawn bullet did not strike at its draw");
  assert.deepEqual(index.citing("m1"), [allTagged(second, "li")[0]], "the old draw is still indexed");
  // An answer box blanked without a redraw: its elements leave the page.
  const answer = strikeTree({ tag: "ul", kids: [citeLi("m1")] });
  index.replace("answer", answer);
  index.strikeGroup("answer", facts);
  for (const el of [answer, ...allTagged(answer, "li"), ...allTagged(answer, "a")]) el.isConnected = false;
  read.delete("m1");
  index.strikeIds(["m1"], facts);
  assert.equal(index.citing("m1").length, 1, "a detached bullet stayed indexed");
  assert.deepEqual(index.size(), { ids: 1, entries: 1, links: 1 });
  index.replace("timeline", null);
  assert.deepEqual(index.size(), { ids: 0, entries: 0, links: 0 }, "an emptied group left entries behind");
});

test("strike index: a deleted cited message counts as done and its links are marked deleted (#2265)", () => {
  const read = new Set();
  const deleted = new Set();
  const facts = { read: (id) => read.has(id), deleted: (id) => deleted.has(id) };
  const index = createStrikeIndex();
  const root = strikeTree({ tag: "div", kids: [
    { tag: "details", kids: [{ tag: "summary" }, { tag: "ul", kids: [citeLi("m1", "m2"), citeLi("m3")] }] },
  ] });
  index.replace("answer", root);
  index.strikeGroup("answer", facts);
  const [pair, solo] = allTagged(root, "li");
  const [section] = allTagged(root, "details");
  const [linkM1, linkM2] = allTagged(pair, "a");
  read.add("m1");
  deleted.add("m2");
  index.strikeIds(["m1", "m2"], facts);
  assert.equal(hasWord(pair, STRUCK_BULLET_CLASS), true, "read plus deleted did not strike the bullet");
  assert.equal(hasWord(linkM2, DELETED_CITE_CLASS), true, "the deleted message's link was not marked");
  assert.equal(hasWord(linkM1, DELETED_CITE_CLASS), false, "a read message's link was marked deleted");
  assert.equal(hasWord(solo, STRUCK_BULLET_CLASS) || hasWord(section, STRUCK_SECTION_CLASS), false, "an unread bullet or its section struck");
  deleted.add("m3");
  index.strikeIds(["m3"], facts);
  assert.equal(hasWord(section, STRUCK_SECTION_CLASS), true, "a section whose citations are all done did not strike");
  // A later read carried the message after all: no longer deleted.
  deleted.delete("m2");
  index.strikeIds(["m2"], facts);
  assert.equal(hasWord(linkM2, DELETED_CITE_CLASS), false, "the deleted mark outlived the removal");
  assert.equal(hasWord(pair, STRUCK_BULLET_CLASS), false, "an undeleted unread citation left its bullet struck");
});

test("catchup strike: a Done strikes the current AND the earlier summary's bullet; put back clears both; deleted stays struck and marked (#2265)", async () => {
  const read = new Set();
  const deleted = new Set();
  const cite = (id) => `<a href="?c=chan-1&v=all&m=discord:chan-1:${id}">${id}</a>`;
  const fx = fixture();
  fx.deps.isReadNow = (id) => read.has(id);
  fx.deps.isDeletedNow = (id) => deleted.has(id);
  const base = savedRecord();
  const record = savedRecord({
    current_summary: { ...base.current_summary, text_html: `<ul><li>${cite("m1")}</li><li>${cite("m2")}</li></ul>` },
    timeline: [
      { kind: "summary", summary: { view: "flat", window: { count: 9, unread_count: 2 }, covered_ids: ["m1", "m3"], markable_ids: ["m1", "m3"], footer_links: [], text: "older", text_html: `<ul><li>${cite("m1")}</li><li>${cite("m3")}</li></ul>`, unknown_terms: [], stamp: null, created_ms: T1 } },
    ],
    staleness: null,
  });
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(record);
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const [curM1, curM2] = bulletsUnder(fx.els["discuss-answer"]);
    const [oldM1, oldM3] = bulletsUnder(fx.els["discuss-timeline"]);
    assert.ok(curM1 && curM2 && oldM1 && oldM3, "the current and earlier summaries' bullets were not drawn");
    assert.equal([curM1, curM2, oldM1, oldM3].some((li) => hasWord(li, STRUCK_BULLET_CLASS)), false, "an unread bullet was struck");
    // The row's own Done names its message: one lookup, both summaries.
    read.add("m1");
    view.refreshStrikes(["m1"]);
    assert.equal(hasWord(curM1, STRUCK_BULLET_CLASS), true, "the current summary's bullet did not strike");
    assert.equal(hasWord(oldM1, STRUCK_BULLET_CLASS), true, "the earlier summary's bullet did not strike");
    assert.equal(hasWord(curM2, STRUCK_BULLET_CLASS) || hasWord(oldM3, STRUCK_BULLET_CLASS), false, "a bullet citing another message struck");
    read.delete("m1");
    view.refreshStrikes(["m1"]);
    assert.equal(hasWord(curM1, STRUCK_BULLET_CLASS) || hasWord(oldM1, STRUCK_BULLET_CLASS), false, "a put back left a bullet struck");
    // Server read state landing (no ids): what moved restyles.
    read.add("m3");
    view.refreshStrikes();
    assert.equal(hasWord(oldM3, STRUCK_BULLET_CLASS), true, "a read landing with no ids missed the earlier summary");
    // Deleted upstream: done for the strike, its links marked, in both summaries.
    deleted.add("m1");
    view.refreshStrikes(["m1"]);
    for (const li of [curM1, oldM1]) {
      assert.equal(hasWord(li, STRUCK_BULLET_CLASS), true, "a deleted cited message left its bullet unstruck");
      assert.equal(hasWord(li.children[0], DELETED_CITE_CLASS), true, "a deleted cited message's link was not marked");
    }
    assert.equal(hasWord(curM2.children[0], DELETED_CITE_CLASS), false, "a live message's link was marked deleted");
  } finally { fx.restore(); }
});

test("catchup new summary: New summary adds a fresh card and keeps the first in the timeline (#2073)", async () => {
  const fx = fixture();
  const before = savedRecord();
  const after = savedRecord({
    current_summary: { ...before.current_summary, text: "fresh point", covered_ids: ["m2"], created_ms: T2 + 60_000 },
    timeline: [{ kind: "summary", summary: before.current_summary }],
    staleness: null,
  });
  let recordCalls = 0;
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    recordCalls += 1;
    return Promise.resolve(recordCalls === 1 ? before : after);
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(fx.els["discuss-answer"].textContent, "saved point", "the first summary did not stand first");
    click(fx.els["discuss-new-summary"]);
    await flush();
    assert.equal(fx.calls.newSummaries.length, 1, "New summary did not make its one fresh call");
    assert.deepEqual(fx.calls.posts, [], "New summary refetched a full summary");
    assert.deepEqual(fx.calls.updates, [], "New summary merged in place like Update");
    assert.equal(fx.els["discuss-status"].textContent, "New summary.");
    assert.equal(fx.els["discuss-answer"].textContent, "fresh point", "the fresh summary did not stand on top");
    const timeline = fx.els["discuss-timeline"].children;
    assert.equal(timeline.length, 1, "the timeline did not keep the first summary under the fresh one");
    assert.match(timeline[0].children[0].textContent, /Earlier summary/, "the first summary is not labelled earlier");
    assert.match(timeline[0].children[0].textContent, /covers 09:51–09:52/, "the earlier summary does not name the range it covers");
  } finally { fx.restore(); }
});

test("catchup new summary: disabled with its reason when nothing is new (#2073)", async () => {
  const fx = fixture();
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(savedRecord({ staleness: { new_unread: 0, marked_read: 1 } }));
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const button = fx.els["discuss-new-summary"];
    assert.equal(button.disabled, true, "New summary stayed offered with nothing new");
    assert.equal(button.title, "No new messages since the last summary", "the disabled reason is missing");
    click(button);
    await flush();
    assert.equal(fx.calls.newSummaries.length, 0, "a disabled New summary still called");
  } finally { fx.restore(); }
});

test("catchup new summary: a second press while one is in flight starts nothing (#2073)", async () => {
  const fx = fixture();
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(savedRecord());
  };
  const attempt = deferred();
  fx.deps.catchupNewSummary = (channelId, scope) => {
    fx.calls.newSummaries.push({ channelId, scope });
    return attempt.promise;
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    click(fx.els["discuss-new-summary"]);
    click(fx.els["discuss-new-summary"]);
    click(fx.els["discuss-update"]);
    assert.equal(fx.calls.newSummaries.length, 1, "an in-flight New summary started another");
    assert.equal(fx.calls.updates.length, 0, "Update started while a New summary was in flight");
    attempt.resolve(answer({ text: "fresh point" }));
    await flush();
    assert.equal(fx.els["discuss-status"].textContent, "New summary.");
  } finally { fx.restore(); }
});

// ---- 11. full-screen drops to partial on a message link (`#1586`) ------------------------------

test("discussdrop: a citation tap in the full sheet drops to half at the kept height", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    view.setDockedHeight(65);
    view.setSheetFull(true);
    assert.equal(view.sheetFull(), true);
    tapCitation(fx, "/?c=chan-1&v=all&m=discord%3Achan-1%3Am1");
    assert.equal(fx.calls.jumps.length, 1, "the citation did not jump");
    assert.equal(view.isOpen(), true, "the drop closed the panel");
    assert.equal(view.sheetFull(), false, "the full sheet did not drop to half");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-sheet"), "half");
    assert.equal(view.dockedHeight(), 65, "the drop forgot the dragged height");
    assert.equal(fx.kept.sheetKeeps.at(-1), false, "the partial choice was not kept");
    // The maximize button's own act stands it full again.
    view.setSheetFull(true);
    assert.equal(view.sheetFull(), true, "maximize did not restore full screen");
  } finally { fx.restore(); }
});

test("discussdrop: a footer time in the full sheet drops to half and jumps", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    view.setSheetFull(true);
    const first = fx.els["discuss-links"].children[0].children[0];
    click(first);
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m1", thread: null }]);
    assert.equal(view.sheetFull(), false, "the footer jump left the sheet full");
    assert.equal(view.isOpen(), true, "the footer jump closed the panel");
  } finally { fx.restore(); }
});

test("discussdrop: maximized drops to the mode it came from; full screen restores it", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    view.setMode("docked");
    click(fx.els["discuss-full-toggle"]);
    assert.equal(view.mode(), "maximized", "the full toggle did not maximize");
    tapCitation(fx, "/?c=chan-1&v=all&m=discord%3Achan-1%3Am1");
    assert.equal(fx.calls.jumps.length, 1, "the citation did not jump");
    assert.equal(view.mode(), "docked", "maximized did not drop to its partial mode");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "docked");
    assert.equal(view.isOpen(), true, "the drop closed the panel");
    click(fx.els["discuss-full-toggle"]);
    assert.equal(view.mode(), "maximized", "the full-screen button did not maximize again");
  } finally { fx.restore(); }
});

test("discussdrop: an in-thread tap drops; a tap at partial keeps nothing new", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    // At half/overlay a staying tap jumps without touching the kept layout.
    const sheetKeeps = fx.kept.sheetKeeps.length;
    const modeKeeps = fx.kept.modeKeeps.length;
    tapCitation(fx, "/?c=chan-1&v=all&m=discord%3Achan-1%3Am1");
    assert.equal(fx.kept.sheetKeeps.length, sheetKeeps, "a partial tap kept a sheet choice");
    assert.equal(fx.kept.modeKeeps.length, modeKeeps, "a partial tap kept a mode choice");
    // In the thread Discuss now binds to (#1811) and full: an in-thread
    // citation drops to partial and jumps in place, panel open.
    fx.setSelection(THREAD_T1);
    view.sync();
    await flush();
    view.setSheetFull(true);
    tapCitation(fx, "/?c=chan-1&v=thread&t=t1&m=discord%3Achan-1%3Am1%3At1");
    assert.equal(fx.calls.jumps.length, 2, "the in-thread tap did not jump in place");
    assert.equal(view.sheetFull(), false, "the in-thread tap left the sheet full");
    assert.equal(view.isOpen(), true, "the in-thread tap closed the panel");
  } finally { fx.restore(); }
});

test("discussdrop: a fallback tap while full still closes and keeps no partial choice", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    view.setSheetFull(true);
    const sheetKeeps = fx.kept.sheetKeeps.length;
    tapCitation(fx, "/?c=chan-1&v=thread&t=t2&m=discord%3Achan-1%3Am9%3At2");
    assert.equal(view.isOpen(), false, "the fallback tap left the panel open");
    assert.equal(fx.calls.jumps.length, 1, "the fallback tap did not jump");
    assert.equal(view.sheetFull(), true, "the closing tap dropped the sheet first");
    assert.equal(fx.kept.sheetKeeps.length, sheetKeeps, "the closing tap kept a partial choice");
  } finally { fx.restore(); }
});

test("discusspersist: open keeps its scope, close keeps closed, a fresh view restores open (#1821)", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    let keptPanel = null;
    const keeps = [];
    const deps = {
      ...fx.deps,
      storedPanelState: () => keptPanel,
      keepPanelState: (state) => { keptPanel = state; keeps.push(state); },
    };
    const view = createDiscussView(deps);
    view.mountControls();
    assert.equal(view.isOpen(), false, "a fresh view with nothing kept opened");
    view.open();
    await flush();
    assert.equal(view.isOpen(), true);
    assert.deepEqual(keptPanel, { open: true, channelId: "chan-1", threadId: "t1", view: "flat", label: "First thread" },
      "opening did not keep its thread scope");
    view.close();
    assert.deepEqual(keptPanel, { open: false, channelId: "chan-1", threadId: "t1", view: "flat", label: "First thread" },
      "closing did not persist as closed");
    // Reload with the closed entry: a fresh view over the same store stays closed.
    const closedFx = fixture({ selection: THREAD_T1 });
    try {
      const reloaded = createDiscussView({ ...closedFx.deps, storedPanelState: () => keptPanel, keepPanelState: (s) => { keptPanel = s; } });
      reloaded.mountControls();
      reloaded.sync();
      await flush();
      assert.equal(reloaded.isOpen(), false, "a kept-closed panel restored open");
      assert.equal(closedFx.calls.previews.length, 0, "a kept-closed restore read a preview");
    } finally { closedFx.restore(); }
    // Reload with the open entry: still open, same scope, no model call.
    keptPanel = { open: true, channelId: "chan-1", threadId: "t1", view: "flat" };
    const openFx = fixture({ selection: THREAD_T1 });
    try {
      const restored = createDiscussView({ ...openFx.deps, storedPanelState: () => keptPanel, keepPanelState: (s) => { keptPanel = s; } });
      restored.mountControls();
      await flush();
      assert.equal(restored.isOpen(), true, "a kept-open panel did not restore open");
      assert.equal(restored.boundScope()?.threadId, "t1", "the restored scope lost its thread");
      assert.deepEqual(openFx.calls.posts, [], "the restore asked the model");
      assert.equal(openFx.calls.previews.length >= 1, true, "the restore read no preview");
      // A kept scope on another channel does not force this channel open.
      keptPanel = { open: true, channelId: "chan-2", threadId: null, view: "flat" };
      const otherFx = fixture();
      try {
        const other = createDiscussView({ ...otherFx.deps, storedPanelState: () => keptPanel, keepPanelState: () => {} });
        other.mountControls();
        other.sync();
        await flush();
        assert.equal(other.isOpen(), false, "another channel's kept panel forced itself open");
      } finally { otherFx.restore(); }
    } finally { openFx.restore(); }
  } finally { fx.restore(); }
});

test("catchup: with no injected calls, a follow-up sends the api one object body (#1844)", async () => {
  // The seam that never worked live: no `catchupFollowup` is injected, so the
  // view's own `deps.api` fallback runs — and `deps.api` serialises the body
  // itself, so the body it is handed must be the object, never a string.
  const fx = fixture();
  const apiCalls = [];
  const deps = { ...fx.deps };
  delete deps.catchupPost;
  delete deps.catchupPreview;
  delete deps.catchupRecord;
  delete deps.catchupFollowup;
  delete deps.catchupUpdate;
  deps.api = (path, options) => {
    apiCalls.push({ path, options });
    if (path.includes("/catchup/messages")) {
      return Promise.resolve({ you: { speaker: "you", text: "q", text_html: "", at_ms: 1 }, agent: { speaker: "agent", text: "a", text_html: "", at_ms: 1 }, stamp: null });
    }
    if (path.includes("/catchup/preview")) return Promise.resolve(preview());
    if (path.includes("/catchup/record")) return Promise.resolve({ exists: false, current_summary: null, timeline: [], staleness: null });
    return Promise.resolve(answer());
  };
  try {
    const view = createDiscussView(deps);
    view.mountControls();
    view.open();
    await flush();
    click(fx.els["discuss-summarize"]);
    await flush();
    const box = fx.els["discuss-followup"];
    box.value = "Find the message where I ask about the closure work.";
    click(fx.els["discuss-send"]);
    await flush();
    const sent = apiCalls.filter((call) => call.path.includes("/catchup/messages"));
    assert.equal(sent.length, 1, `the follow-up never reached the api: ${JSON.stringify(apiCalls.map((call) => call.path))}`);
    assert.equal(sent[0].options.method, "POST");
    assert.equal(typeof sent[0].options.body, "object", "the api was handed a pre-stringified body");
    assert.deepEqual(sent[0].options.body, { thread_id: null, text: "Find the message where I ask about the closure work." });
    assert.equal(
      JSON.stringify(sent[0].options.body),
      "{\"thread_id\":null,\"text\":\"Find the message where I ask about the closure work.\"}",
      "the wire bytes are one JSON object, not a string inside a string",
    );
  } finally { fx.restore(); }
});

test("catchup: a failed follow-up stands as an error, and the next line clears it (#1844)", async () => {
  const fx = fixture();
  let fail = true;
  fx.deps.catchupFollowup = (channelId, scope, text) => {
    fx.calls.followups.push({ channelId, scope, text });
    if (fail) return Promise.reject(new Error("HTTP 422 invalid_request_body: the body was a string"));
    return Promise.resolve({ you: { speaker: "you", text, text_html: "", at_ms: 1 }, agent: { speaker: "agent", text: "a", text_html: "", at_ms: 1 }, stamp: null });
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    click(fx.els["discuss-summarize"]);
    await flush();
    const box = fx.els["discuss-followup"];
    box.value = "and the deploy?";
    click(fx.els["discuss-send"]);
    await flush();
    const line = fx.els["discuss-status"];
    assert.match(line.textContent, /HTTP 422/);
    assert.equal(line.className, "discuss-status--error", "the failure is not styled as an error");
    assert.equal(line.getAttribute("role"), "alert", "the failure is not announced as one");
    fail = false;
    box.value = "and the deploy?";
    click(fx.els["discuss-send"]);
    await flush();
    assert.equal(line.textContent, "Answered below.");
    assert.equal(line.className, "", "the error styling outlived the failure");
    assert.equal(line.getAttribute("role"), "status");
  } finally { fx.restore(); }
});

// ---- summary search (#2136) ---------------------------------------------------------------------

test("search: words fold case and every word must match", () => {
  assert.deepEqual(summarySearchWords("  Deploy  FAILED "), ["deploy", "failed"]);
  assert.deepEqual(summarySearchWords("   "), []);
  assert.equal(summarySearchMatches("The Deploy failed twice", ["deploy", "failed"]), true);
  assert.equal(summarySearchMatches("The deploy passed", ["deploy", "failed"]), false);
  assert.equal(summarySearchMatches("anything", []), false);
});

test("search: rows keep their topic heading; hits span current and earlier summaries", () => {
  const current = { text: "## Deploys\n- Deploy freeze lifted\n- Docs updated", text_html: "" };
  const earlier = { text: "## Rollback\n- Deploy rollback rehearsed", text_html: "" };
  assert.deepEqual(summarySearchRows(current).map((row) => [row.heading, row.text]), [
    ["Deploys", "Deploy freeze lifted"],
    ["Deploys", "Docs updated"],
  ]);
  const hits = summarySearchHits([
    { place: "Current summary", answer: current },
    { place: "Earlier summary · 09:51", answer: earlier },
  ], "DEPLOY");
  assert.deepEqual(hits.map((hit) => [hit.place, hit.heading, hit.text]), [
    ["Current summary", "Deploys", "Deploy freeze lifted"],
    ["Earlier summary · 09:51", "Rollback", "Deploy rollback rehearsed"],
  ]);
  assert.deepEqual(summarySearchHits([{ place: "Current summary", answer: current }], "deploy docs"), []);
});

test("search: typing in the Discuss search lists labelled hits from current and earlier summaries", async () => {
  const fx = fixture();
  const record = savedRecord();
  record.current_summary.text = "## Deploys\n- Deploy freeze lifted";
  record.timeline[0].summary.text = "## Rollback\n- Deploy rollback rehearsed";
  fx.deps.catchupRecord = (channelId, scope) => {
    fx.calls.records.push({ channelId, scope });
    return Promise.resolve(record);
  };
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    fx.els["discuss-search"].hidden = true;
    click(fx.els["discuss-search-toggle"]);
    assert.equal(fx.els["discuss-search"].hidden, false, "the search icon did not open the search");
    fx.els["discuss-search-input"].value = "deploy";
    fireInput(fx.els["discuss-search-input"]);
    const hits = fx.els["discuss-search-results"].children;
    assert.equal(hits.length, 2, "hits from both summaries were not listed");
    assert.match(hits[0].textContent, /Current summary — Deploys/);
    assert.match(hits[0].textContent, /Deploy freeze lifted/);
    assert.match(hits[1].textContent, /Earlier summary · .* — Rollback/);
    assert.match(hits[1].textContent, /Deploy rollback rehearsed/);
    assert.match(fx.els["discuss-search-status"].textContent, /2 matching rows/);
    fx.els["discuss-search-input"].value = "deploy missingword";
    fireInput(fx.els["discuss-search-input"]);
    assert.equal(fx.els["discuss-search-results"].children.length, 0, "an AND miss still listed rows");
    assert.match(fx.els["discuss-search-status"].textContent, /No summary rows match/);
  } finally { fx.restore(); }
});

test("timestamps: citation links are not handed to the device voice (#1823)", () => {
  assert.equal(withoutTimestampCitations("Ada shipped the fix (09:51 ↩)"), "Ada shipped the fix");
  assert.equal(withoutTimestampCitations("Ada shipped the fix (09:52 ↗). Next."), "Ada shipped the fix. Next.");
  assert.equal(
    withoutTimestampCitations("Ada shipped the fix [(09:51 ↩)](?c=C9&v=all&m=slack:C9:111)"),
    "Ada shipped the fix");
  assert.equal(withoutTimestampCitations("at 13:33 we ship (as planned)"), "at 13:33 we ship (as planned)");
});

// ---- `#2263 vibe-talk-discuss-split-pane` -------------------------------------------------------

test("split: the newest follow-up pair leaves the timeline for Latest; a record ending on a summary has none (#2263)", () => {
  const you = (t) => ({ kind: "turn", speaker: "you", text: "q", at_ms: t });
  const agent = (t) => ({ kind: "turn", speaker: "agent", text: "a", at_ms: t });
  const summary = (t) => ({ kind: "summary", created_ms: t });
  const pair = splitLatestPair([summary(1), you(2), agent(2), you(3), agent(3)]);
  assert.deepEqual(pair.history.map(timelineEntryKey), ["summary:1", "turn:2:you", "turn:2:agent"]);
  assert.deepEqual(pair.latest.map(timelineEntryKey), ["turn:3:you", "turn:3:agent"]);
  // A re-summarize after the last follow-up: the record ends on a summary, so no Latest.
  const none = splitLatestPair([you(2), agent(2), summary(4)]);
  assert.equal(none.latest.length, 0);
  assert.equal(none.history.length, 3);
  // A question still unanswered moves alone; an answer with no question before it moves alone.
  assert.deepEqual(splitLatestPair([summary(1), you(5)]).latest.map(timelineEntryKey), ["turn:5:you"]);
  assert.deepEqual(splitLatestPair([agent(6), agent(7)]).latest.map(timelineEntryKey), ["turn:7:agent"]);
  assert.deepEqual(splitLatestPair([]), { history: [], latest: [] });
});

test("split: a released boundary collapses under its threshold, else keeps at least the floor (#2263)", () => {
  assert.deepEqual(releasedTimeline(0), { collapsed: true, height: TIMELINE_MIN_PX });
  assert.deepEqual(releasedTimeline(TIMELINE_COLLAPSE_BELOW_PX - 1), { collapsed: true, height: TIMELINE_MIN_PX });
  assert.deepEqual(releasedTimeline(TIMELINE_COLLAPSE_BELOW_PX), { collapsed: false, height: TIMELINE_MIN_PX });
  assert.deepEqual(releasedTimeline(180.4), { collapsed: false, height: 180 });
  assert.deepEqual(releasedTimeline(Number.NaN), { collapsed: true, height: TIMELINE_MIN_PX });
});

test("split: the boundary's keys step, page, go to the ends and fold; others are not its (#2263)", () => {
  assert.deepEqual(timelineKeyAct("ArrowUp", 100, 400, 600), { kind: "height", px: 100 + TIMELINE_STEP_PX });
  assert.deepEqual(timelineKeyAct("ArrowDown", 100, 400, 600), { kind: "height", px: 100 - TIMELINE_STEP_PX });
  assert.deepEqual(timelineKeyAct("ArrowDown", TIMELINE_MIN_PX, 400, 600), { kind: "height", px: TIMELINE_MIN_PX }, "a step under the floor");
  assert.deepEqual(timelineKeyAct("ArrowUp", 395, 400, 600), { kind: "height", px: 400 }, "a step past the panel's room");
  assert.deepEqual(timelineKeyAct("PageUp", 100, 400, 600), { kind: "height", px: 250 });
  assert.deepEqual(timelineKeyAct("PageDown", 300, 400, 600), { kind: "height", px: 150 });
  assert.deepEqual(timelineKeyAct("Home", 300, 400, 600), { kind: "height", px: TIMELINE_MIN_PX });
  assert.deepEqual(timelineKeyAct("End", 100, 400, 600), { kind: "height", px: 400 });
  assert.deepEqual(timelineKeyAct("Enter", 100, 400, 600), { kind: "toggle" });
  assert.deepEqual(timelineKeyAct(" ", 100, 400, 600), { kind: "toggle" });
  assert.equal(timelineKeyAct("a", 100, 400, 600), null);
  assert.equal(timelineNewHint(0), "");
  assert.equal(timelineNewHint(1), "1 new summary");
  assert.equal(timelineNewHint(3), "3 new summaries");
});

function splitFixture({ height = null, collapsed = false } = {}) {
  const els = {};
  for (const id of ["discuss-panel", "discuss-timeline-section", "discuss-timeline-head", "discuss-timeline", "discuss-timeline-fold",
    "discuss-timeline-count", "discuss-timeline-new", "discuss-split", "discuss-latest", "discuss-current-section", "discuss-current-head"]) {
    els[id] = fakeElement(id === "discuss-timeline-fold" ? "button" : "div");
    els[id].removeProperty = undefined;
  }
  const styleRemoved = [];
  els["discuss-panel"].style.removeProperty = (name) => { styleRemoved.push(name); delete els["discuss-panel"].style.props[name]; };
  const kept = { height, collapsed, heightKeeps: [], collapsedKeeps: [] };
  const split = createTimelineSplit({
    maybe: (id) => els[id] ?? null,
    storedHeight: () => kept.height,
    keepHeight: (px) => { kept.height = px; kept.heightKeeps.push(px); },
    storedCollapsed: () => kept.collapsed,
    keepCollapsed: (c) => { kept.collapsed = c; kept.collapsedKeeps.push(c); },
  });
  return { els, kept, split, styleRemoved };
}

test("split: the kept height and fold stand on the panel from the device, and every keep goes back to it (#2263)", () => {
  const fx = splitFixture({ height: 180, collapsed: true });
  fx.split.applyStored();
  assert.equal(fx.els["discuss-panel"].style.props["--discuss-timeline-height"], "180px");
  assert.equal(fx.els["discuss-panel"].getAttribute("data-timeline-size"), "kept");
  assert.equal(fx.els["discuss-timeline-section"].getAttribute("data-collapsed"), "true");
  assert.equal(fx.els["discuss-timeline-fold"].getAttribute("aria-expanded"), "false");
  assert.deepEqual([fx.kept.heightKeeps, fx.kept.collapsedKeeps], [[], []], "restating the kept layout kept something");
  // The title folds and unfolds; collapsing never forgets the height.
  fx.split.mount((el, type, handler) => el.addEventListener(type, handler));
  click(fx.els["discuss-timeline-fold"]);
  assert.equal(fx.els["discuss-timeline-section"].getAttribute("data-collapsed"), "false");
  assert.deepEqual(fx.kept.collapsedKeeps, [false]);
  assert.equal(fx.split.height(), 180);
  // The grip's keys: Enter folds, ArrowUp grows by a step and keeps it.
  fire(fx.els["discuss-split"], "keydown", { key: "Enter" });
  assert.equal(fx.split.collapsed(), true);
  fire(fx.els["discuss-split"], "keydown", { key: "Enter" });
  assert.equal(fx.split.collapsed(), false);
  fire(fx.els["discuss-split"], "keydown", { key: "ArrowUp" });
  assert.equal(fx.kept.height, 180 + TIMELINE_STEP_PX);
  assert.equal(fx.els["discuss-panel"].style.props["--discuss-timeline-height"], `${180 + TIMELINE_STEP_PX}px`);
  // A double-click on the header goes back to the stylesheet's default share.
  fire(fx.els["discuss-timeline-head"], "dblclick", { target: fx.els["discuss-timeline-head"] });
  assert.equal(fx.kept.height, null);
  assert.equal(fx.els["discuss-panel"].getAttribute("data-timeline-size"), null);
  assert.ok(fx.styleRemoved.includes("--discuss-timeline-height"), "the default share did not take the kept height off");
});

test("split: collapsed, the header counts its rows and names earlier summaries that arrived, without opening (#2263)", () => {
  const fx = splitFixture();
  fx.split.applyStored();
  fx.split.drawHead("scope-a", 3, 1);
  assert.equal(fx.els["discuss-timeline-count"].textContent, " · 3");
  assert.equal(fx.els["discuss-timeline-new"].hidden, true);
  fx.split.setCollapsed(true, true);
  fx.split.drawHead("scope-a", 4, 1);
  assert.equal(fx.els["discuss-timeline-new"].hidden, true, "a follow-up is not a new summary");
  fx.split.drawHead("scope-a", 5, 2);
  assert.equal(fx.els["discuss-timeline-new"].textContent, "1 new summary");
  assert.equal(fx.els["discuss-timeline-new"].hidden, false);
  assert.equal(fx.split.collapsed(), true, "a new summary opened the timeline");
  // Another scope keeps its own count; back to the first, the hint still stands.
  fx.split.drawHead("scope-b", 2, 2);
  assert.equal(fx.els["discuss-timeline-new"].hidden, true, "a scope first drawn collapsed showed a hint");
  fx.split.drawHead("scope-a", 5, 2);
  assert.equal(fx.els["discuss-timeline-new"].textContent, "1 new summary");
  // Opened: seen, so the hint goes and does not come back for the same summaries.
  fx.split.setCollapsed(false, true);
  assert.equal(fx.els["discuss-timeline-new"].hidden, true);
  fx.split.setCollapsed(true, true);
  fx.split.drawHead("scope-a", 5, 2);
  assert.equal(fx.els["discuss-timeline-new"].hidden, true);
  // An empty history hides the section.
  fx.split.drawHead("scope-a", 0, 0);
  assert.equal(fx.els["discuss-timeline-section"].hidden, true);
});

test("split: a record ending on a follow-up draws that pair open in Latest and the rest in the timeline (#2263)", async () => {
  const fx = fixture();
  for (const id of ["discuss-latest", "discuss-timeline-section", "discuss-timeline-fold", "discuss-timeline-count", "discuss-timeline-new", "discuss-split"]) {
    fx.els[id] = fakeElement(id === "discuss-timeline-fold" ? "button" : "div");
  }
  let record = savedRecord({
    timeline: [
      { kind: "summary", summary: { view: "flat", window: { count: 9, unread_count: 1 }, covered_ids: ["m0"], markable_ids: ["m0"], footer_links: [], text: "older point", text_html: "", unknown_terms: [], stamp: null, created_ms: T1 } },
      { kind: "turn", turn: { speaker: "you", text: "first question", text_html: "", at_ms: T1 + 1 } },
      { kind: "turn", turn: { speaker: "agent", text: "first answer", text_html: "", at_ms: T1 + 1 } },
      { kind: "turn", turn: { speaker: "you", text: "newest question", text_html: "", at_ms: T2 } },
      { kind: "turn", turn: { speaker: "agent", text: "newest answer", text_html: "", at_ms: T2 } },
    ],
  });
  fx.deps.catchupRecord = () => Promise.resolve(record);
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    const latest = fx.els["discuss-latest"];
    assert.equal(latest.hidden, false);
    assert.equal(latest.children.length, 1);
    const pair = latest.children[0];
    assert.equal(pair.open, true, "the Latest pair is not open");
    assert.match(pair.children[0].textContent, /^Latest .* — newest question$/, "the Latest header does not carry the question");
    assert.match(pair.children[1].textContent, /You\s*newest question/);
    assert.match(pair.children[1].textContent, /Discuss\s*newest answer/);
    const rows = [...fx.els["discuss-timeline"].children];
    assert.equal(rows.length, 2, "the timeline still holds the Latest pair");
    assert.match(rows[0].children[0].textContent, /Earlier summary/);
    assert.match(rows[1].children[0].textContent, /Follow-up question .* — first question/);
    assert.equal(rows[1].open ?? false, false, "a timeline pair drew open");
    assert.equal(fx.els["discuss-timeline-count"].textContent, " · 2");
    assert.equal(fx.els["discuss-timeline-section"].hidden, false);
    // A redraw keeps the folds the reader opened (a row closing under the reader would move it).
    rows[0].open = true;
    view.openWith({ channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" });
    await flush();
    assert.equal(fx.els["discuss-timeline"].children[0].open, true, "the redraw closed the row the reader opened");
    assert.notEqual(fx.els["discuss-timeline"].children[0], rows[0], "the timeline was not redrawn");
    // Summarize again: the record ends on a summary, so Latest hides and the pair joins the timeline.
    record = savedRecord({ timeline: [...record.timeline, { kind: "summary", summary: { ...record.timeline[0].summary, created_ms: T2 + 1 } }] });
    // A same-scope reopen restores the answer without a read; the record is read on openWith.
    view.openWith({ channelId: "chan-1", threadId: null, view: "flat", label: "Channel chan-1" });
    await flush();
    assert.equal(latest.hidden, true, "Latest still stands after a summary");
    assert.equal(fx.els["discuss-timeline"].children.length, 4);
  } finally { fx.restore(); }
});

test("split: on a phone's half sheet a Latest pair collapses the timeline by default; the reader's own choice wins and is kept (#2263)", () => {
  // The stylesheet's phone regime, as 22-discuss.css says it: `--discuss-half-sheet: 1` on the
  // half sheet, nothing on a desk. The script reads that and decides nothing itself.
  const savedStyle = globalThis.getComputedStyle;
  let phone = true;
  globalThis.getComputedStyle = (node) => ({
    getPropertyValue: (name) => (name === "--discuss-half-sheet" && phone && node.getAttribute("data-sheet") === "half" ? " 1" : ""),
  });
  try {
    const fx = splitFixture({ collapsed: null });
    fx.els["discuss-panel"].setAttribute("data-sheet", "half");
    fx.split.applyStored();
    fx.split.showLatest(null);
    assert.equal(fx.split.collapsed(), false, "no Latest pair, yet the timeline collapsed");
    fx.split.showLatest("turn:1:you");
    assert.equal(fx.split.collapsed(), true, "a Latest pair on the half sheet did not collapse the timeline");
    assert.deepEqual(fx.kept.collapsedKeeps, [], "the default was kept as if it were the reader's choice");
    // The full sheet keeps the open default; back on the half sheet it collapses again.
    fx.els["discuss-panel"].setAttribute("data-sheet", "full");
    fx.split.restate();
    assert.equal(fx.split.collapsed(), false, "the full sheet collapsed the timeline by default");
    fx.els["discuss-panel"].setAttribute("data-sheet", "half");
    fx.split.restate();
    assert.equal(fx.split.collapsed(), true);
    // The reader opens it: that wins, is kept, and survives the next Latest pair and a reload.
    fx.split.setCollapsed(false, true);
    fx.split.showLatest("turn:2:you");
    assert.equal(fx.split.collapsed(), false, "a new Latest pair overrode the reader's choice");
    assert.deepEqual(fx.kept.collapsedKeeps, [false]);
    fx.split.applyStored();
    assert.equal(fx.split.collapsed(), false, "the reader's choice did not survive a reload");
    // A desk (the stylesheet's desk regime) never takes the half-sheet default.
    phone = false;
    const desk = splitFixture({ collapsed: null });
    desk.els["discuss-panel"].setAttribute("data-sheet", "half");
    desk.split.applyStored();
    desk.split.showLatest("turn:1:you");
    assert.equal(desk.split.collapsed(), false, "a desk took the phone's half-sheet default");
  } finally {
    if (savedStyle === undefined) delete globalThis.getComputedStyle;
    else globalThis.getComputedStyle = savedStyle;
  }
});
