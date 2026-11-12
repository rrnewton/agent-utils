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
const { createDiscussView, formatStampLine, DISCUSS_UNAVAILABLE_HINT } = VIEW;

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
  "discuss-actions", "discuss-resize", "discuss-sheet-toggle",
  "discuss-mode-overlay", "discuss-mode-docked", "discuss-mode-maximized", "screen-main",
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
  const calls = { posts: [], previews: [], dismiss: [], restore: [], jumps: [], errors: [], statuses: [], templates: [] };
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
    selectedChannelId: () => "chan-1",
    selection: () => currentSelection,
    serviceName: () => "discord",
    channelLabel: (id) => `Channel ${id}`,
    onJump: (ref) => calls.jumps.push(ref),
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
    assert.equal(fx.els["discuss-summarize"].textContent, "Summarize last 2 unread since 09:51");
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
    assert.equal(fx.els["discuss-summarize"].textContent, "Summarize last 1 unread since 09:52");
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

test("catchup: the bar button opens and closes; sync restates the scope label", async () => {
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
    assert.equal(fx.els["discuss-scope"].textContent, "Other thread");
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
  } finally { fx.restore(); }

  const plain = fixture({ respond: () => Promise.resolve(answer({ text: "<b>not markup</b>" })) });
  try {
    const view = createDiscussView(plain.deps);
    view.mountControls();
    await openAndSummarize(view, plain);
    await flush();
    assert.equal(plain.els["discuss-answer"].innerHTMLSet, false, "plain text reached innerHTML");
    assert.equal(plain.els["discuss-answer"].textContent, "<b>not markup</b>");
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
    assert.equal(fx.els["discuss-covers"].textContent, "This summary covers 2 unread messages:");
    const items = fx.els["discuss-links"].children;
    assert.equal(items.length, 2);
    assert.equal(items[0].children[0].textContent, "09:51 · Ada");
    click(items[1].children[0]);
    assert.deepEqual(fx.calls.jumps, [{ service: "discord", channel: "chan-1", message: "m2", thread: "t9" }]);
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

test("catchup: a late answer after a scope change is ignored", async () => {
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
    assert.equal(fx.els["discuss-answer"].textContent, "", "the stale answer was drawn");
    assert.equal(fx.els["discuss-status"].textContent, "Reading…", "the stale answer restated the status");
    assert.equal(fx.els["discuss-footer"].hidden, true, "the stale answer drew its footer");
  } finally { fx.restore(); }
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

test("catchup layout: the sheet starts at half, Maximize sticks across close/reopen", async () => {
  const fx = fixture();
  try {
    const view = createDiscussView(fx.deps);
    view.mountControls();
    view.open();
    await flush();
    assert.equal(view.sheetFull(), false, "the sheet did not start at half");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-sheet"), "half");
    assert.equal(fx.els["discuss-sheet-toggle"].textContent, "Maximize");
    click(fx.els["discuss-sheet-toggle"]);
    assert.equal(view.sheetFull(), true, "Maximize did not stand the sheet full");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-sheet"), "full");
    assert.equal(fx.els["discuss-sheet-toggle"].textContent, "Restore");
    assert.deepEqual(fx.kept.sheetKeeps, [true], "the full height was not kept");
    view.close();
    view.open();
    await flush();
    assert.equal(view.sheetFull(), true, "the full height did not stick across close/reopen");
    assert.equal(fx.els["discuss-panel"].getAttribute("data-sheet"), "full");
    click(fx.els["discuss-sheet-toggle"]);
    assert.equal(view.sheetFull(), false, "Restore did not return the sheet to half");
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
    click(fx.els["discuss-mode-docked"]);
    assert.equal(view.mode(), "docked");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "docked");
    assert.equal(fx.els["discuss-mode-docked"].getAttribute("aria-pressed"), "true");
    assert.equal(fx.els["discuss-mode-overlay"].getAttribute("aria-pressed"), "false");
    assert.deepEqual(fx.kept.modeKeeps, ["docked"], "the docked choice was not kept");
    view.close();
    view.open();
    await flush();
    assert.equal(view.mode(), "docked", "the mode did not survive close/reopen");
    view.setMode("maximized");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), "maximized");
    view.setMode("overlay");
    assert.equal(fx.els["screen-main"].getAttribute("data-discuss"), null, "overlay left the screen marked");
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
