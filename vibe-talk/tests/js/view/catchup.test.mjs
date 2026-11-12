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

import { moduleForTests } from "../../../web/build.mjs";

const VIEW = await moduleForTests("src/view/catchup/index.ts");
const {
  createDiscussView, formatStampLine, DISCUSS_UNAVAILABLE_HINT,
  DISCUSS_BINDING_RULES, WIDEN_OR_SIDEWAYS_ACTION, discussScopeKey, isSameDiscussScope,
  linkDecision, normalizeDiscussScope, viewChangeDecision,
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
    replaceChildren(...kids) { el.children = []; for (const kid of kids) el.appendChild(kid); },
    addEventListener(type, listener) { (el.listeners[type] ||= []).push(listener); },
    removeEventListener(type, listener) { el.listeners[type] = (el.listeners[type] || []).filter((one) => one !== listener); },
    setAttribute(name, value) { el.attributes[name] = String(value); },
    getAttribute(name) { return name in el.attributes ? el.attributes[name] : null; },
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
  "discuss-answer", "discuss-footer", "discuss-covers", "discuss-links",
  "discuss-mark-read", "discuss-mark-unread", "discuss-unmarkable",
  "discuss-stamp", "discuss-retry", "discuss-terms", "discuss-terms-summary", "discuss-terms-list",
  "discuss-stale", "discuss-stale-text", "discuss-update", "discuss-timeline",
  "discuss-followup", "discuss-send",
  "discuss-actions", "discuss-resize",
  "discuss-dock-toggle", "discuss-full-toggle", "screen-main",
  "discuss-summarize", "discuss-count", "discuss-since",
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

function fixture({ selection = { view: "flat", threadId: null, thread: null }, respond } = {}) {
  const els = {};
  for (const id of IDS) els[id] = fakeElement(id.includes("mark") || id.endsWith("open") || id.endsWith("close") || id.endsWith("retry") ? "button" : "div");
  els["discuss-panel"].hidden = true;
  els["discuss-footer"].hidden = true;
  els["discuss-retry"].hidden = true;
  const calls = { posts: [], previews: [], records: [], followups: [], dismiss: [], restore: [], jumps: [], errors: [], statuses: [], templates: [], bound: [], history: [] };
  // `#1344`: the device's kept layout, in memory; the clamp mirrors the device setting's.
  const kept = { width: null, keeps: [], sheetFull: false, sheetKeeps: [], mode: "overlay", modeKeeps: [] };
  const clampW = (value) => {
    const px = Math.round(Number(value));
    return Number.isFinite(px) ? Math.min(960, Math.max(320, px)) : 560;
  };
  let currentSelection = selection;
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
      return Promise.resolve(preview());
    },
    catchupRecord: (channelId, scope) => {
      calls.records.push({ channelId, scope });
      return Promise.resolve({ exists: false, current_summary: null, timeline: [], staleness: null });
    },
    catchupFollowup: (channelId, scope, text) => {
      calls.followups.push({ channelId, scope, text });
      return Promise.resolve({ you: { speaker: "you", text, text_html: "", at_ms: T2 }, agent: { speaker: "agent", text: "followed up", text_html: "", at_ms: T2 }, stamp: null });
    },
    selectedChannelId: () => "chan-1",
    selection: () => currentSelection,
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
  };
  const savedDocument = globalThis.document;
  globalThis.document = {
    createElement: (tag) => {
      if (tag !== "template") return fakeElement(tag);
      // The dom sink's one parser, faked: it records what it was handed and
      // answers one parsed child for it.
      const template = {
        _html: "",
        content: null,
        set innerHTML(value) {
          this._html = String(value);
          this.content = { childNodes: [fakeElement("span")] };
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

test("catchup: the bar button opens and closes; narrowing keeps the bound label (#1395)", async () => {
  // Old expectation (sync relabels to the new scope) was the #1395 bug: an
  // open global Discuss narrowed into a thread stays bound and labelled
  // global (`narrowing-stays-open`); it closes only from its own button.
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    click(fx.els["discuss-open"]);
    assert.equal(view.isOpen(), true);
    assert.equal(fx.calls.posts.length, 0, "opening asked the model");
    await flush();
    fx.setSelection({ view: "thread", threadId: "t2", thread: { title: "Other thread" } });
    view.sync();
    assert.equal(view.isOpen(), true, "narrowing closed the panel");
    assert.equal(fx.els["discuss-scope"].textContent, "Channel chan-1", "narrowing relabelled the bound scope");
    click(fx.els["discuss-open"]);
    assert.equal(view.isOpen(), false);
    assert.equal(fx.els["discuss-open"].getAttribute("aria-expanded"), "false");
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

test("catchup: a late answer after narrowing still lands; after widening it is ignored (#1395)", async () => {
  // Old expectation (any scope change drops the answer) was the #1395 bug
  // for narrowing: the panel stays bound to global, so its answer lands.
  // Widening closes the panel, and a late answer for it is ignored as before.
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
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.", "the narrowed answer was dropped");
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
    assert.equal(view.isOpen(), false, "widening left the panel open");
    widePending.resolve(answer());
    await flush();
    assert.equal(wide.els["discuss-answer"].textContent, "", "the widened answer was drawn");
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
    assert.equal(timeline.length, 2, "the timeline did not draw");
    assert.equal(timeline[1].textContent, "and the deploy?");
    // [Update] is the plain re-summarize: one POST, unbounded.
    click(fx.els["discuss-update"]);
    await flush();
    assert.equal(fx.calls.posts.length, 1);
    assert.equal(fx.calls.posts[0].body.since_ms, null);
  } finally { fx.restore(); }
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
    // Programmatic close: no reserved column, remembered mode kept.
    view.close();
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
    "narrowing-stays-open",
    "in-thread-link-jumps-in-place",
    "out-of-thread-link-returns-to-bound",
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

test("binding: narrowing stays open with its answer; All/Main are one scope", async () => {
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
    // Main -> thread: narrowing, panel stays bound and labelled global.
    fx.setSelection(THREAD_T1);
    view.sync();
    assert.equal(view.isOpen(), true, "narrowing closed the panel");
    assert.equal(view.boundScope().kind, "global", "narrowing rebound the panel");
    assert.equal(fx.els["discuss-scope"].textContent, "Channel chan-1");
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.", "narrowing dropped the answer");
    assert.equal(fx.calls.posts.length, 1, "narrowing asked the model again");
    assert.deepEqual(fx.calls.history, [], "narrowing pushed a history entry");
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

test("binding: an out-of-thread citation returns to the bound global view, panel open", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    fx.setSelection(THREAD_T1);
    view.sync();
    // Another thread's message, then a top-level message: both leave the
    // current thread for the bound global view, panel still open.
    tapCitation(fx, "/?c=chan-1&v=thread&t=t2&m=discord%3Achan-1%3Am9%3At2");
    assert.equal(fx.calls.bound.length, 1, "the tap did not return to the bound view");
    assert.equal(fx.calls.bound[0].scope.threadId, null);
    assert.deepEqual(fx.calls.bound[0].ref, { service: "discord", channel: "chan-1", message: "m9", thread: "t2" });
    tapCitation(fx, "/?c=chan-1&v=all&m=discord%3Achan-1%3Am8");
    assert.equal(fx.calls.bound.length, 2, "a top-level tap did not return to the bound view");
    assert.equal(view.isOpen(), true, "a return-to-bound tap closed the panel");
    assert.deepEqual(fx.calls.jumps, [], "a return-to-bound tap jumped in place");
  } finally { fx.restore(); }
});

test("binding: widening closes behind a history entry; Back restores it with no model call", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    const postsBefore = fx.calls.posts.length;
    const previewsBefore = fx.calls.previews.length;
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.sync();
    assert.equal(view.isOpen(), false, "widening left the panel open");
    assert.equal(fx.calls.history.length, 1, "widening pushed no history entry");
    const entry = fx.calls.history[0];
    assert.equal(entry.discussOpen, true);
    assert.equal(entry.recordKey, "chan-1:thread:t1", "the entry carries no record key");
    assert.equal(entry.scope.threadId, "t1");
    assert.equal(entry.snapshot.answer.text, "Two things happened.");
    // Back: the thread view again, Discuss open, same summary, zero calls.
    fx.setSelection(THREAD_T1);
    assert.equal(view.restoreFromHistory(entry), true);
    assert.equal(view.isOpen(), true, "Back did not reopen the panel");
    assert.equal(fx.els["discuss-answer"].textContent, "Two things happened.", "Back showed another summary");
    assert.equal(fx.els["discuss-scope"].textContent, "First thread");
    assert.equal(fx.calls.posts.length, postsBefore, "Back spent a model call");
    assert.equal(fx.calls.previews.length, previewsBefore, "Back read a preview");
    // Opening fresh in All binds global.
    view.close();
    fx.setSelection({ view: "flat", threadId: null, thread: null });
    view.open();
    await flush();
    assert.equal(view.boundScope().kind, "global");
  } finally { fx.restore(); }
});

test("binding: sideways closes; a fallback tap closes and jumps; an out-of-scope tap follows it", async () => {
  const fx = fixture({ selection: THREAD_T1 });
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    await openAndSummarize(view, fx);
    await flush();
    // Sideways: thread t1 -> thread t2 ends the binding.
    fx.setSelection({ view: "thread", threadId: "t2", thread: { title: "Second thread" } });
    view.sync();
    assert.equal(view.isOpen(), false, "a sideways switch left the panel open");
    assert.equal(fx.calls.history.length, 1);
    // Back, then a citation for the other thread: fallback close + jump.
    fx.setSelection(THREAD_T1);
    assert.equal(view.restoreFromHistory(fx.calls.history[0]), true);
    tapCitation(fx, "/?c=chan-1&v=thread&t=t2&m=discord%3Achan-1%3Am9%3At2");
    assert.equal(view.isOpen(), false, "a fallback tap left the panel open");
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
