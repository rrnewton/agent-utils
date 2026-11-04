// Tests for web/src/view/dom/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The message boxes' keys and growth (web/src/view/composer). */
const VIEW_COMPOSER = await moduleForTests("src/view/composer/index.ts");
/** The DOM primitives: walks, the shell's elements, focus (web/src/view/dom). */
const VIEW_DOM = await moduleForTests("src/view/dom/index.ts");

test("`#624 voice-page-modularity`: the DOM walks and the box helpers, on plain nodes, from the module alone", () => {
  const { nodeWithin, shownNode, childByClass } = VIEW_DOM;
  const node = (className, children = [], extra = {}) => {
    const made = { className, children, nodeType: 1, hidden: false, parentNode: null, ...extra };
    for (const child of children) child.parentNode = made;
    return made;
  };
  const author = node("msg-author reading-mark");
  const inText = node("archive");
  const body = node("md", [inText]);
  const meta = node("meta", [author]);
  const row = node("row", [meta, body]);
  assert.deepStrictEqual([nodeWithin(row, author), nodeWithin(meta, inText), nodeWithin(row, row), nodeWithin(row, null)], [true, false, true, false]);
  assert.equal(childByClass(row, "reading-mark"), author, "a class among others was not found");
  assert.equal(childByClass(row, "archive"), null, "a part was looked for inside the message's own text");
  assert.equal(row.partsFound.get("reading-mark"), author, "what was found was not remembered");
  // Remembered, but no longer carrying the class: walked for again.
  author.className = "msg-author";
  assert.equal(childByClass(row, "reading-mark"), null);
  assert.equal(shownNode(author), true);
  meta.hidden = true;
  assert.equal(shownNode(author), false, "a node inside a hidden one counted as shown");
  // The composer's helpers built on them.
  const { isComposer, focusAtEnd } = VIEW_COMPOSER;
  assert.deepStrictEqual([isComposer({ id: "reply-text" }), isComposer({ id: "search-input" }), isComposer(null)], [true, false, false]);
  const calls = [];
  const box = { value: "draft", focus: (options) => calls.push(["focus", options]), scrollIntoView: (options) => calls.push(["scroll", options]),
    setSelectionRange: (start, end) => calls.push(["caret", start, end]) };
  focusAtEnd(box);
  assert.deepStrictEqual(calls, [["focus", { preventScroll: true }], ["scroll", { block: "nearest", inline: "nearest" }], ["caret", 5, 5]]);
});

test("`#624 voice-page-modularity`: where a key landed — a field typed in, a control keeping its own keys, a link — read from the target alone", () => {
  const { isTextField, keepsOwnKeys, pressedBySpace, isLink, answersEnter, scrollsItself } = VIEW_DOM;
  const node = (tagName, fields = {}) => ({ tagName, ...fields });
  for (const missing of [null, undefined, "", 0]) {
    assert.equal(isTextField(missing), false);
    assert.equal(keepsOwnKeys(missing), false);
    assert.equal(pressedBySpace(missing), false);
    assert.equal(isLink(missing), false);
    assert.equal(answersEnter(missing), false);
  }
  assert.equal(isTextField(node("input")), true, "an input of no type is text");
  assert.equal(isTextField(node("INPUT", { type: "Search" })), true);
  assert.equal(isTextField(node("INPUT", { type: "checkbox" })), false);
  assert.equal(isTextField(node("textarea")), true);
  assert.equal(isTextField(node("DIV", { isContentEditable: true })), true);
  assert.equal(isTextField(node("DIV", { isContentEditable: "true" })), false);
  assert.equal(keepsOwnKeys(node("select")), true);
  assert.equal(keepsOwnKeys(node("INPUT", { type: "range" })), true);
  assert.equal(keepsOwnKeys(node("DIV", { getAttribute: (name) => (name === "role" ? "slider" : null) })), true);
  assert.equal(keepsOwnKeys(node("DIV", { getAttribute: () => "button" })), false);
  assert.equal(keepsOwnKeys(node("DIV", { getAttribute: "separator" })), false, "a getAttribute that is no function is not called");
  assert.equal(pressedBySpace(node("summary")), true);
  assert.equal(pressedBySpace(node("A")), false);
  assert.equal(isLink(node("a")), true);
  assert.equal(answersEnter(node("A")), true);
  assert.equal(answersEnter(node("DIV", { isContentEditable: true })), true);
  assert.equal(answersEnter(node("LI")), false);
  assert.equal(scrollsItself(node("TEXTAREA", { scrollHeight: 120, clientHeight: 100 })), true);
  assert.equal(scrollsItself(node("TEXTAREA", { scrollHeight: 101, clientHeight: 100 })), false, "a pixel's rounding is not a page");
  assert.equal(scrollsItself(node("DIV", { scrollHeight: 500, clientHeight: 100 })), false);
});

test("`#624 voice-page-modularity`: plain text goes on the clipboard by its API, else through a hidden textarea, and says whether it went", async () => {
  const { copyText } = VIEW_DOM;
  const written = [];
  assert.equal(await copyText("hello", () => ({ writeText: async (text) => { written.push(text); } })), true);
  assert.deepStrictEqual(written, ["hello"]);
  const was = globalThis.document;
  const events = [];
  const focused = { focus: () => events.push("focus back") };
  globalThis.document = {
    activeElement: focused,
    body: { append: (node) => { events.push("append"); node.parentNode = { removeChild: () => events.push("remove") }; } },
    createElement: () => ({ style: {}, setAttribute: () => {}, focus: () => events.push("focus scratch"), select: () => events.push("select") }),
    execCommand: (command) => { events.push(command); return true; },
  };
  try {
    assert.equal(await copyText("x", () => ({ writeText: async () => { throw new Error("NotAllowedError"); } })), true,
      "a refused API falls back to the textarea");
    assert.deepStrictEqual(events, ["append", "focus scratch", "select", "copy", "remove", "focus back"]);
    events.length = 0;
    assert.equal(await copyText("x", () => { throw new ReferenceError("navigator is not defined"); }), true,
      "a host with no clipboard falls back too");
    globalThis.document.execCommand = undefined;
    assert.equal(await copyText("x", () => null), false, "neither works: says so");
  } finally {
    globalThis.document = was;
  }
});

/** What stands over every screen (web/src/view/shell). */
const VIEW_SHELL = await moduleForTests("src/view/shell/index.ts");

/** A shell's elements, each with its text, its hidden state and its attributes. */
function elements() {
  const made = new Map();
  return (id) => {
    if (!made.has(id)) {
      const attrs = new Map();
      made.set(id, { id, hidden: true, textContent: "", setAttribute: (name, value) => attrs.set(name, value), getAttribute: (name) => attrs.get(name) ?? null });
    }
    return made.get(id);
  };
}

test("`#624 voice-page-modularity`: the status line says a thing, takes itself away, and falls back while the service is silent", () => {
  const { createShellNotices, STATUS_DISMISS_MS } = VIEW_SHELL;
  const el = elements();
  const timers = [];
  let silent = null;
  const notices = createShellNotices({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    el, redact: (text) => String(text).replace("secret-token-1", "[redacted]"), unresponsive: () => silent,
    holdingReader: (mutate) => mutate(), detailChanged: () => {},
  });
  notices.setStatus("Saved with secret-token-1.");
  assert.equal(el("status-line").hidden, false);
  assert.equal(el("status").textContent, "Saved with [redacted].", "a credential never reaches the line");
  assert.equal(timers[0].ms, STATUS_DISMISS_MS);
  timers[0].run();
  assert.equal(el("status-line").hidden, true);
  assert.equal(el("status").textContent, "Saved with [redacted].", "a hide, not an erase");
  silent = "The voice service is not answering.";
  notices.setStatus("Something else.");
  notices.dismissStatus();
  assert.equal(el("status-line").hidden, false, "while the service is silent the line says so");
  assert.equal(el("status").textContent, silent);
  silent = null;
  notices.setStatus("Ready.");
  notices.holdStatus();
  assert.equal(timers.at(-1), null, "a held line has no timer to take it away");
  assert.equal(el("status-line").hidden, false);
});

test("`#624 voice-page-modularity`: the error panel stays twelve seconds, or until acted on, and the details are kept for Settings", () => {
  const { createShellNotices, BANNER_DISMISS_MS } = VIEW_SHELL;
  const el = elements();
  const timers = [];
  const details = [];
  const notices = createShellNotices({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    el, redact: (text) => String(text), unresponsive: () => null, holdingReader: (mutate) => mutate(),
    detailChanged: (text) => details.push(text),
  });
  notices.showError("HTTP 502");
  assert.equal(el("error-wrap").hidden, false);
  assert.equal(el("status-line").getAttribute("data-state"), "error");
  assert.equal(timers.at(-1).ms, 12000);
  timers.at(-1).run();
  assert.equal(el("error-wrap").hidden, true);
  notices.showError("The call dropped.", { stay: true });
  assert.equal(timers.filter(Boolean).length, 0, "a reason that stays has no timer");
  notices.clearError();
  assert.equal(el("error").textContent, "");
  notices.showDetail("live stream attached");
  notices.addDetail("dropped once");
  assert.deepStrictEqual(details, ["live stream attached", "live stream attached · dropped once"]);
  assert.equal(el("connection-banner").hidden, false);
  assert.equal(timers.at(-1).ms, BANNER_DISMISS_MS);
  timers.at(-1).run();
  assert.equal(el("connection-banner").hidden, true);
});

test("`#624 voice-page-modularity`: the chips stand on the channel's box where it reaches into their strip", () => {
  const { createChipsOverComposer } = VIEW_SHELL;
  const props = new Map();
  let steered = 0;
  const box = (fields) => ({ hidden: false, offsetHeight: 0, parentNode: null, getAttribute: () => null,
    getBoundingClientRect: () => ({ top: 0, bottom: 0, height: 0 }), ...fields });
  const els = {
    "post-confirm": box({ hidden: true }),
    "pane-discord": box({}),
    "scroll-area": box({ scrollHeight: 2000, scrollTop: 1600, clientHeight: 400 }),
    "channel-composer": box({ offsetHeight: 80, getBoundingClientRect: () => ({ top: 700, bottom: 780 }) }),
    "screen-main": box({ getBoundingClientRect: () => ({ bottom: 800 }) }),
    "scroll-tools": box({ getBoundingClientRect: () => ({ height: 40 }) }),
    "read-steer": box({ hidden: true }),
    "frame-body": box({ style: { setProperty: (name, value) => props.set(name, value) } }),
  };
  const chips = createChipsOverComposer({
    lookup: (id) => els[id], el: (id) => els[id], onChannel: () => true, rootFontPx: () => 16,
    root: () => ({ getAttribute: () => null }), placeReadSteer: () => { steered += 1; },
  });
  chips.placeChipsOverComposer();
  assert.equal(props.get("--composer-top"), "100px", "the box's top edge is 100px above the foot");
  assert.equal(steered, 1);
  chips.placeChipsOverComposer();
  assert.equal(steered, 1, "nothing moved, nothing held again");
  els["scroll-area"].scrollTop = 0;
  chips.placeChipsOverComposer();
  assert.equal(props.get("--composer-top"), "0px", "a box below the screen leaves them at the foot");
});

test("`#624 voice-page-modularity`: undo stamps only rise, so two offers up at once always have an order", () => {
  const stamps = VIEW_SHELL.createUndoStamps();
  assert.deepStrictEqual([stamps.next(), stamps.next(), stamps.next()], [1, 2, 3]);
  assert.equal(VIEW_SHELL.createUndoStamps().next(), 1, "each page load counts from the start");
});

test("`#624 voice-page-modularity`: the screens remember where Settings and Help go back to, as each screen is arrived at", () => {
  const { createScreens, SCREENS } = VIEW_SHELL;
  assert.deepStrictEqual([...SCREENS], ["signin", "main", "settings", "help"]);
  const screens = createScreens();
  assert.deepStrictEqual([screens.screen(), screens.view(), screens.beforeSettings(), screens.helpReturn()], ["signin", "voice", "signin", "settings"]);
  screens.arrive("main", "signin");
  screens.arrive("settings", "main");
  assert.equal(screens.beforeSettings(), "main", "Settings goes back to the screen it was opened from");
  screens.arrive("help", "settings");
  assert.equal(screens.beforeSettings(), "main", "Help is never what Settings goes back to");
  screens.arrive("main", "help");
  screens.setHelpReturn("main");
  screens.arrive("help", "main");
  assert.equal(screens.helpReturn(), "main", "? opened Help from the main screen");
  screens.arrive("settings", "help");
  assert.equal(screens.helpReturn(), "settings", "Help left by any way forgets where ? opened it");
  screens.setView("discord");
  assert.equal(screens.view(), "discord");
});

test("`#820 voice-page-action-menu`: the action menu's open state on its opener and the menu together, and its line where no item has work", () => {
  const { createActionMenu, ACTION_MENU_ITEMS } = VIEW_SHELL;
  const nodes = {};
  const node = (hidden) => ({ hidden, attributes: {}, setAttribute(name, value) { this.attributes[name] = value; } });
  for (const id of ACTION_MENU_ITEMS) nodes[id] = node(true);
  nodes["action-menu"] = node(true);
  nodes["action-menu-open"] = node(false);
  nodes["action-menu-empty"] = node(true);
  let changes = 0;
  const menu = createActionMenu({ byId: (id) => nodes[id], changed: () => { changes += 1; } });
  menu.setOpen(true);
  assert.deepStrictEqual([menu.open(), nodes["action-menu"].hidden, nodes["action-menu-open"].attributes["aria-expanded"], changes],
    [true, false, "true", 1]);
  assert.equal(nodes["action-menu-empty"].hidden, false, "no item has work: the line says so");
  nodes["expand-all"].hidden = false;
  menu.render();
  assert.equal(nodes["action-menu-empty"].hidden, true, "an item with work hides the line");
  menu.setOpen(false);
  assert.deepStrictEqual([menu.open(), nodes["action-menu"].hidden, nodes["action-menu-open"].attributes["aria-expanded"], changes],
    [false, true, "false", 2]);
});

test("`#624 voice-page-modularity`: the channel picker opened by Ctrl+K is remembered until a choice or Esc forgets it", () => {
  const { createChannelPick } = VIEW_SHELL;
  const pick = createChannelPick();
  assert.equal(pick.byKey(), false);
  pick.openedByKey();
  assert.equal(pick.byKey(), true);
  pick.forget();
  assert.equal(pick.byKey(), false);
});

test("`#624 voice-page-modularity`: the copy noted for the `copy` event is forgotten only by the wait it was noted for", () => {
  const { createPendingCopy } = VIEW_DOM;
  const pending = createPendingCopy();
  const [first, second] = [{ text: "first" }, { text: "second" }];
  assert.equal(pending.held(), null);
  pending.hold(first);
  assert.equal(pending.held(), first);
  pending.hold(second);
  pending.release(first);
  assert.equal(pending.held(), second, "the end of an older wait forgot the copy noted since");
  pending.release(second);
  assert.equal(pending.held(), null);
});

test("`#624 voice-page-modularity`: work asked for many times before a frame is done once in it, and at once with no frames", () => {
  const { createOncePerFrame } = VIEW_DOM;
  let runs = 0;
  const frames = [];
  let scheduler = (fn) => frames.push(fn);
  const work = createOncePerFrame(() => { runs += 1; }, () => scheduler);
  work.ask();
  work.ask();
  work.ask();
  assert.deepStrictEqual([runs, frames.length, work.due()], [0, 1, true], "three asks before a frame asked for more than one");
  frames.shift()();
  assert.deepStrictEqual([runs, work.due()], [1, false]);
  work.ask();
  assert.equal(frames.length, 1, "an ask after the frame did not ask for the next");
  // A page with no frames does the work at once, every time.
  scheduler = null;
  frames.shift()();
  work.ask();
  work.ask();
  assert.equal(runs, 4);
});

test("`#624 voice-page-modularity`: a row button says the keys the hints last said for its class, or none", () => {
  const { createRowShortcuts } = VIEW_SHELL;
  const button = () => {
    const attributes = new Map();
    return { attributes, setAttribute: (name, value) => attributes.set(name, value), removeAttribute: (name) => attributes.delete(name) };
  };
  const shortcuts = createRowShortcuts();
  assert.deepStrictEqual([...shortcuts.classes()], []);
  const done = button();
  done.setAttribute("aria-keyshortcuts", "stale");
  shortcuts.say(done, "done-button");
  assert.equal(done.attributes.has("aria-keyshortcuts"), false, "a button no key presses kept its shortcuts");
  shortcuts.set(new Map([["done-button", ["e", "Shift+E"]], ["reply-button", ["r"]]]));
  assert.deepStrictEqual([...shortcuts.classes()], ["done-button", "reply-button"]);
  shortcuts.say(done, "done-button");
  assert.equal(done.attributes.get("aria-keyshortcuts"), "e Shift+E");
});
