// Tests for web/src/view/documents/ on its own: the modules built as the page build builds them
// and held directly, with no page around them (`#624 voice-page-modularity` style, as in
// voice.test.mjs / dom.test.mjs). Covers the Documents view draft contracts:
//   1. selector: nextView rotates voice->discord->docs->voice; otherViews gives the two others in
//      next-rotation order; VIEW_LABELS are Voice/Channel/Documents.
//   2. render: renderDocumentMarkdown renders heading/paragraph/bold/code/link, and a malicious
//      document renders INERT (no <script, onerror, javascript:, <style, <img) while its visible
//      text still appears as text.
//   3. list: refresh() renders the API's rows in order with the unfetched one dimmed; clicking the
//      unfetched card fetches (POST) then opens; clicking the fetched card opens without fetching.
// Deterministic: no network, no timers. The list test drives the module through fake API/element
// seams in the minimal plain-object style of the existing view tests.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

// Load each module on its own so a missing module fails only its own contract's tests, with the
// build error in the message, instead of taking the whole file down before any test registers.
async function load(entry) {
  try {
    return { mod: await moduleForTests(entry), error: null };
  } catch (error) {
    return { mod: null, error };
  }
}

const SELECTOR = await load("src/view/documents/selector.ts");
const RENDER = await load("src/view/documents/render.ts");
const LIST = await load("src/view/documents/list.ts");

function requireModule(loaded, entry) {
  assert.ok(loaded.mod, `${entry} did not load: ${loaded.error ? loaded.error.message : "unknown error"}`);
  return loaded.mod;
}

// ---- 1. selector -----------------------------------------------------------------------------

test("documents selector: nextView rotates voice -> discord -> docs -> voice", () => {
  const { nextView } = requireModule(SELECTOR, "src/view/documents/selector.ts");
  assert.equal(typeof nextView, "function", "selector does not export a nextView function");
  assert.equal(nextView("voice"), "discord");
  assert.equal(nextView("discord"), "docs");
  assert.equal(nextView("docs"), "voice");
  // Three steps from anywhere come back to the start.
  for (const view of ["voice", "discord", "docs"]) {
    assert.equal(nextView(nextView(nextView(view))), view, `rotation does not close at ${view}`);
  }
});

test("documents selector: otherViews returns the two others in next-rotation order", () => {
  const { otherViews } = requireModule(SELECTOR, "src/view/documents/selector.ts");
  assert.equal(typeof otherViews, "function", "selector does not export an otherViews function");
  assert.deepEqual([...otherViews("voice")], ["discord", "docs"]);
  assert.deepEqual([...otherViews("discord")], ["docs", "voice"]);
  assert.deepEqual([...otherViews("docs")], ["voice", "discord"]);
});

test("documents selector: VIEW_LABELS are Voice/Channel/Documents", () => {
  const { VIEW_LABELS } = requireModule(SELECTOR, "src/view/documents/selector.ts");
  assert.ok(VIEW_LABELS, "selector does not export VIEW_LABELS");
  const label = (view) => (VIEW_LABELS instanceof Map ? VIEW_LABELS.get(view) : VIEW_LABELS[view]);
  assert.equal(label("voice"), "Voice");
  assert.equal(label("discord"), "Channel");
  assert.equal(label("docs"), "Documents");
});

// ---- 2. render -------------------------------------------------------------------------------

// What the renderer answers, as HTML text: a string as-is, an { html } shape, or a node's markup.
function htmlOf(rendered) {
  if (typeof rendered === "string") return rendered;
  if (rendered && typeof rendered.html === "string") return rendered.html;
  if (rendered && typeof rendered.innerHTML === "string") return rendered.innerHTML;
  if (rendered && typeof rendered.outerHTML === "string") return rendered.outerHTML;
  return String(rendered ?? "");
}

test("documents render: a heading/paragraph/bold/code/link document renders as markup", () => {
  const { renderDocumentMarkdown } = requireModule(RENDER, "src/view/documents/render.ts");
  assert.equal(typeof renderDocumentMarkdown, "function", "render does not export a renderDocumentMarkdown function");
  const html = htmlOf(renderDocumentMarkdown("# Release notes\n\nShip **today** with `fast` code and [the plan](https://example.com/plan).\n"));
  assert.match(html, /<h1[^>]*>Release notes<\/h1>/i, "heading not rendered as a heading");
  assert.match(html, /<p[^>]*>/i, "paragraph not rendered as a paragraph");
  assert.match(html, /<(strong|b)[^>]*>today<\/(strong|b)>/i, "bold not rendered as emphasis markup");
  assert.match(html, /<code[^>]*>fast<\/code>/i, "inline code not rendered as code");
  assert.match(html, /<a[^>]*href="https:\/\/example\.com\/plan"[^>]*>the plan<\/a>/i, "link not rendered as a safe anchor");
});

test("documents render: a malicious document renders inert, its text still readable as text", () => {
  const { renderDocumentMarkdown } = requireModule(RENDER, "src/view/documents/render.ts");
  const malicious = [
    "# Notes",
    "",
    "<script>alert(1)</script>",
    "",
    "<img src=x onerror=alert(1)>",
    "",
    "[x](javascript:alert(1))",
    "",
    "[x](data:text/html;base64,AAA)",
    "",
    "<style>body { color: red; }</style>",
  ].join("\n");
  const html = htmlOf(renderDocumentMarkdown(malicious));
  const lower = html.toLowerCase();
  assert.ok(!lower.includes("<script"), "output carries a live <script tag");
  // The handler's name may survive as escaped visible text; what must not survive is a live tag carrying it.
  assert.ok(!/<[^>]*\son\w+\s*=/i.test(html), "output carries a live event-handler attribute");
  assert.ok(!lower.includes("javascript:"), "output carries a javascript: address");
  assert.ok(!lower.includes("<style"), "output carries a live <style tag");
  assert.ok(!lower.includes("<img"), "output carries a live <img tag");
  // The payloads' visible text survives as inert text (escaped markup or link text), not as code.
  assert.ok(html.includes("alert(1)"), "the script payload's text vanished instead of rendering as text");
});

// ---- 3. list ---------------------------------------------------------------------------------

// A minimal fake element in the style of the existing view tests: plain object with class/text/
// children and the listener surface a card list uses. Backs both deps-supplied elements and a
// global document.createElement seam.
function fakeElement(tagName = "div") {
  const classes = new Set();
  const el = {
    tagName: tagName.toUpperCase(),
    children: [],
    childNodes: [],
    listeners: {},
    dataset: {},
    attributes: {},
    hidden: false,
    classList: {
      add: (...names) => { for (const n of names) classes.add(n); sync(); },
      remove: (...names) => { for (const n of names) classes.delete(n); sync(); },
      toggle: (name, on) => { if (on === undefined ? !classes.has(name) : on) classes.add(name); else classes.delete(name); sync(); },
      contains: (name) => classes.has(name),
    },
    append(...kids) { for (const kid of kids) el.appendChild(kid); },
    appendChild(kid) { el.children.push(kid); el.childNodes.push(kid); if (kid && typeof kid === "object") kid.parentNode = el; return kid; },
    replaceChildren(...kids) { el.children = []; el.childNodes = []; el.append(...kids); },
    addEventListener(type, listener) { (el.listeners[type] ||= []).push(listener); },
    removeEventListener(type, listener) { el.listeners[type] = (el.listeners[type] || []).filter((one) => one !== listener); },
    setAttribute(name, value) { el.attributes[name] = String(value); },
    getAttribute(name) { return name in el.attributes ? el.attributes[name] : null; },
    hasAttribute(name) { return name in el.attributes; },
    removeAttribute(name) { delete el.attributes[name]; },
  };
  let className = "";
  let textContent = "";
  let innerHTML = "";
  const sync = () => { className = [...classes].join(" "); };
  Object.defineProperties(el, {
    className: {
      get: () => className,
      set: (value) => { className = String(value ?? ""); classes.clear(); for (const n of className.split(/\s+/).filter(Boolean)) classes.add(n); },
    },
    textContent: {
      get: () => textContent + el.children.map((kid) => (typeof kid === "string" ? kid : kid?.textContent ?? "")).join(""),
      set: (value) => { textContent = String(value ?? ""); if (value) { el.children = []; el.childNodes = []; } },
    },
    innerHTML: {
      get: () => innerHTML,
      set: (value) => { innerHTML = String(value ?? ""); },
    },
  });
  el.click = () => dispatchClick(el);
  return el;
}

// Fire a click the way the module wired it: its own listeners, an onclick property, or nothing.
function dispatchClick(el, event = {}) {
  const ev = { target: el, currentTarget: el, preventDefault() {}, stopPropagation() {}, ...event };
  const results = [];
  if (typeof el.onclick === "function") results.push(el.onclick(ev));
  for (const listener of [...(el.listeners.click || [])]) results.push(listener(ev));
  return results;
}

async function flush(pending) {
  await Promise.all(pending.flat(Infinity).filter((one) => one && typeof one.then === "function"));
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

function classText(el) {
  return `${el.className || ""} ${[...(el.classList ? [] : [])]}`;
}

test("documents list: refresh renders rows in order, unfetched dimmed; clicks fetch-then-open / open directly", async () => {
  const mod = requireModule(LIST, "src/view/documents/list.ts");
  const factory = mod.createDocumentList || mod.createDocumentsList || mod.mountDocumentList || mod.mountDocumentsList || mod.createList;
  assert.equal(typeof factory, "function",
    `list exports no list factory (found: ${Object.keys(mod).join(", ") || "none"})`);

  const rows = [
    { key: "doc-alpha", id: "doc-alpha", url: "https://example.com/alpha.md", title: "Alpha doc", file_name: "alpha.md", mention_count: 1, kind: "github_markdown", fetched: true, fetched_ms: 1000, moved_to: null },
    // Unfetched: the server's title can carry the linking message's words; the card names the file.
    { key: "doc-beta", id: "doc-beta", url: "https://example.com/beta.md", title: "a whole linking message, not the document", file_name: "beta.md", mention_count: 2, kind: "github_markdown", fetched: false, fetched_ms: null, moved_to: null },
  ];
  // One payload readable as a bare array or under documents/rows/items, whichever the module reads.
  const listPayload = () => { const out = [...rows]; out.documents = rows; out.rows = rows; out.items = rows; return out; };

  const order = [];
  const fetchCalls = [];
  const api = {
    async request(path, options = {}) {
      const method = (options.method || "GET").toUpperCase();
      if (method === "POST") { fetchCalls.push({ path, options }); order.push("fetch"); return { key: "doc-beta", fetched: true }; }
      return listPayload();
    },
    async decoded(_type, path, options) { return api.request(path, options); },
    async list() { return listPayload(); },
    async listDocuments() { return listPayload(); },
    async documents() { return listPayload(); },
    async fetchDocument(key) { fetchCalls.push({ key }); order.push("fetch"); return { key, fetched: true }; },
    async fetch(key) { fetchCalls.push({ key }); order.push("fetch"); return { key, fetched: true }; },
  };

  const container = fakeElement("div");
  const opened = [];
  const onOpen = (what) => { opened.push(what); order.push(`open:${typeof what === "string" ? what : (what?.key || what?.id || what?.title || "?")}`); };
  // The list's real seam is the page's `api(path, options)` function; offer it callable.
  const apiFn = async (path, options = {}) => api.request(path, options);
  const deps = {
    api: apiFn,
    container,
    root: container,
    element: container,
    listElement: container,
    onOpen,
    open: onOpen,
    onSelect: onOpen,
    createElement: (tag) => fakeElement(tag),
    document: { createElement: (tag) => fakeElement(tag), createElementNS: (_ns, tag) => fakeElement(tag), createTextNode: (text) => ({ nodeType: 3, textContent: String(text) }) },
    byId: () => container,
  };

  const savedDocument = globalThis.document;
  const savedFetch = globalThis.fetch;
  globalThis.document = deps.document;
  globalThis.fetch = async (path, options = {}) => {
    const method = (options.method || "GET").toUpperCase();
    if (method === "POST") { fetchCalls.push({ path, options }); order.push("fetch"); }
    const payload = method === "POST" ? { key: "doc-beta", fetched: true } : listPayload();
    return { ok: true, status: 200, json: async () => payload, text: async () => JSON.stringify(payload) };
  };
  try {
    // Construct through whichever seam the factory takes: deps alone, or api first.
    let list = null;
    let lastError = null;
    for (const args of [[deps], [api, deps], [container, deps], [deps, container]]) {
      try {
        const made = factory(...args);
        const candidate = made && typeof made.then === "function" ? await made : made;
        if (candidate && typeof candidate.refresh === "function") { list = candidate; break; }
        if (candidate && typeof candidate === "object") lastError = new Error(`factory answered without refresh(): ${Object.keys(candidate).join(", ")}`);
      } catch (error) { lastError = error; }
    }
    assert.ok(list, `no construction seam yielded a list with refresh()${lastError ? `: ${lastError.message}` : ""}`);

    await list.refresh();

    const cards = [...(container.children.length ? container.children : container.childNodes)]
      .filter((kid) => kid && typeof kid === "object" && String(kid.className).includes("doc-card"));
    assert.equal(cards.length, 2, `refresh rendered ${cards.length} cards, not two (container HTML: ${String(container.innerHTML).slice(0, 200)})`);
    assert.ok(cards[0].textContent.includes("Alpha doc"), "first card is not the first row in order");
    assert.ok(cards[1].textContent.includes("beta.md"), "second card does not name its file");
    assert.ok(!cards[1].textContent.includes("a whole linking message"), "the unfetched card carries the linking message");

    const dimmed = (el) => /dim|unfetch|not-fetched|pending/i.test(`${el.className || ""}`);
    assert.equal(dimmed(cards[0]), false, "the fetched card carries the dimmed class");
    assert.equal(dimmed(cards[1]), true, `the unfetched card carries no dimmed class (className: ${JSON.stringify(cards[1].className)})`);

    // Clicking the unfetched card fetches (POST) first, then opens it.
    order.length = 0; fetchCalls.length = 0; opened.length = 0;
    let pending = dispatchClick(cards[1]);
    if (pending.length === 0 && (container.listeners.click || []).length) {
      pending = dispatchClick(container, { target: cards[1] });
    }
    await flush(pending);
    assert.ok(fetchCalls.length > 0, "clicking the unfetched card fetched nothing");
    assert.ok(order.indexOf("fetch") > -1 && order.findIndex((entry) => entry.startsWith("open:")) > order.indexOf("fetch"),
      `the unfetched card did not fetch before opening (order: ${order.join(", ")})`);
    assert.equal(opened.length, 1, "clicking the unfetched card did not open it exactly once");
    assert.ok(JSON.stringify(opened[0]).includes("doc-beta") || JSON.stringify(opened[0]).includes("Beta doc"),
      "onOpen was not handed the unfetched row");

    // Clicking the fetched card opens it without fetching. (Re-query: fetching a
    // row redraws the list, so the first render's cards are detached by now.)
    order.length = 0; fetchCalls.length = 0; opened.length = 0;
    const fresh = [...(container.children.length ? container.children : container.childNodes)]
      .filter((kid) => kid && typeof kid === "object" && String(kid.className).includes("doc-card"));
    pending = dispatchClick(fresh[0]);
    if (pending.length === 0 && (container.listeners.click || []).length) {
      pending = dispatchClick(container, { target: fresh[0] });
    }
    await flush(pending);
    assert.equal(fetchCalls.length, 0, "clicking the fetched card fetched anyway");
    assert.equal(opened.length, 1, "clicking the fetched card did not open it exactly once");
    assert.ok(JSON.stringify(opened[0]).includes("doc-alpha") || JSON.stringify(opened[0]).includes("Alpha doc"),
      "onOpen was not handed the fetched row");
  } finally {
    if (savedDocument === undefined) delete globalThis.document; else globalThis.document = savedDocument;
    if (savedFetch === undefined) delete globalThis.fetch; else globalThis.fetch = savedFetch;
  }
});
