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

test("`#624 voice-page-modularity`: whether nothing holds the focus, read from the document alone", () => {
  const { focusLost } = VIEW_DOM;
  const was = globalThis.document;
  const body = { name: "body" };
  const root = { name: "root" };
  const held = { name: "held" };
  try {
    for (const [active, expected] of [[null, true], [undefined, true], [body, true], [root, true], [held, false]]) {
      globalThis.document = { activeElement: active, body, documentElement: root };
      assert.equal(focusLost(), expected, `activeElement ${active && active.name} misread`);
    }
  } finally {
    globalThis.document = was;
  }
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
  // `#892 voice-page-optimistic-done`: whether it still says what was said, as it was said.
  assert.equal(notices.statusSays("Saved with secret-token-1."), true, "the line does not say what it was told to");
  assert.equal(notices.statusSays("Saved."), false);
  assert.equal(timers[0].ms, STATUS_DISMISS_MS);
  timers[0].run();
  assert.equal(el("status-line").hidden, true);
  assert.equal(el("status").textContent, "Saved with [redacted].", "a hide, not an erase");
  assert.equal(notices.statusSays("Saved with secret-token-1."), false, "a line taken away still says it");
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

test("`#880 voice-page-menu-placement`: the reader's row, hidden or shown, on the root and in the item's word, kept and held", () => {
  const { createReaderRow, READER_WORDS } = VIEW_SHELL;
  const attributes = new Map();
  const root = { setAttribute: (name, value) => attributes.set(name, value), removeAttribute: (name) => attributes.delete(name) };
  const label = { textContent: "" };
  const kept = [];
  let stored = true;
  let reads = 0;
  const row = createReaderRow({
    byId: (id) => { assert.equal(id, "reader-toggle-label"); return label; },
    root: () => root,
    stored: () => { reads += 1; return stored; },
    keep: (hidden) => kept.push(hidden),
  });
  assert.equal(reads, 0, "made, it read the storage before its first use");
  row.render();
  assert.deepStrictEqual([attributes.get("data-reader"), label.textContent, row.hidden()], ["hidden", READER_WORDS.hidden, true]);
  assert.equal(READER_WORDS.hidden, "Show reader", "the word is not what a press does");
  row.setHidden(false);
  assert.deepStrictEqual([attributes.has("data-reader"), label.textContent, row.hidden(), kept], [false, "Hide reader", false, [false]]);
  // A browser that will not keep it: the page still holds the choice it was given.
  stored = true;
  assert.equal(row.hidden(), false, "the choice made here was lost to the storage's answer");
  assert.equal(reads, 1, "the storage was read again after the first use");
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

test("`#624 voice-page-modularity`: the Undo offer for the last Done stands until replaced or cleared, the same object", () => {
  const { createUndoOffer } = VIEW_SHELL;
  const offer = createUndoOffer();
  assert.equal(offer.current(), null);
  const done = { channel: "c", messages: ["1", "2"], at: 3 };
  offer.set(done);
  assert.equal(offer.current(), done, "the offer is not the object set: an undo compares it by identity");
  offer.set(null);
  assert.equal(offer.current(), null);
});

test("chords: Ctrl and Cmd alike, letters by place only off ASCII, digits and brackets by place, ? under Cmd", () => {
  const { chordMatches, typedKey, typesCharacter, swallow, MAC_FIELD_EDIT_LETTERS } = VIEW_SHELL;
  const key = (fields) => ({ key: "", code: "", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...fields });
  assert.equal(chordMatches({ key: "k", mod: true }, key({ key: "k", ctrlKey: true })), true);
  assert.equal(chordMatches({ key: "k", mod: true }, key({ key: "k", metaKey: true })), true, "Cmd is Ctrl");
  assert.equal(chordMatches({ key: "k" }, key({ key: "k", ctrlKey: true })), false, "a held Ctrl is another chord");
  assert.equal(chordMatches({ key: "e" }, key({ key: "у", code: "KeyE" })), true, "a layout for another script is read by place");
  assert.equal(chordMatches({ key: "z" }, key({ key: ";", code: "KeyZ" })), false, "ASCII punctuation is that character, not z");
  assert.equal(chordMatches({ key: "e", shift: true }, key({ key: "E", shiftKey: true })), true);
  assert.equal(chordMatches({ key: "1", mod: true }, key({ key: "&", code: "Digit1", ctrlKey: true })), true, "AZERTY's Ctrl+1");
  assert.equal(chordMatches({ key: "2", mod: true }, key({ key: "@", code: "Digit2", ctrlKey: true, shiftKey: true })), false);
  assert.equal(chordMatches({ key: "0", mod: true }, key({ key: "0", code: "Digit0", ctrlKey: true, shiftKey: true })), false, "a shifted 0 is never Ctrl+0");
  assert.equal(chordMatches({ key: ")", mod: true }, key({ key: "0", code: "Digit0", ctrlKey: true, shiftKey: true })), true, "the bracket chord by place");
  assert.equal(chordMatches({ key: "(", mod: true }, key({ key: ")", code: "Digit9", ctrlKey: true, shiftKey: true })), false, "a typed bracket is that bracket");
  assert.equal(typedKey(key({ key: "/", code: "Slash", metaKey: true, shiftKey: true })), "?");
  assert.equal(typedKey(key({ key: "/", code: "Slash", ctrlKey: true, shiftKey: true })), "/", "only under Cmd");
  assert.equal(chordMatches({ key: "Escape" }, key({ key: "Escape" })), true);
  assert.deepStrictEqual([typesCharacter({ key: "j" }), typesCharacter({ key: " " }), typesCharacter({ key: "j", mod: true }), typesCharacter({ key: "Escape" })], [true, false, false, false]);
  let prevented = 0;
  swallow({ preventDefault: () => prevented++ });
  swallow(null);
  assert.equal(prevented, 1);
  assert.equal(MAC_FIELD_EDIT_LETTERS.includes("k"), true);
  assert.equal(MAC_FIELD_EDIT_LETTERS.includes("s"), false, "Ctrl+S is the page's on a Mac too");
});

test("`#846 draft-chip-honest`, `#624 voice-page-modularity`: the reply's last sight is kept by each drawing of the chip, and a frame redraws only when the reply crosses its list's edge", () => {
  const { createReplySight } = VIEW_SHELL;
  const sight = createReplySight();
  assert.equal(sight.crossed(false), false, "out of sight at first: nothing to redraw");
  assert.equal(sight.crossed(true), true, "it came into sight");
  assert.equal(sight.crossed(true), false, "still in sight is no crossing");
  sight.keep(false);
  assert.equal(sight.crossed(false), false, "a drawing kept it out of sight");
  assert.equal(sight.crossed(true), true);
});

test("`#624 voice-page-modularity`: the prompts tray is shut until opened, and says so on its opener and the tray together", () => {
  const { createPromptsTray } = VIEW_SHELL;
  const node = (hidden) => ({ hidden, attributes: {}, setAttribute(name, value) { this.attributes[name] = value; } });
  const nodes = { "prompts-tray": node(true), "prompts-open": node(false) };
  let changes = 0;
  const tray = createPromptsTray({ byId: (id) => nodes[id], changed: () => { changes += 1; } });
  assert.deepStrictEqual([tray.open(), changes], [false, 0], "a new tray was open, or said something before it was asked");
  tray.setOpen(true);
  assert.deepStrictEqual([tray.open(), nodes["prompts-tray"].hidden, nodes["prompts-open"].attributes["aria-expanded"], changes],
    [true, false, "true", 1]);
  tray.setOpen("yes");
  assert.equal(tray.open(), false, "something other than true opened the tray");
  assert.deepStrictEqual([nodes["prompts-tray"].hidden, nodes["prompts-open"].attributes["aria-expanded"], changes], [true, "false", 2],
    "the tray and its opener came apart, or the back gesture was not told");
  const other = createPromptsTray({ byId: (id) => nodes[id], changed: () => {} });
  tray.setOpen(true);
  assert.equal(other.open(), false, "two trays share their state");
});

test("`#624 voice-page-modularity`: the keyboard dispatch runs the first binding that takes a key, in its column, by the rules of the focus", () => {
  const { createKeyDispatch } = VIEW_SHELL;
  const log = [];
  let single = true;
  const field = { field: true };
  const side = { pane: "side" };
  const inSide = { in: side };
  const slider = { own: true };
  let mod = "Cmd";
  let main = true;
  const bind = (name, chords, extra = {}) => ({ chords, run: () => { log.push(name); return extra.takes ?? true; }, ...extra });
  const keymap = [
    { chords: [{ key: "?" }] },
    bind("passes", [{ key: "j" }], { takes: false }),
    bind("down", [{ key: "j" }], { group: "moving", repeats: true }),
    bind("done", [{ key: "e" }], { group: "message" }),
    bind("filter", [{ key: "f" }], { group: "filters" }),
    bind("escape", [{ key: "Escape" }]),
    { chords: [{ key: "s", mod: true }], everywhere: true, run: (event, target) => { log.push(target === field ? "send from the focus" : "send"); return true; } },
    bind("bare", [{ key: "b" }, { key: "b", mod: true }], { everywhere: true }),
    bind("in field", [{ key: "x" }], { inField: true }),
  ];
  const keys = createKeyDispatch({
    keymap: () => keymap,
    singleKeysOn: () => single,
    isTextField: (target) => Boolean(target && target.field),
    keepsOwnKeys: (target) => Boolean(target && target.own),
    keysInUse: () => log.push("keys"),
    modKeyName: () => mod,
    activeElement: () => field,
    onMain: () => main,
    paneFor: (target) => (target && target.in) || null,
    paneColumn: (pane) => pane.pane,
    mainColumn: () => "main",
    inColumn: (column, act) => { log.push(`in ${column}`); act(); },
    listGroups: ["moving", "message"],
    readerActed: () => log.push("acted"),
    routeScrollKey: (event) => log.push(`scroll ${event.key}`),
  });
  const press = (fields) => {
    const event = { key: "", code: "", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, target: {}, prevented: 0, ...fields };
    event.preventDefault = () => { event.prevented += 1; event.defaultPrevented = true; };
    log.length = 0;
    keys.onEverywhereKey(event);
    keys.onPageKey(event);
    return [...log, `prevented ${event.prevented}`];
  };
  assert.deepStrictEqual(press({ key: "j" }), ["keys", "in main", "passes", "acted", "down", "prevented 1"],
    "the walk stopped at a binding that did not take the key, or the reader's acting in the list was not noted before the key that moves");
  assert.deepStrictEqual(press({ key: "f", target: inSide }), ["keys", "in side", "filter", "prevented 1"], "the key ran outside its list's column, or a filter key counted as acting in the list");
  assert.deepStrictEqual(press({ key: "e", repeat: true }), ["keys", "in main", "scroll e", "prevented 0"], "a held e was a Done on every row");
  assert.deepStrictEqual(press({ key: "j", target: field }), ["keys", "in main", "scroll j", "prevented 0"], "a single key fired in a text field");
  assert.deepStrictEqual(press({ key: "Escape", target: field }), ["keys", "in main", "escape", "prevented 1"], "Escape did not leave a field");
  single = false;
  assert.deepStrictEqual(press({ key: "e" }), ["keys", "in main", "scroll e", "prevented 0"], "a single key fired with the single keys off");
  single = true;
  assert.deepStrictEqual(press({ key: "j", target: slider }), ["keys", "in main", "scroll j", "prevented 0"], "a single key fired on an element that keeps its own keys");
  assert.deepStrictEqual(press({ key: "x", target: field }), ["keys", "in main", "in field", "prevented 1"], "a binding marked inField did not fire in a field");
  main = false;
  assert.deepStrictEqual(press({ key: "f", target: inSide }), ["keys", "in main", "filter", "prevented 1"], "off the main screen a key went to a list's column");
  main = true;
  assert.deepStrictEqual(press({ key: "k", ctrlKey: true, target: field }), ["keys", "prevented 0"], "a Mac field's own Ctrl edit was taken");
  assert.equal(keys.macFieldEditKey({ key: "л", code: "KeyK", ctrlKey: true }, field), true, "a Mac field's Ctrl edit was not read by place on a layout for another script");
  assert.deepStrictEqual([keys.macFieldEditKey({ key: "k", ctrlKey: true }, {}), keys.macFieldEditKey({ key: "k", ctrlKey: true, metaKey: true }, field),
    keys.macFieldEditKey({ key: "s", ctrlKey: true }, field)], [false, false, false], "off a field, with Cmd held too, or for Ctrl+S, the key was left to the field");
  mod = "Ctrl";
  assert.deepStrictEqual(press({ key: "k", ctrlKey: true, target: field }), ["keys", "in main", "scroll k", "prevented 0"], "Ctrl in a field was left to it off an Apple keyboard");
  mod = "Cmd";
  assert.deepStrictEqual(press({ key: "s", metaKey: true, target: null }), ["keys", "in main", "send from the focus", "keys", "prevented 1"],
    "Cmd+S was not heard first at the window, ran without the focus as its target, or the document's listener ran it again");
  assert.deepStrictEqual(press({ key: "s", ctrlKey: true, target: field }), ["keys", "in main", "send from the focus", "keys", "prevented 1"], "Ctrl+S in a field on a Mac was left to the field");
  assert.deepStrictEqual(press({ key: "b" }), ["keys", "in main", "bare", "prevented 1"], "the bare chord of an everywhere binding was taken at the window");
  assert.deepStrictEqual(press({ key: "s", ctrlKey: true, repeat: true }), ["keys", "prevented 1"], "a held chord's repeats were not refused, or ran it");
  assert.deepStrictEqual(press({ key: "j", isComposing: true }), ["prevented 0"], "a key fired while an input method composed");
  assert.deepStrictEqual(press({ key: "/", keyCode: 229 }), ["prevented 0"], "a key fired while an input method held it (keyCode 229)");
  assert.equal(keys.chordAllowed(keymap[3], { key: "e" }, {}), true);
});

test("`#624 voice-page-modularity`: the keys are said from the keymap in the reader's own words, and what the switch turned off is left out", () => {
  const { createKeyHints, createRowShortcuts, chordWords, chordShortcuts, viewKeysNote } = VIEW_SHELL;
  assert.deepStrictEqual([chordWords({ key: "s", mod: true }, "Cmd"), chordWords({ key: "ArrowDown", alt: true }, "Cmd"),
    chordWords({ key: "ArrowDown", alt: true }, "Ctrl"), chordWords({ key: "e", shift: true }, "Ctrl"), chordWords({ key: "j" }, "Ctrl")],
  ["Cmd+S", "Option+↓", "Alt+↓", "Shift+E", "j"]);
  assert.deepStrictEqual([chordShortcuts({ key: "s", mod: true }), chordShortcuts({ key: " " }), chordShortcuts({ key: "e", shift: true })],
    [["Control+S", "Meta+S"], ["Space"], ["Shift+E"]]);
  assert.match(viewKeysNote("Cmd"), /^Cmd\+0 goes to the voice view, and Cmd\+1 to Cmd\+4/);
  const node = (tag = "div") => ({
    tag, attributes: {}, children: [], textContent: "", className: "",
    setAttribute(name, value) { this.attributes[name] = value; },
    removeAttribute(name) { delete this.attributes[name]; },
    append(...more) { this.children.push(...more); },
    replaceChildren(...more) { this.children = more; },
  });
  const done = node("button");
  const row = { done };
  const nodes = {};
  const byId = (id) => (nodes[id] ??= node());
  byId("discord-log").children = [row];
  const keymap = [
    { group: "Page", chords: [{ key: "/" }, { key: "/", mod: true }], does: "Search", controls: ["search-toggle"] },
    { group: "Page", chords: [{ key: "," }], does: () => "Settings, now", controls: ["open-settings"] },
    { group: "Message", chords: [{ key: "e" }], does: "Done", rowControls: ["done"] },
    { group: "Views", chords: [{ key: "1", mod: true }, { key: "2", mod: true }, { key: "3", mod: true }], does: "A view" },
  ];
  let single = true;
  const rowShortcuts = createRowShortcuts();
  const hints = createKeyHints({
    keymap: () => keymap, singleKeysOn: () => single, modKeyName: () => "Ctrl", byId, rowShortcuts,
    childByClass: (at, className) => at[className] || null, groups: ["Page", "Message", "Views"], viewGroup: "Views",
  });
  const was = globalThis.document;
  globalThis.document = { createElement: node };
  try {
    hints.render();
    assert.deepStrictEqual([nodes["single-keys"].checked, nodes["search-toggle"].attributes["aria-keyshortcuts"],
      nodes["open-settings"].attributes["aria-keyshortcuts"], done.attributes["aria-keyshortcuts"], nodes["search-toggle"].attributes.title],
    [true, "/ Control+/ Meta+/", ",", "E", "Search messages (/ or Ctrl+/)"]);
    const list = nodes["keyboard-keys"].children;
    assert.deepStrictEqual(list.map((one) => one.tag), ["h3", "ul", "h3", "ul", "h3", "ul", "p"], "the groups, or the note under the views, are not where they were");
    assert.deepStrictEqual(list[1].children.map((one) => one.children.map((part) => part.textContent)),
      [["/ or Ctrl+/", "Search"], [",", "Settings, now"]]);
    assert.equal(list[5].children[0].children[0].textContent, "Ctrl+1, Ctrl+2, Ctrl+3");
    single = false;
    hints.render();
    assert.deepStrictEqual([nodes["single-keys"].checked, nodes["open-settings"].attributes["aria-keyshortcuts"], done.attributes["aria-keyshortcuts"],
      nodes["search-toggle"].attributes.title, nodes["open-settings"].attributes.title],
    [false, undefined, undefined, "Search messages (Ctrl+/)", "Settings ()"], "a key the switch turned off was still said on its button");
    assert.match(nodes["keyboard-keys"].children[0].textContent, /^Single-key shortcuts are off in Settings: .* the Ctrl and Alt chords act\.$/);
    assert.equal(hints.bindingWords(keymap[0]), "/ or Ctrl+/", "the full list leaves out keys the switch turned off");
  } finally {
    globalThis.document = was;
  }
});

test("`#296 copy-highlighted-message`, `#624 voice-page-modularity`: the highlighted message is copied in both flavours, never over the browser's own copy", async () => {
  const { createHighlightCopy, htmlText, plainParagraphsHtml, COPIED_MESSAGE, NOTHING_TO_COPY, COPIED_SELECTION } = VIEW_SHELL;
  assert.equal(htmlText(`a<b> & "c" 'd'`), "a&#60;b&#62; &#38; &#34;c&#34; &#39;d&#39;");
  assert.equal(plainParagraphsHtml("one\ntwo\n \nthree"), "<p>one<br>two</p><p>three</p>");
  const list = {};
  const row = (attributes = {}, extra = {}) => ({ parentNode: list, getAttribute: (name) => attributes[name] ?? null,
    hasAttribute: (name) => name in attributes, ...extra });
  const messages = new Map();
  let pane = null;
  let selection = "";
  let field = false;
  const said = [];
  const timers = [];
  const written = [];
  let writes = true;
  const copies = createHighlightCopy({
    isTextField: () => field,
    selectedText: () => selection,
    activePane: () => pane,
    rowShown: () => true,
    rowMessages: (lit) => messages.get(lit) || [],
    combinedContent: (all) => all.map((one) => one.content).join("\n\n"),
    renderedHtmlOf: (one) => one.html || "",
    rawTextOf: (one) => one.content,
    setStatus: (text) => said.push(text),
    copyRefused: "refused",
    writeClipboardText: async (text) => { written.push(text); return writes; },
    guardQuietly: (run) => run,
    setTimeout: (run) => timers.push(run),
  });
  const was = globalThis.document;
  let execs = 0;
  globalThis.document = { activeElement: null, execCommand: () => { execs += 1; return false; } };
  const copyEvent = () => {
    const data = {};
    const event = { clipboardData: { setData: (type, value) => { data[type] = value; } }, prevented: false, preventDefault() { this.prevented = true; } };
    return [event, data];
  };
  try {
    const lit = row();
    messages.set(lit, [{ content: "first\nline", html: "" }, { content: "second", html: "<b>second</b>" }]);
    pane = { lit, list: () => list };
    let [event, data] = copyEvent();
    copies.onPageCopy(event);
    assert.deepStrictEqual([data, event.prevented, said], [{ "text/plain": "first\nline\n\nsecond", "text/html": "<div><p>first<br>line</p></div><div><b>second</b></div>" }, true, [COPIED_MESSAGE]]);
    const before = { prevented: false, preventDefault() { this.prevented = true; } };
    copies.onPageBeforeCopy(before);
    assert.equal(before.prevented, true, "beforecopy was left alone with a message to copy");
    selection = "words";
    [event, data] = copyEvent();
    copies.onPageCopy(event);
    assert.deepStrictEqual([data, event.prevented], [{}, false], "the message was copied over the words the reader selected");
    said.length = 0;
    assert.equal(copies.copyFromKeyboard(), true);
    await Promise.resolve();
    assert.deepStrictEqual([execs, written, said], [1, ["words"], [COPIED_SELECTION]], "c with words selected did not copy the words");
    selection = "";
    field = true;
    assert.equal(copies.copyChordFromKeyboard(), false);
    assert.equal(timers.length, 0, "Ctrl+C in a text field was taken from the browser");
    field = false;
    assert.equal(copies.copyChordFromKeyboard(), false, "the chord's default was not left to the browser");
    [event, data] = copyEvent();
    copies.onPageCopy(event);
    assert.equal(data["text/plain"], "first\nline\n\nsecond");
    written.length = 0;
    timers.shift()();
    assert.deepStrictEqual(written, [], "a copy the copy event served was copied again a task later");
    copies.copyChordFromKeyboard();
    said.length = 0;
    writes = false;
    timers.shift()();
    await Promise.resolve();
    await Promise.resolve();
    assert.deepStrictEqual([written, said], [["first\nline\n\nsecond"], ["refused"]], "a chord no copy event answered was not copied a task later, or a refusal was not said");
    pane = { lit: row({ "data-read-run": "head" }), list: () => list };
    said.length = 0;
    copies.copyFromKeyboard();
    assert.deepStrictEqual(said, [NOTHING_TO_COPY], "the line of a collapsed run was copied");
    pane = { lit: row({ "data-card": "" }, { cardRoot: { content: "the thread's first", html: "" } }), list: () => list };
    [event, data] = copyEvent();
    copies.onPageCopy(event);
    assert.equal(data["text/plain"], "the thread's first", "a thread card did not copy its first message");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: a scroll key no binding took goes to the list on screen only from where it cannot be used", () => {
  const { createScrollKeys, pageStep, SCROLL_KEYS } = VIEW_SHELL;
  assert.deepStrictEqual([pageStep({ clientHeight: 800 }), pageStep({}), SCROLL_KEYS.includes(" ")], [700, 1, true]);
  const log = [];
  const area = { clientHeight: 400, scrollTop: 1000, children: [], focus: (options) => log.push(`focus ${options.preventScroll}`) };
  const inList = { parentNode: area };
  const otherInList = { parentNode: area };
  const rows = [{ id: "first" }, { id: "last" }];
  const pane = { row: inList, scroller: () => area };
  const field = { field: true };
  const tall = { field: true, tall: true };
  const picker = { own: true, tagName: "select" };
  const slider = { own: true, tagName: "INPUT" };
  const button = { space: true };
  let side = false;
  let main = true;
  const keys = createScrollKeys({
    scrollArea: () => (side ? "side" : area),
    sideScroll: () => "side",
    toFoot: () => log.push("foot"),
    endFilterWalk: () => log.push("walk ended"),
    closeOverscroll: () => log.push("room closed"),
    openOverscroll: () => log.push("room opened"),
    scrollToNewest: () => log.push("newest"),
    visibleScroller: () => area,
    isTextField: (node) => Boolean(node && node.field),
    scrollsItself: (node) => Boolean(node && node.tall),
    keepsOwnKeys: (node) => Boolean(node && node.own),
    pressedBySpace: (node) => Boolean(node && node.space),
    onMain: () => main,
    paneFor: (node) => (node === inList || node === otherInList ? pane : null),
    paneRows: () => rows,
    selectRow: (_pane, row, bring) => log.push(`select ${row.id} ${bring}`),
    followPageScroll: (_pane, _row, up) => log.push(`follow ${up}`),
  });
  const key = (name, fields = {}) => {
    const event = { key: name, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, repeat: false, prevented: false, ...fields };
    event.preventDefault = () => { event.prevented = true; };
    return event;
  };
  const route = (event, target) => {
    log.length = 0;
    area.scrollTop = 1000;
    keys.routeScrollKey(event, target);
    return [...log, area.scrollTop, event.prevented];
  };
  assert.deepStrictEqual(route(key("PageDown"), field), ["room opened", 1350, true], "PgDn in a short field did not page the list");
  assert.deepStrictEqual(route(key("PageUp"), tall), [1000, false], "a field that scrolls itself lost its page keys");
  assert.deepStrictEqual(route(key("ArrowDown"), field), [1000, false], "an arrow was taken from a field");
  assert.deepStrictEqual(route(key("PageDown", { repeat: true }), field), [1350, true], "a held PgDn opened the room past the foot");
  assert.deepStrictEqual(route(key("Home"), picker), [0, true], "Home on a picker did not go to the list's top");
  assert.deepStrictEqual(route(key("ArrowDown"), picker), [1000, false], "an arrow was taken from a picker");
  assert.deepStrictEqual(route(key("PageDown"), slider), [1000, false], "a slider lost its keys");
  assert.deepStrictEqual(route(key(" ", { shiftKey: true }), inList), ["follow true", 1000, false], "Shift+Space from the selected message did not follow up");
  assert.deepStrictEqual(route(key(" "), button), [1000, false], "Space was taken from a button");
  assert.deepStrictEqual(route(key("PageDown"), { elsewhere: true }), ["room opened", "focus true", 1000, false], "the list did not take the focus from elsewhere");
  assert.deepStrictEqual(route(key("PageDown", { ctrlKey: true }), field), [1000, false], "a Ctrl chord was routed");
  assert.deepStrictEqual(route(key("Enter"), field), [1000, false], "a key a browser does not scroll with was routed");
  assert.deepStrictEqual(route(key("PageUp", { metaKey: true }), field), [1000, false], "Cmd+PgUp in a field paged the list");
  assert.deepStrictEqual(route(key("End", { metaKey: true }), picker), [1000, false], "Cmd+End on a picker went to the list's end");
  assert.deepStrictEqual(route(key("End"), picker), ["walk ended", "newest", 1000, true], "End on a picker did not take the list to its newest line");
  assert.deepStrictEqual(route(key(" "), inList), ["room opened", "follow false", 1000, false], "Space from the selected message did not open the room and follow down");
  assert.deepStrictEqual(route(key("PageDown"), otherInList), ["room opened", 1000, false], "a page key from another row of the list was followed as if from the selected one");
  main = false;
  assert.deepStrictEqual(route(key("PageDown"), inList), ["room opened", 1000, false], "off the main screen a page key looked for a list's pane");
  main = true;
  log.length = 0;
  assert.equal(keys.jumpList(field, true), true);
  assert.deepStrictEqual(log, ["walk ended", "newest"], "Ctrl+End on the main list was not the Newest chip");
  side = true;
  log.length = 0;
  keys.jumpToNewest();
  assert.deepStrictEqual(log, ["walk ended", "room closed", "newest"], "the Newest chip on the right left the room past the foot open");
  log.length = 0;
  assert.equal(keys.listEdge(inList, true), true);
  assert.deepStrictEqual(log, ["foot", "select last false"], "End over a list that is not the main one did not go to its foot and select its newest");
  area.scrollTop = 500;
  log.length = 0;
  assert.equal(keys.listEdge(inList, false), true);
  assert.deepStrictEqual([log, area.scrollTop], [["select first false"], 0]);
  assert.equal(keys.listEdge({ nowhere: true }, true), false, "Home and End outside a list were taken");
});

test("`#624 voice-page-modularity`: the search, Settings, Help, Back and Escape keys act by the screen that is up, and Escape takes the first that applies", () => {
  const { createScreenKeys } = VIEW_SHELL;
  const log = [];
  let screen = "main";
  let helpReturn = "main";
  const state = { search: false, docked: false, keysHere: true, threadList: false, channel: true, view: "all", cards: false,
    menus: [], rowMenu: false, litCard: false, allFocus: false, composer: null, place: { pane: null, row: null } };
  const field = { focus: () => log.push("field focus"), select: () => log.push("field select") };
  const keys = createScreenKeys({
    screens: { screen: () => screen, helpReturn: () => helpReturn, setHelpReturn: (to) => { helpReturn = to; } },
    showScreen: (name) => { log.push(`show ${name}`); screen = name; },
    closeSettings: () => { log.push("close settings"); screen = "main"; },
    showHelp: (entry) => { log.push(`help ${entry}`); screen = "help"; },
    byId: () => field,
    searchOpen: () => state.search,
    setSearchOpen: (open) => { log.push(`search ${open}`); state.search = open; },
    openMenus: () => state.menus,
    closeOpenMenu: (name) => { const open = state.menus.includes(name); if (open) log.push(`closed ${name}`); state.menus = state.menus.filter((one) => one !== name); return open; },
    closeRowMenuOnEscape: () => state.rowMenu,
    replyDocked: () => state.docked,
    replyKeysHere: () => state.keysHere,
    closeReply: () => log.push("reply closed"),
    threadListOnScreen: () => state.threadList,
    threadListBack: () => log.push("thread list back"),
    channelOnScreen: () => state.channel,
    selectionView: () => state.view,
    sideShowsCards: () => state.cards,
    leaveThread: () => log.push("thread left"),
    keyContext: () => state.place,
    selectRow: (_pane, row, bring) => log.push(`select ${row.id} ${bring}`),
    settleFocus: () => log.push("focus settled"),
    isComposer: (node) => node === state.composer,
    focusList: (pane) => log.push(`focus list ${pane}`),
    paneFor: () => "pane",
    forgetChannelPick: () => log.push("pick forgotten"),
    putOutLitCard: () => state.litCard,
    clearAllThreadFocus: () => state.allFocus,
    clearLinkedMark: () => state.linked,
  });
  const run = (act) => { log.length = 0; const did = act(); return [did, ...log]; };
  state.menus = ["menu:actions"];
  assert.deepStrictEqual(run(() => keys.searchFromAnywhere()), [true, "closed menu:actions", "search true", "field focus", "field select"],
    "Ctrl+/ on the main screen did not close the menu and then open the search");
  state.search = false;
  assert.deepStrictEqual(run(() => keys.searchFromKeyboard()), [true, "search true", "field focus", "field select"]);
  assert.deepStrictEqual(run(() => keys.searchFromKeyboard()), [true, "field focus", "field select"], "a search already open was opened again");
  state.search = false;
  screen = "settings";
  assert.deepStrictEqual(run(() => keys.searchFromKeyboard()), [true, "close settings", "search true", "field focus", "field select"], "/ in Settings did not leave it for the search");
  state.search = false;
  screen = "call";
  assert.deepStrictEqual(run(() => keys.searchFromKeyboard()), [false], "/ searched off the main screen");
  state.menus = ["menu:pace", "menu:row"];
  assert.deepStrictEqual(run(() => keys.searchFromAnywhere()), [true, "closed menu:pace", "closed menu:row"], "Ctrl+/ left a menu open, or was not taken off the main screen");
  screen = "signin";
  assert.deepStrictEqual(run(() => keys.settingsFromKeyboard()), [true, "show settings"]);
  assert.deepStrictEqual(run(() => keys.settingsFromKeyboard()), [true, "close settings"], ", in Settings did not leave it");
  screen = "help";
  assert.deepStrictEqual(run(() => keys.settingsFromKeyboard()), [true, "close settings"], ", in Help did not leave it");
  helpReturn = "signin";
  assert.deepStrictEqual([run(() => keys.keysFromKeyboard()), helpReturn], [[true, "help keyboard"], "main"], "? on the main screen did not keep it to return to");
  screen = "call";
  assert.deepStrictEqual(run(() => keys.settingsFromKeyboard()), [false]);
  assert.deepStrictEqual(run(() => keys.keysFromKeyboard()), [false], "? opened Help from a screen the gear does not reach");
  screen = "settings";
  assert.deepStrictEqual(run(() => keys.keysFromKeyboard()), [true, "help keyboard"]);
  assert.deepStrictEqual([helpReturn, run(() => keys.keysFromKeyboard()), helpReturn], ["settings", [true, "help keyboard"], "settings"], "? in Help forgot where it was pressed");
  assert.deepStrictEqual(run(() => keys.goBack()), [true, "show settings"], "Back from Help did not return to where ? was pressed");
  assert.deepStrictEqual(run(() => keys.goBack()), [true, "close settings"]);
  state.docked = true;
  state.keysHere = false;
  state.view = "thread";
  assert.deepStrictEqual(run(() => keys.goBack()), [true, "thread left"], "Back in the other column put down the reply");
  assert.equal(keys.replyOnScreen(), true);
  state.keysHere = true;
  assert.deepStrictEqual(run(() => keys.goBack()), [true, "reply closed"]);
  state.docked = false;
  state.cards = true;
  assert.deepStrictEqual(run(() => keys.goBack()), [false], "Back over the right column's cards left a thread");
  state.cards = false;
  state.threadList = true;
  assert.deepStrictEqual(run(() => keys.goBack()), [true, "thread list back"]);
  state.threadList = false;
  state.view = "all";
  const target = { id: "target" };
  state.rowMenu = true;
  state.menus = ["menu:pace"];
  state.search = true;
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true], "a ⋯ menu was not closed first, before the popover and the search");
  state.rowMenu = false;
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true, "closed menu:pace"], "the popover was not closed before the search");
  state.search = false;
  state.menus = ["menu:actions"];
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true, "closed menu:actions"]);
  state.menus = ["menu:prompts", "menu:dismissed"];
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true, "closed menu:prompts"]);
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true, "closed menu:dismissed"]);
  const sheet = { className: "msg-details", parentNode: null };
  const row = { id: "row", className: "row", children: [sheet], removeChild: (child) => { log.push(`removed ${child.className}`); } };
  sheet.parentNode = row;
  state.place = { pane: "pane", row };
  const was = globalThis.document;
  globalThis.document = { activeElement: sheet };
  try {
    assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true, "removed msg-details", "select row false"], "the details sheet with the focus in it was not closed onto its message");
  } finally {
    globalThis.document = was;
  }
  state.place = { pane: null, row: null };
  state.search = true;
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true, "search false", "focus settled"]);
  const box = { value: "draft", nodeType: 1, hidden: false, parentNode: null };
  state.composer = box;
  assert.deepStrictEqual(run(() => keys.onEscape({}, box)), [true, "focus list pane"], "a box with words in it went Back, or kept the focus");
  box.value = "";
  state.view = "thread";
  assert.deepStrictEqual(run(() => keys.onEscape({}, box)), [true, "thread left"], "an empty box did not go Back at once");
  state.view = "all";
  assert.deepStrictEqual(run(() => keys.onEscape({}, box)), [true, "focus list pane"], "an empty box with no Back did not leave for the list");
  assert.deepStrictEqual(run(() => keys.onEscape({}, { id: "thread-select" })), [true, "pick forgotten", "focus list pane"], "Escape on the bar's picker did not leave it");
  assert.deepStrictEqual(run(() => keys.onEscape({}, { id: "discord-channel" })), [true, "pick forgotten", "focus list pane"], "Escape on the channel picker did not leave it");
  state.litCard = true;
  assert.deepStrictEqual(run(() => keys.onEscape({}, { id: "discord-channel" })), [true, "pick forgotten", "focus list pane"], "a lit card was put out before the picker was left");
  state.litCard = false;
  screen = "settings";
  assert.deepStrictEqual(run(() => keys.onEscape({}, { id: "discord-channel" })), [true, "close settings"], "off the main screen a picker named as the bar's was taken for it");
  screen = "main";
  state.litCard = true;
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true]);
  state.litCard = false;
  state.allFocus = true;
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true]);
  state.allFocus = false;
  // `#890 voice-page-linked-highlight`. The linked message's mark, after everything else and before Back.
  state.linked = true;
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [true], "Escape left the linked message's mark on screen");
  state.linked = false;
  assert.deepStrictEqual(run(() => keys.onEscape({}, target)), [false], "Escape with nothing to do did something");
});

test("`#624 voice-page-modularity`: m and Ctrl+S find the box to write in, stay in one on screen, and ↑ in the empty channel box selects the newest message", () => {
  const { createWriteKeys } = VIEW_SHELL;
  const log = [];
  const node = (id, extra = {}) => ({ id, nodeType: 1, hidden: false, parentNode: null, value: "",
    focus: (options) => log.push(`focus ${id}${options && options.preventScroll ? " quietly" : ""}`), scrollIntoView: () => {}, ...extra });
  const nodes = {};
  for (const id of ["reply-text", "compose-text", "channel-compose-text", "side-tools", "scroll-tools"]) nodes[id] = node(id);
  const sideBox = node("side-compose-text");
  const chip = node("chip", { parentNode: nodes["side-tools"] });
  const leftChip = node("left-chip", { parentNode: nodes["scroll-tools"] });
  let gone = false;
  const state = { screen: "main", view: "discord", reply: false, channel: true, threadList: false, mainRows: false, behind: false, side: false, rows: ["r1", "r2"] };
  const keys = createWriteKeys({
    byId: (id) => (state.side && id === "channel-compose-text" ? sideBox : nodes[id]),
    leftById: (id) => nodes[id],
    screen: () => state.screen,
    view: () => state.view,
    closeSettings: () => { log.push("close settings"); state.screen = "main"; },
    replyOnScreen: () => state.reply,
    channelOnScreen: () => state.channel,
    threadListOnScreen: () => state.threadList,
    threadListBack: () => { log.push("thread list back"); state.threadList = false; },
    mainRowsOnScreen: () => state.mainRows,
    behindThreadList: () => state.behind,
    inColumnGone: () => gone,
    isComposer: (held) => Boolean(held && /compose-text$|^reply-text$/.test(held.id)),
    inSide: () => state.side,
    activeColumn: () => (state.side ? "side" : "main"),
    mainColumn: () => "main",
    inColumn: (column, act) => { log.push(`in ${column}`); const was = state.side; state.side = column === "side"; try { return act(); } finally { state.side = was; } },
    channelPane: () => "channel pane",
    sidePane: () => "side pane",
    paneFor: (held) => (held === sideBox ? "side pane" : null),
    paneColumn: (pane) => (pane === "side pane" ? "side" : "main"),
    paneRows: (pane) => state.rows.map((id) => `${pane} ${id}`),
    selectRow: (pane, row, ...rest) => log.push(`select ${row}${rest.length ? " +" : ""}`),
  });
  const was = globalThis.document;
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  globalThis.document = { activeElement: null };
  try {
    assert.deepStrictEqual(run(() => keys.composeFromKeyboard()), [true, "focus channel-compose-text quietly"]);
    state.reply = true;
    assert.deepStrictEqual(run(() => keys.composeFromKeyboard()), [true, "focus reply-text"], "an open reply was not where writing went");
    state.reply = false;
    state.view = "voice";
    assert.deepStrictEqual(run(() => keys.composeFromKeyboard()), [true, "focus compose-text"], "the call's view did not write in the agent's box");
    nodes["compose-text"].hidden = true;
    assert.deepStrictEqual(run(() => keys.composeFromKeyboard()), [false]);
    state.view = "discord";
    state.behind = true;
    assert.deepStrictEqual(run(() => keys.composeFromKeyboard()), [false], "a box behind the thread list took the focus");
    state.behind = false;
    state.channel = false;
    assert.deepStrictEqual(run(() => keys.composeFromKeyboard()), [false], "m wrote in the channel's box with the channel not on screen");
    state.channel = true;
    state.side = true;
    sideBox.hidden = true;
    assert.deepStrictEqual(run(() => keys.composeFromKeyboard()), [true, "in main", "focus channel-compose-text quietly"], "a column with no box did not go to Main's");
    sideBox.hidden = false;
    state.side = false;
    state.screen = "help";
    state.threadList = true;
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true, "close settings", "thread list back", "in main", "focus channel-compose-text quietly"],
      "Ctrl+S from Help over the thread list did not leave both for the box");
    state.threadList = true;
    state.mainRows = true;
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true, "in main", "focus channel-compose-text quietly"], "the thread list with Main channel's box was left");
    state.mainRows = false;
    state.reply = true;
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true, "in main", "focus reply-text"], "the thread list with a reply open among its cards was left");
    state.reply = false;
    state.threadList = false;
    globalThis.document.activeElement = nodes["channel-compose-text"];
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true], "Ctrl+S in a box on screen moved the focus");
    gone = true;
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true, "in main", "focus channel-compose-text quietly"], "Ctrl+S stayed in a box whose column has gone");
    gone = false;
    globalThis.document.activeElement = leftChip;
    state.side = true;
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true, "in main", "focus channel-compose-text quietly"], "the left column's chips did not write in its box");
    state.side = false;
    globalThis.document.activeElement = chip;
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true, "in side", "focus side-compose-text quietly"], "the right column's chips did not write in its box");
    state.side = true;
    globalThis.document.activeElement = null;
    assert.deepStrictEqual(run(() => keys.writeFromKeyboard()), [true, "in side", "focus side-compose-text quietly"], "with the focus nowhere, the column the keys moved last was not chosen");
    state.side = false;
    assert.deepStrictEqual(run(() => keys.newestFromComposer(nodes["channel-compose-text"])), [true, "select channel pane r2"], "↑ did not select the newest message, or passed a scroll choice");
    nodes["channel-compose-text"].value = "words";
    assert.deepStrictEqual(run(() => keys.newestFromComposer(nodes["channel-compose-text"])), [false], "↑ in a box with words was taken");
    nodes["channel-compose-text"].value = "";
    state.rows = [];
    assert.deepStrictEqual(run(() => keys.newestFromComposer(nodes["channel-compose-text"])), [false]);
    assert.deepStrictEqual(run(() => keys.newestFromComposer(nodes["reply-text"])), [false], "↑ in another box was taken");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the view, filter and undo keys choose from the main screen, in the left column, and Undo takes the newest offer", () => {
  const { createViewKeys } = VIEW_SHELL;
  const log = [];
  const state = { screen: "main", before: "main", view: "discord", side: false, channel: "c1", onScreen: true, readHidden: false,
    threadList: false, search: false, pinned: false, links: false, done: null, send: null, sendHeld: true, bookmark: null, current: "c1" };
  const keys = createViewKeys({
    screens: { screen: () => state.screen, beforeSettings: () => state.before, view: () => state.view },
    inSide: () => state.side,
    mainColumn: () => "main",
    inColumn: (column, act) => { log.push(`in ${column}`); const was = state.side; state.side = column === "side"; try { return act(); } finally { state.side = was; } },
    closeSettings: () => { log.push("close settings"); state.screen = state.before; },
    pressViewSwitch: () => { log.push("switch"); state.view = state.view === "voice" ? "discord" : "voice"; },
    selectedChannelId: () => state.channel,
    channelOnScreen: () => state.onScreen && state.screen === "main" && state.view === "discord",
    chooseView: (which) => log.push(`view ${which}`),
    settleFocus: () => log.push("settled"),
    readModeHidden: () => state.readHidden,
    cycleReadMode: () => log.push("cycle read"),
    readModeSaid: () => "Fold read",
    setStatus: (text) => log.push(`say ${text}`),
    threadListOnScreen: () => state.threadList,
    searchOpen: () => state.search,
    setSearchOpen: (open) => { log.push(`search ${open}`); state.search = open; },
    pinnedOnly: () => state.pinned,
    setPinnedOnly: (...args) => { log.push(`pinned ${args.join(" ")}`); state.pinned = args[0]; },
    linksOnly: () => state.links,
    setLinksOnly: (on) => { log.push(`links ${on}`); state.links = on; },
    cycleAuthorFilter: () => log.push("author"),
    focusChannelList: () => log.push("focus list"),
    doneOffer: () => state.done,
    undoDone: () => log.push("undo done"),
    sendOffer: () => state.send,
    sendHeld: (id) => state.sendHeld && id === state.send.id,
    putBackSend: (id) => log.push(`put back ${id}`),
    bookmarkOffer: () => state.bookmark,
    currentChannelId: () => state.current,
    undoBookmarkRemoval: () => log.push("bookmark back"),
  });
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  assert.deepStrictEqual(run(() => keys.viewFromKeyboard("flat", false)), [true, "view flat", "settled"]);
  state.side = true;
  assert.deepStrictEqual(run(() => keys.viewFromKeyboard("main", false)), [true, "in main", "view main", "settled"], "a view was chosen in the right column");
  state.side = false;
  assert.deepStrictEqual(run(() => keys.viewFromKeyboard("main", true)), [true], "a held key's repeat chose a view");
  state.screen = "settings";
  assert.deepStrictEqual(run(() => keys.viewFromKeyboard("threads", false)), [true, "close settings", "view threads", "settled"], "Settings over the main screen was not left for the view");
  state.screen = "settings";
  state.before = "signin";
  assert.deepStrictEqual(run(() => keys.viewFromKeyboard("threads", false)), [false], "a view was chosen from Settings over the sign-in screen");
  state.screen = "main";
  state.before = "main";
  state.view = "voice";
  assert.deepStrictEqual(run(() => keys.viewFromKeyboard("columns", false)), [true, "switch", "view columns", "settled"], "the call's view did not give way to the channel");
  state.channel = "";
  assert.deepStrictEqual(run(() => keys.viewFromKeyboard("main", false)), [false], "a view was chosen with no channel");
  assert.deepStrictEqual(run(() => keys.voiceFromKeyboard(false)), [true, "switch"], "Ctrl+0 needed a channel, or did not go to the call's view");
  assert.deepStrictEqual(run(() => keys.voiceFromKeyboard(false)), [true], "Ctrl+0 on the call's view left it");
  state.view = "discord";
  assert.deepStrictEqual(run(() => keys.voiceFromKeyboard(true)), [true], "a held Ctrl+0's repeat switched the view");
  state.side = true;
  state.screen = "help";
  assert.deepStrictEqual(run(() => keys.voiceFromKeyboard(false)), [true, "in main", "close settings", "switch"], "Ctrl+0 from Help in the right column did not leave Help for the call's view, in the left column");
  state.side = false;
  state.screen = "signin";
  assert.deepStrictEqual(run(() => keys.voiceFromKeyboard(false)), [false]);
  state.screen = "main";
  state.channel = "c1";
  state.view = "discord";
  assert.deepStrictEqual(run(() => keys.readModeFromKeyboard()), [true, "cycle read", "say Fold read"]);
  state.readHidden = true;
  assert.deepStrictEqual(run(() => keys.readModeFromKeyboard()), [false], "h cycled a hidden read button");
  state.side = true;
  assert.deepStrictEqual(run(() => keys.filterFromKeyboard("pinned")), [true, "in main", "search true", "pinned true", "focus list"],
    "Shift+P did not filter the left column, open the bar, or pass one argument");
  state.side = false;
  assert.deepStrictEqual(run(() => keys.filterFromKeyboard("links")), [true, "links true", "focus list"]);
  assert.deepStrictEqual(run(() => keys.filterFromKeyboard("author")), [true, "author", "focus list"]);
  state.threadList = true;
  assert.deepStrictEqual(run(() => keys.filterFromKeyboard("pinned")), [false], "a filter key over the thread list filtered the column behind it");
  state.threadList = false;
  assert.deepStrictEqual(run(() => keys.undoFromKeyboard()), [true, "say Nothing to undo."]);
  state.done = { at: 2 };
  state.send = { id: "s1", at: 5 };
  state.bookmark = { channel: "c1", at: 3 };
  assert.deepStrictEqual(run(() => keys.undoFromKeyboard()), [true, "put back s1"], "the newest offer was not taken back first");
  state.send = { id: "s1", at: 3 };
  assert.deepStrictEqual(run(() => keys.undoFromKeyboard()), [true, "put back s1"], "of two offers made at once, the one listed first was not taken back first");
  state.send = null;
  assert.deepStrictEqual(run(() => keys.undoFromKeyboard()), [true, "bookmark back"]);
  state.bookmark = { channel: "c2", at: 9 };
  assert.deepStrictEqual(run(() => keys.undoFromKeyboard()), [true, "undo done"], "another channel's removed bookmark was put back from here");
  state.done = { };
  state.send = { id: "s2" };
  state.sendHeld = false;
  assert.deepStrictEqual(run(() => keys.undoFromKeyboard()), [true, "undo done"], "a dismissed send no longer held was offered");
  state.onScreen = false;
  assert.deepStrictEqual(run(() => keys.undoFromKeyboard()), [false]);
});

test("`#624 voice-page-modularity`: Alt+↓ and Alt+↑ step through the channels and stop at the ends, and only a channel chosen from the picker Ctrl+K opened hands the keys back", () => {
  const { createChannelKeys } = VIEW_SHELL;
  const log = [];
  const picker = { hidden: false, disabled: false, focus: () => log.push("picker focus"), showPicker: () => log.push("picker open") };
  const state = { onScreen: true, selected: "b" };
  const keys = createChannelKeys({
    channelOnScreen: () => state.onScreen,
    picker: () => picker,
    channelsInPickerOrder: () => [{ id: "a" }, { id: "b" }, { id: 3 }],
    selectedChannelId: () => state.selected,
    chooseChannel: (id) => { log.push(`choose ${JSON.stringify(id)}`); state.selected = id; },
    changeChannel: () => log.push("change"),
    settleFocus: () => log.push("settled"),
  });
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  assert.deepStrictEqual(run(() => keys.stepChannel({ key: "ArrowDown" })), [true, 'choose "3"', "change", "settled"], "Alt+↓ did not choose the next channel by its id as a string");
  assert.deepStrictEqual(run(() => keys.stepChannel({ key: "ArrowUp" })), [true, 'choose "b"', "change", "settled"],
    "the selected \"3\" was not found as the numeric id 3, so Alt+↑ did not step back from it");
  state.selected = "3";
  assert.deepStrictEqual(run(() => keys.stepChannel({ key: "ArrowDown" })), [true], "Alt+↓ on the last channel wrapped round");
  state.selected = "a";
  assert.deepStrictEqual(run(() => keys.stepChannel({ key: "ArrowUp" })), [true], "Alt+↑ on the first channel wrapped round");
  state.selected = "gone";
  assert.deepStrictEqual(run(() => keys.stepChannel({ key: "ArrowDown" })), [true], "a channel not in the picker stepped somewhere");
  picker.disabled = true;
  assert.deepStrictEqual([run(() => keys.stepChannel({ key: "ArrowDown" })), run(() => keys.pickChannel())], [[false], [false]], "a disabled picker was stepped or opened");
  picker.disabled = false;
  picker.hidden = true;
  assert.deepStrictEqual([run(() => keys.stepChannel({ key: "ArrowDown" })), run(() => keys.pickChannel())], [[false], [false]], "a hidden picker was stepped or opened");
  picker.hidden = false;
  state.onScreen = false;
  assert.deepStrictEqual([run(() => keys.stepChannel({ key: "ArrowDown" })), run(() => keys.pickChannel())], [[false], [false]], "the channel keys acted off the channel's list");
  state.onScreen = true;
  const was = globalThis.document;
  globalThis.document = { activeElement: picker };
  try {
    assert.deepStrictEqual(run(() => keys.channelPicked()), [undefined], "a picker reached by a click handed the keys back on a change");
    assert.deepStrictEqual(run(() => keys.pickChannel()), [true, "picker focus", "picker open"]);
    assert.deepStrictEqual(run(() => keys.channelPicked()), [undefined, "settled"], "a channel chosen from Ctrl+K's picker kept the keys on it");
    assert.deepStrictEqual(run(() => keys.channelPicked()), [undefined], "the next change on the picker was taken for Ctrl+K's too");
    keys.pickChannel();
    keys.forgetPick();
    assert.deepStrictEqual(run(() => keys.channelPicked()), [undefined], "a pick forgotten by Esc handed the keys back");
    keys.pickChannel();
    globalThis.document = { activeElement: null };
    assert.deepStrictEqual(run(() => keys.channelPicked()), [undefined], "a change with the focus gone from the picker moved it");
    picker.showPicker = () => { throw new Error("NotAllowedError"); };
    assert.deepStrictEqual(run(() => keys.pickChannel()), [true, "picker focus"], "a browser refusing to open the picker from a key failed the key");
    delete picker.showPicker;
    assert.deepStrictEqual(run(() => keys.pickChannel()), [true, "picker focus"], "a picker that cannot be opened from script failed the key");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the row keys open, enter and leave threads, press the row's buttons and act only on the row they may", () => {
  const { createRowKeys } = VIEW_SHELL;
  const log = [];
  const button = (className, extra = {}) => ({ className, nodeType: 1, hidden: false, attributes: {}, getAttribute(name) { return this.attributes[name] ?? null; },
    click: () => log.push(`click ${className}`), focus: (options) => log.push(`focus ${className}${options && options.preventScroll ? " quietly" : ""}`), scrollIntoView: () => {}, ...extra });
  const rowOf = (attributes, children, id = "row") => ({ id, attributes, children, parentNode: {}, getAttribute(name) { return this.attributes[name] ?? null; },
    hasAttribute(name) { return name in this.attributes; } });
  const fold = button("fold");
  const badge = button("thread-badge");
  const row = rowOf({}, [fold, badge, button("reply-button"), button("row-more-button")]);
  const state = { acted: { pane: "channel pane", row, took: true }, card: null, columns: false, side: false, screen: "main", view: "discord", sideMode: "list",
    fromCards: false, threadList: false, selectionView: "all", reply: false, menu: null, url: "https://chat/1", composer: false };
  const keys = createRowKeys({
    rowForAct: (_target, runLine, ...rest) => {
      if (rest.length || (runLine !== undefined && runLine !== true)) log.push(`rowForAct asked ${runLine} ${rest}`);
      return runLine || state.acted.row?.getAttribute("data-read-run") !== "head" ? state.acted : { pane: "channel pane", row: null, took: true };
    },
    keyContext: () => ({ pane: state.acted.pane, row: state.acted.row }),
    openReadRun: () => log.push("open run"),
    liveUnpackedRow: () => state.unpacked || null,
    liveCardRow: () => "live card",
    selectRow: (_pane, selected, ...bring) => log.push(["select", selected.id || selected, ...bring].join(" ")),
    channelPane: () => "channel pane",
    sidePane: () => "side pane",
    focusList: (pane) => log.push(`focus list ${pane}`),
    settleFocus: () => log.push("settled"),
    actOnRow: (_row, control) => log.push(`act ${control ? control.className : "none"}`),
    cardForArrow: () => state.card,
    columnsShown: () => state.columns,
    isComposer: () => state.composer,
    closeThread: () => log.push("thread closed"),
    inSide: () => state.side,
    screen: () => state.screen,
    view: () => state.view,
    sideMode: () => state.sideMode,
    sideFromCards: () => state.fromCards,
    sideThreadId: () => state.sideThreadId || "t1",
    cardRowOnRight: (id) => (id === "t1" ? rowOf({ "data-card": "" }, [], "t1 card") : null),
    closeSideThread: () => log.push("side thread closed"),
    rowShown: () => true,
    threadListOnScreen: () => state.threadList,
    selectionView: () => state.selectionView,
    channelOnScreen: () => true,
    replyOnScreen: () => state.reply,
    byId: (id) => button(id),
    menuItems: () => [button("first item")],
    readCardAloud: () => log.push("read card"),
    readRowAloud: () => log.push("read row"),
    toggleDetails: () => log.push("details"),
    firstMessage: () => ({ id: "m1" }),
    upstreamUrlOf: () => state.url,
    openRowMenu: () => state.menu,
    copyUpstreamLink: (url, message) => log.push(`copy ${url} ${message.id}`),
    copyDeepLink: (message) => log.push(`copy deep ${message.id}`),
  });
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  const was = globalThis.document;
  globalThis.document = { activeElement: null };
  try {
    const enter = { key: "Enter" };
    assert.deepStrictEqual(run(() => keys.openFromKeyboard(enter, {})), [true, "act fold"]);
    assert.deepStrictEqual(run(() => keys.openFromKeyboard(enter, { tagName: "BUTTON" })), [false], "Enter on a button in a row was not left to it");
    row.attributes["data-read-run"] = "head";
    assert.deepStrictEqual(run(() => keys.openFromKeyboard({ key: "o" }, {})), [true, "open run", "select row false"], "o on the line of a run did not open it and keep the keys on it");
    row.parentNode = null;
    state.unpacked = "unpacked row";
    assert.deepStrictEqual(run(() => keys.openFromKeyboard({ key: "o" }, {})), [true, "open run", "select unpacked row false"], "a run's line drawn anew in an unpacked block lost the keys");
    state.unpacked = null;
    assert.deepStrictEqual(run(() => keys.openFromKeyboard({ key: "o" }, {})), [true, "open run", "select live card false"], "a run's line drawn anew over the cards lost the keys");
    row.parentNode = {};
    assert.deepStrictEqual(run(() => keys.readFromKeyboard({})), [true], "s on the line of a run read it, or was not taken");
    delete row.attributes["data-read-run"];
    const cardRow = rowOf({ "data-card": "" }, [button("side-card")]);
    state.acted = { pane: "side pane", row: cardRow, took: true };
    assert.deepStrictEqual(run(() => keys.openFromKeyboard(enter, {})), [true, "click side-card", "focus list side pane"], "Enter on a card did not press it and keep the keys on the cards");
    cardRow.parentNode = null;
    assert.deepStrictEqual(run(() => keys.openFromKeyboard(enter, {})), [true, "click side-card", "select live card false"], "a card drawn anew by its press lost the keys");
    state.acted = { pane: null, row: null, took: true };
    assert.deepStrictEqual(run(() => keys.openFromKeyboard(enter, {})), [true], "a key taken with no row to act on was not reported taken");
    assert.deepStrictEqual(run(() => keys.pressRowButton({}, "reply-button")), ["took"]);
    assert.deepStrictEqual([run(() => keys.replyFromKeyboard({})), run(() => keys.menuFromKeyboard({}))], [[true], [true]], "r or . with no row to press, but taken, moved the focus");
    state.acted = { pane: null, row: null, took: false };
    assert.deepStrictEqual([run(() => keys.pressRowButton({}, "reply-button")), run(() => keys.replyFromKeyboard({}))], [[null], [false]]);
    state.acted = { pane: "channel pane", row, took: true };
    state.reply = true;
    assert.deepStrictEqual(run(() => keys.replyFromKeyboard({})), [true, "click reply-button", "focus reply-text"], "r did not open the reply and put the keys in its box");
    assert.deepStrictEqual(run(() => keys.menuFromKeyboard({})), [true, "click row-more-button", "focus first item quietly"]);
    assert.deepStrictEqual(run(() => keys.threadFromKeyboard({})), [true, "click thread-badge", "settled"]);
    badge.attributes["data-thread-state"] = "start";
    state.composer = true;
    assert.deepStrictEqual(run(() => keys.threadFromKeyboard({})), [true, "click thread-badge"], "a thread with no replies took the focus from its box");
    state.composer = false;
    state.columns = true;
    assert.deepStrictEqual(run(() => keys.threadFromKeyboard({})), [true, "click thread-badge", "focus list side pane"], "in two columns the keys did not go with the thread");
    badge.hidden = true;
    assert.deepStrictEqual(run(() => keys.threadFromKeyboard({})), [true], "a hidden thread tag was pressed");
    badge.hidden = false;
    state.card = rowOf({}, [button("thread-card-open")]);
    assert.deepStrictEqual(run(() => keys.threadFromKeyboard({})), [true, "click thread-card-open", "focus list side pane"], "→ on a card did not open its thread");
    state.card = rowOf({ "data-card": "" }, [button("side-card-open"), button("thread-card-open")]);
    state.columns = false;
    assert.deepStrictEqual(run(() => keys.threadFromKeyboard({})), [true, "click side-card-open", "settled"], "→ on a thread card did not press its replies button, or did not settle the focus in one column");
    state.columns = true;
    state.card = null;
    assert.deepStrictEqual(run(() => keys.threadOutFromKeyboard()), [false], "← from the left column in two columns was taken");
    state.side = true;
    assert.deepStrictEqual(run(() => keys.threadOutFromKeyboard()), [true, "focus list channel pane"]);
    state.sideMode = "thread";
    state.fromCards = true;
    assert.deepStrictEqual(run(() => keys.threadOutFromKeyboard()), [true, "side thread closed", "select t1 card"],
      "← from a thread entered from its card did not go back to the cards, with the keys on that card brought into view");
    state.sideThreadId = "gone";
    assert.deepStrictEqual(run(() => keys.threadOutFromKeyboard()), [true, "side thread closed", "focus list side pane"], "← with the card it came from gone did not leave the keys on the cards");
    state.sideThreadId = "t1";
    state.side = false;
    state.columns = false;
    assert.deepStrictEqual(run(() => keys.threadOutFromKeyboard()), [false], "← outside a thread was taken");
    state.selectionView = "thread";
    assert.deepStrictEqual(run(() => keys.threadOutFromKeyboard()), [true, "thread closed", "settled"]);
    state.threadList = true;
    assert.deepStrictEqual(run(() => keys.threadOutFromKeyboard()), [false], "← over the thread list left a thread not on screen");
    state.threadList = false;
    assert.deepStrictEqual([run(() => keys.columnFromKeyboard(true)), (state.columns = true, run(() => keys.columnFromKeyboard(true)))], [[false], [true, "focus list side pane"]]);
    assert.deepStrictEqual(run(() => keys.readFromKeyboard({})), [true, "read row"]);
    state.acted = { pane: "side pane", row: rowOf({ "data-card": "" }, []), took: true };
    assert.deepStrictEqual(run(() => keys.readFromKeyboard({})), [true, "read card"], "s on a card did not read it as a press does");
    state.acted = { pane: "channel pane", row, took: true };
    assert.deepStrictEqual(run(() => keys.detailsFromKeyboard({})), [true, "details"]);
    state.menu = { root: fold, close: () => log.push("menu closed") };
    fold.parentNode = row;
    assert.deepStrictEqual(run(() => keys.copyLinkFromKeyboard({})), [true, "menu closed", "copy https://chat/1 m1"], "l left the row's own menu open over the status");
    state.menu = { root: button("another row's item"), close: () => log.push("menu closed") };
    assert.deepStrictEqual(run(() => keys.copyLinkFromKeyboard({})), [true, "copy https://chat/1 m1"], "l closed another row's menu");
    state.url = null;
    assert.deepStrictEqual(run(() => keys.copyLinkFromKeyboard({})), [false], "l on a row with no address was taken");
    // `#889 voice-page-copy-link-key`: Cmd+L copies the vibe-talk link, address in the service or none, and shuts
    // the row's own menu as l does; never from a box being typed in.
    assert.deepStrictEqual(run(() => keys.deepLinkFromKeyboard({})), [true, "copy deep m1"], "Cmd+L on a row with no service address copied nothing");
    state.menu = { root: fold, close: () => log.push("menu closed") };
    assert.deepStrictEqual(run(() => keys.deepLinkFromKeyboard({})), [true, "menu closed", "copy deep m1"], "Cmd+L left the row's own menu open");
    assert.deepStrictEqual(run(() => keys.deepLinkFromKeyboard({ tagName: "TEXTAREA" })), [false], "Cmd+L was taken in a box being typed in");
    // A press that only moved the selection onto the screen is taken, not also given to the address bar.
    const actedWas = state.acted;
    state.acted = { pane: null, row: null, took: true };
    assert.deepStrictEqual(run(() => keys.deepLinkFromKeyboard({})), [true], "Cmd+L that moved the selection went to the browser too");
    state.acted = { pane: null, row: null, took: false };
    assert.deepStrictEqual(run(() => keys.deepLinkFromKeyboard({})), [false], "Cmd+L with no row was taken");
    state.acted = actedWas;
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: n goes to the next unread row below the selection or the screen's top, and e marks Done and moves on without taking the focus from a box", () => {
  const { createDoneKeys } = VIEW_SHELL;
  const log = [];
  const rowOf = (id, attributes = {}) => ({ id, attributes, getAttribute(name) { return name === "data-id" ? id : this.attributes[name] ?? null; }, hasAttribute(name) { return name in this.attributes; } });
  let rows = [rowOf("1"), rowOf("2"), rowOf("3"), rowOf("c4", { "data-card": "" }), rowOf("5")];
  const state = { read: new Set(["1", "2", "3"]), done: new Set(), inView: new Set(["2", "3", "c4", "5"]), row: null, took: true, again: true, after: "unread" };
  const keys = createDoneKeys({
    keyContext: () => ({ pane: "pane", row: state.row }),
    rowForAct: (_target, runLine, stay) => { log.push(`rowForAct ${runLine} ${stay}`); return { pane: state.row ? "pane" : null, row: state.row, took: state.took }; },
    paneRows: () => rows,
    rowInView: (_pane, row) => state.inView.has(row.id),
    rowIsRead: (row) => state.read.has(row.id),
    selectRow: (...args) => log.push(`select ${args.slice(1).map((one) => one.id).join(" ")}`),
    setStatus: (text) => log.push(`say ${text}`),
    paneRowFor: (_pane, id, card) => (state.again ? rows.find((row) => row.id === id && row.hasAttribute("data-card") === card) || null : null),
    nearRow: (_pane, at) => rows[at] || null,
    takeRow: (_pane, row, focus) => log.push(`take ${row.id} ${focus}`),
    bringIntoView: (...args) => log.push(`bring ${args.slice(1).map((one) => one.id).join(" ")}`),
    idsOf: (row) => (row.hasAttribute("data-card") ? [] : [row.id]),
    isDone: (id) => state.done.has(id),
    toggleDone: (ids) => log.push(`toggle ${ids}`),
    afterDone: () => state.after,
  });
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  assert.deepStrictEqual(run(() => keys.nextUnreadFromKeyboard({})), [true, "select c4"], "n with nothing selected did not start below the screen's top, or skipped a card with unread replies");
  rows[3].attributes["data-thread-read"] = "";
  state.row = rows[0];
  assert.deepStrictEqual(run(() => keys.nextUnreadFromKeyboard({})), [true, "select 5"], "n from a selection out of sight did not start at the screen's top, or stopped on a card said read");
  state.row = rows[4];
  assert.deepStrictEqual(run(() => keys.nextUnreadFromKeyboard({})), [true, "say Nothing unread below."]);
  state.row = rows[1];
  state.read.delete("3");
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "rowForAct false false", "toggle 2", "take 3 true", "bring 3"], "e did not mark Done and move to the next unread with the focus");
  state.done.add("2");
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "rowForAct false false", "toggle 2", "take 2 true", "bring 2"], "e on a Done message moved on, or did not stay on it");
  state.done.delete("2");
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({ tagName: "TEXTAREA" })), [true, "rowForAct false true", "toggle 2", "take 3 false", "bring 3"], "e from a box took the focus from it");
  state.again = false;
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "rowForAct false false", "toggle 2", "take 2 true", "bring 2"], "a Done whose rows went did not land on the row now where it was");
  state.row = rows[3];
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [false, "rowForAct false false"], "e on a card with no message of its own was taken");
  state.row = null;
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "rowForAct false false"]);
  state.took = false;
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [false, "rowForAct false false"]);
  state.row = rows[2];
  state.took = true;
  state.read = new Set(["1", "2", "3", "5"]);
  delete rows[3].attributes["data-thread-read"];
  state.again = true;
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "rowForAct false false", "toggle 3", "take c4 true", "bring c4"],
    "a Done did not move on to the card with unread replies, found again as a card");
  state.read = new Set(["3", "5"]);
  state.inView = new Set(["2", "3"]);
  state.row = null;
  assert.deepStrictEqual(run(() => keys.nextUnreadFromKeyboard({})), [true, "select 2"],
    "n with nothing selected did not start at the screen's top: it took the unread row above the screen, or skipped the first in sight");
  state.row = rows[4];
  assert.deepStrictEqual(run(() => keys.nextUnreadFromKeyboard({})), [true, "select 2"], "n from a selection out of sight below did not start at the screen's top");
  rows = [];
  assert.deepStrictEqual(run(() => keys.nextUnreadFromKeyboard({})), [false]);
});

test("`#1314 unread-step-keys`: Shift+arrows step the selection through unread rows only, and stay with a hint at either end", () => {
  const { createDoneKeys } = VIEW_SHELL;
  const log = [];
  const rowOf = (id, attributes = {}) => ({ id, attributes, getAttribute(name) { return name === "data-id" ? id : this.attributes[name] ?? null; }, hasAttribute(name) { return name in this.attributes; } });
  const rows = [rowOf("1"), rowOf("2"), rowOf("3"), rowOf("4"), rowOf("5")];
  const state = { read: new Set(["1", "3", "4"]), row: rows[0] };
  const keys = createDoneKeys({
    keyContext: () => ({ pane: "pane", row: state.row }),
    rowForAct: () => ({ pane: null, row: null, took: false }),
    paneRows: () => rows,
    rowInView: () => true,
    rowIsRead: (row) => state.read.has(row.id),
    selectRow: (_pane, row) => { state.row = row; log.push(`select ${row.id}`); },
    setStatus: (text) => log.push(`say ${text}`),
    paneRowFor: () => null,
    nearRow: () => null,
    takeRow: () => {},
    bringIntoView: () => {},
    idsOf: (row) => [row.id],
    isDone: () => false,
    toggleDone: () => {},
  });
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  // Down: only the unread rows, in order (2, then 5), then the end stop.
  assert.deepStrictEqual(run(() => keys.stepUnreadFromKeyboard({}, 1)), [true, "select 2"], "Shift+Down did not skip the read rows below");
  assert.deepStrictEqual(run(() => keys.stepUnreadFromKeyboard({}, 1)), [true, "select 5"], "Shift+Down stopped on a read row");
  assert.deepStrictEqual(run(() => keys.stepUnreadFromKeyboard({}, 1)), [true, "say No more unread below."], "Shift+Down at the last unread did not stay with the hint");
  assert.equal(state.row.id, "5", "the selection moved at the end");
  // Up: back the same way (2), then the end stop above.
  assert.deepStrictEqual(run(() => keys.stepUnreadFromKeyboard({}, -1)), [true, "select 2"], "Shift+Up did not skip the read rows above");
  assert.deepStrictEqual(run(() => keys.stepUnreadFromKeyboard({}, -1)), [true, "say No more unread above."], "Shift+Up at the first unread did not stay with the hint");
  assert.equal(state.row.id, "2");
  // No selection: down starts below the screen's top; up finds nothing above it and says so.
  state.row = null;
  assert.deepStrictEqual(run(() => keys.stepUnreadFromKeyboard({}, 1)), [true, "select 2"]);
  state.row = null;
  assert.deepStrictEqual(run(() => keys.stepUnreadFromKeyboard({}, -1)), [true, "say No more unread above."], "Shift+Up with nothing selected went somewhere");
});

test("`#1321 after-done-setting`: e stays, goes to the next message, or goes to the next unread, as Settings says", () => {
  const { createDoneKeys } = VIEW_SHELL;
  const log = [];
  const rowOf = (id, attributes = {}) => ({ id, attributes, getAttribute(name) { return name === "data-id" ? id : this.attributes[name] ?? null; }, hasAttribute(name) { return name in this.attributes; } });
  const rows = [rowOf("1"), rowOf("2"), rowOf("3"), rowOf("4")];
  const state = { read: new Set(["3"]), done: new Set(), row: rows[0], after: "stay" };
  const keys = createDoneKeys({
    keyContext: () => ({ pane: "pane", row: state.row }),
    rowForAct: () => ({ pane: "pane", row: state.row, took: true }),
    paneRows: () => rows,
    rowInView: () => true,
    rowIsRead: (row) => state.read.has(row.id),
    selectRow: () => {},
    setStatus: (text) => log.push(`say ${text}`),
    paneRowFor: (_pane, id, card) => rows.find((row) => row.id === id && row.hasAttribute("data-card") === card) || null,
    nearRow: (_pane, at) => rows[at] || null,
    takeRow: (_pane, row, focus) => log.push(`take ${row.id} ${focus}`),
    bringIntoView: (_pane, row) => log.push(`bring ${row.id}`),
    idsOf: (row) => [row.id],
    isDone: (id) => state.done.has(id),
    toggleDone: (ids) => log.push(`toggle ${ids}`),
    afterDone: () => state.after,
  });
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  // Stay (the default): marked, and the selection lands back on the message itself.
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "toggle 1", "take 1 true", "bring 1"], "e under Stay moved the selection");
  // Next message: the row below, read or not (3 is read; from 2 it is next anyway, from 1 it is 2).
  state.after = "next";
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "toggle 1", "take 2 true", "bring 2"], "e under Next message did not select the next message");
  // Next unread: past the read row 3 to 4, from row 2.
  state.row = rows[1];
  state.after = "unread";
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "toggle 2", "take 4 true", "bring 4"], "e under Next unread did not skip the read row");
  state.after = "next";
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "toggle 2", "take 3 true", "bring 3"], "e under Next message stopped on skipping a read row");
  // Putting one back never moves, whatever the choice.
  state.done.add("2");
  state.after = "unread";
  assert.deepStrictEqual(run(() => keys.doneFromKeyboard({})), [true, "toggle 2", "take 2 true", "bring 2"], "e putting a message back moved the selection");
});

test("`#624 voice-page-modularity`: Shift+E's fold keeps the reader's place, and Shift+O folds a message away, marks it Done and steps above it", () => {
  const { createFoldKeys } = VIEW_SHELL;
  const log = [];
  const folds = new Map();
  const rowOf = (id, attributes = {}, extra = {}) => ({ id, attributes, foldEntry: { id }, getAttribute(name) { return this.attributes[name] ?? null; },
    hasAttribute(name) { return name in this.attributes; }, ...extra });
  const area = { scrollTop: 500 };
  const pane = { scroller: () => area, head: () => 100, foot: () => 700 };
  let rows = [rowOf("1"), rowOf("2"), rowOf("3")];
  const state = { acted: { pane, row: rows[2], took: true }, links: false, bottom: false, done: new Set(["1"]), root: false, unpackRoot: false };
  const folded = new Map();
  const keys = createFoldKeys({
    showingLinks: () => state.links,
    isFolded: (entry) => folds.get(entry.id) === true,
    setFolded: (entry, on) => { log.push(`fold ${entry.id} ${on}`); folds.set(entry.id, on); state.rect = state.after; },
    atBottom: () => state.bottom,
    preservingCollapse: (area2, row2, head, foot, act) => {
      // The owner's per-act choice, modelled on this fixture's geometry (`#1251`, `#1215`):
      // at the foot or with the head on screen the act is simply held; with the head above the
      // viewport the next row abuts the foot here, so holding it moves by the foot's shift; a
      // row taller than the screen is centered in the room.
      const was = row2.getBoundingClientRect();
      if (state.bottom || was.top >= head - 1) { log.push("hold"); act(); return; }
      act();
      const now = row2.getBoundingClientRect();
      area2.scrollTop += was.bottom <= foot ? now.bottom - was.bottom : (now.top + now.bottom) / 2 - (head + foot) / 2;
    },
    renderScrollTools: () => log.push("tools"),
    rowForAct: (...args) => { if (args.length !== 1) log.push(`rowForAct ${args.length} args`); return state.acted; },
    paneRows: () => rows,
    shownRowFor: (row) => rows.find((one) => one.id === row.attributes.shownAs) || null,
    rowMessages: () => [{ thread: { is_root: state.unpackRoot } }],
    isThreadRoot: (message) => Boolean(message.thread && message.thread.is_root),
    isOpenThreadRoot: (row) => state.root && row === state.rootRow,
    setStatus: (text) => log.push(`say ${text}`),
    idsOf: (row) => (row.attributes.empty ? [] : [row.id]),
    threadViewKey: (id) => `thread:${id}`,
    columnViewKey: (column) => `${column}:view`,
    paneColumn: () => "main",
    inColumn: (column, act) => { log.push(`in ${column}`); act(); },
    foldedOnes: () => folded,
    closeReadRun: (key, ids) => log.push(`close run ${key} ${ids}`),
    isDone: (id) => state.done.has(id),
    markDone: (ids, row) => log.push(`done ${ids} of ${row.id}`),
    renderSideColumn: () => log.push("side drawn"),
    applyReadRuns: () => log.push("runs"),
    paneRowFor: (_pane, id, card) => (card === false ? rows.find((one) => one.id === id) || null : null),
    selectRow: (...args) => log.push(`select ${args[1].id}${args.length > 2 ? " +" : ""}`),
  });
  const run = (act) => { log.length = 0; return [act(), ...log]; };
  const tall = rows[1];
  tall.getBoundingClientRect = () => state.rect;
  state.rect = { top: 150, bottom: 400 };
  state.after = { top: 150, bottom: 180 };
  assert.deepStrictEqual(run(() => keys.foldForRead(pane, tall)), [true, "hold", "fold 2 true", "tools"], "a fold with its head on screen was not the fold control's own held act");
  assert.deepStrictEqual(run(() => keys.foldForRead(pane, tall)), [true], "a folded row was folded again");
  folds.clear();
  state.rect = { top: 40, bottom: 400 };
  state.after = { top: 40, bottom: 60 };
  assert.deepStrictEqual([run(() => keys.foldForRead(pane, tall)), area.scrollTop], [[true, "fold 2 true", "tools"], 160], "with its head off the top, its foot on screen did not hold");
  folds.clear();
  area.scrollTop = 500;
  state.rect = { top: 40, bottom: 900 };
  state.after = { top: 40, bottom: 60 };
  assert.deepStrictEqual([run(() => keys.foldForRead(pane, tall)), area.scrollTop], [[true, "fold 2 true", "tools"], 150], "a row taller than the screen was not centered in the room");
  folds.clear();
  state.bottom = true;
  assert.deepStrictEqual(run(() => keys.foldForRead(pane, tall)), [true, "hold", "fold 2 true", "tools"], "a reader on the newest line was not kept there");
  folds.clear();
  state.links = true;
  assert.deepStrictEqual(run(() => keys.foldForRead(pane, tall)), [false], "a row showing its links was folded out of sight");
  state.links = false;
  assert.deepStrictEqual(run(() => keys.foldForRead(pane, { foldEntry: null })), [false]);
  folds.set("2", true);
  state.links = true;
  assert.deepStrictEqual(run(() => keys.foldForRead(pane, tall)), [true], "a folded row showing its links was not said to be folded");
  state.links = false;
  folds.clear();
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true, "close run main:view 3", "done 3 of 3", "in main", "runs", "tools", "select 2", "say Folded away."],
    "Shift+O did not fold the message away, mark it Done and step above it");
  assert.deepStrictEqual(folded.get("main:view"), new Set(["3"]));
  state.acted = { pane, row: rows[0], took: true };
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true, "close run main:view 1", "in main", "runs", "tools", "select 1", "say Folded away."],
    "a message already Done was marked again, or with nothing above the key did not stay on it");
  const line = rowOf("line", { "data-read-run": "head" });
  rows = [rowOf("1"), rowOf("2"), line];
  state.acted = { pane, row: line, took: true };
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})).slice(0, 2), [true, "close run main:view 2"], "Shift+O on a run's line did not fold the message above it");
  const member = rowOf("m", { "data-read-run": "member", shownAs: "x" });
  rows = [rowOf("x"), member, line];
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})).slice(0, 3), [true, "close run main:view x", "done x of x"],
    "a run's line above a folded member did not fold the message that member is shown as");
  rows = [line];
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true], "a run's line with nothing above it did not take the key, or folded something");
  rows = [rowOf("card", { "data-card": "" }), line];
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true], "a run's line under a card folded the card");
  rows = [rowOf("1", { empty: true }), line];
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [false], "a row with no messages was folded");
  rows = [rowOf("1"), rowOf("2"), line];
  state.root = true;
  state.acted = { pane, row: rows[0], took: true };
  state.rootRow = rows[1];
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})).slice(0, 2), [true, "close run main:view 1"], "the thread's first message was asked about another row");
  state.rootRow = rows[0];
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true, "say The thread's first message stays."]);
  state.root = false;
  const unpacked = rowOf("u1", { "data-unpack-thread": "t9" });
  rows = [rowOf("u0"), unpacked];
  state.acted = { pane, row: unpacked, took: true };
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true, "close run thread:t9 u1", "done u1 of u1", "side drawn", "select u0", "say Folded away."], "an unpacked thread's message was not folded in its thread's view");
  state.unpackRoot = true;
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true, "say The thread's first message stays."], "an unpacked thread's first message was folded");
  state.acted = { pane, row: rowOf("card", { "data-card": "" }), took: true };
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [false], "a card was folded");
  state.acted = { pane: null, row: null, took: true };
  assert.deepStrictEqual(run(() => keys.foldOneFromKeyboard({})), [true]);
});

test("`#624 voice-page-modularity`: ↓ and ↑ read on through a tall message, start from what is on screen, walk an open menu round, and a key acts only on a row in sight", () => {
  const { createSelectionKeys, READ_STEP } = VIEW_SHELL;
  assert.equal(READ_STEP, 0.5);
  const log = [];
  const area = { scrollTop: 1000 };
  const pane = { scroller: () => area, head: () => 100, foot: () => 500 };
  const rowOf = (id, top, bottom, attributes = {}) => ({ id, attributes, getBoundingClientRect: () => ({ top, bottom }),
    getAttribute(name) { return this.attributes[name] ?? null; }, hasAttribute(name) { return name in this.attributes; } });
  const rows = [rowOf("1", -300, 50), rowOf("2", 120, 300), rowOf("3", 320, 900), rowOf("4", 300, 480)];
  const state = { pane, current: null, inView: new Set(["2", "3"]), landing: false, landed: null, menu: null };
  const item = (id, tag = "BUTTON", extra = {}) => ({ id, tagName: tag, hidden: false, focus: (options) => log.push(`focus ${id}${options && options.preventScroll ? " quietly" : ""}`), scrollIntoView: () => {}, ...extra });
  const keys = createSelectionKeys({
    paneFor: (node) => (node && node.outside ? null : state.pane),
    paneRows: () => rows,
    currentRow: () => state.current,
    rowInView: (_pane, row) => state.inView.has(row.id),
    firstRowInView: () => rows.find((row) => state.inView.has(row.id)) || null,
    landingHolds: () => state.landing,
    landingFirst: () => state.landed,
    selectRow: (_pane, row, ...rest) => log.push(["select", row.id, ...rest].join(" ")),
    takeRow: (_pane, row, focus) => log.push(`take ${row.id} ${focus}`),
    landingGap: () => 8,
    openOverscroll: () => log.push("room opened"),
    openRowMenu: () => state.menu,
    setStatus: (text) => log.push(`say ${text}`),
  });
  const run = (act) => { log.length = 0; area.scrollTop = 1000; return [act(), ...log, area.scrollTop]; };
  const was = globalThis.document;
  globalThis.document = { activeElement: null };
  try {
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1)), [true, "select 2", 1000], "↓ with nothing selected did not take the top message showing");
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, -1)), [true, "select 3", 1000], "↑ with nothing selected did not take the bottom message showing");
    state.landing = true;
    state.landed = rows[3];
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1)), [true, "select 4", 1000], "the first key in a thread just entered did not take the message the entry landed on");
    state.landing = false;
    state.current = rows[2];
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1)), [true, "select 3 false", 1200], "↓ on a message running past the foot did not read half a room on through it, or the ring was not drawn");
    state.current = rows[1];
    globalThis.document.activeElement = rows[1];
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1)), [true, "select 3 true 1", 1000], "↓ on a message that fits did not move on to the next, from above");
    state.current = rows[3];
    state.inView = new Set(["4"]);
    globalThis.document.activeElement = rows[3];
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1)), [true, "room opened", 1200], "↓ past the last message did not open the room past the foot and scroll on");
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1, true)), [true, 1200], "a held ↓ past the last message opened the room past the foot");
    state.current = rows[0];
    state.inView = new Set(["1"]);
    globalThis.document.activeElement = rows[0];
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, -1)), [true, 800], "↑ on a message above the head did not read back through it, half a room at most");
    const justPast = rowOf("p", 200, 520);
    rows.push(justPast);
    state.current = justPast;
    state.inView = new Set(["p"]);
    globalThis.document.activeElement = justPast;
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1)), [true, 1028], "↓ on a message just past the foot did not bring its foot up with the landing gap");
    const justAbove = rowOf("q", 80, 300);
    rows.push(justAbove);
    state.current = justAbove;
    state.inView = new Set(["q"]);
    globalThis.document.activeElement = justAbove;
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, -1)), [true, 972], "↑ on a message just above the head did not bring its head down with the landing gap");
    rows.splice(4);
    state.current = null;
    state.inView = new Set();
    assert.deepStrictEqual([run(() => keys.stepFromKeyboard({}, 1)), run(() => keys.stepFromKeyboard({}, -1))], [[true, "select 1", 1000], [true, "select 4", 1000]],
      "with no row in sight the keys did not take the list's first or last row");
    const empty = rows.splice(0);
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({}, 1)), [false, 1000], "↓ in an empty list was taken");
    rows.push(...empty);
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({ outside: true }, 1)), [false, 1000], "↓ outside any list was taken");
    const opener = item("opener");
    const items = [item("a"), item("b", "A"), item("c", "BUTTON", { hidden: true }), { tagName: "DIV", hidden: false, children: [item("d")] }, opener];
    const root = { children: items };
    for (const one of [...items, ...items[3].children]) one.parentNode = one === items[3].children[0] ? items[3] : root;
    state.menu = { root, button: opener };
    assert.deepStrictEqual(keys.menuItems().map((one) => one.id), ["a", "b", "d"], "the menu's items are not its shown buttons and links, in order, without its own opener");
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard(items[3].children[0], 1)), [true, "focus a quietly", 1000], "↓ on the last item did not go round to the first");
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard(items[0], -1)), [true, "focus d quietly", 1000], "↑ on the first item did not go round to the last");
    assert.deepStrictEqual([run(() => keys.stepFromKeyboard(items[3], 1)), run(() => keys.stepFromKeyboard(items[3], -1))], [[true, "focus a quietly", 1000], [true, "focus d quietly", 1000]],
      "from a part of the menu that is no item, ↓ and ↑ did not take its first and last items");
    state.current = rows[1];
    state.inView = new Set(["2"]);
    globalThis.document.activeElement = rows[1];
    assert.deepStrictEqual(run(() => keys.stepFromKeyboard({ outside: false }, 1)), [true, "select 3 true 1", 1000], "with a menu open and the focus outside it, ↓ did not move the selection");
    state.menu = null;
    assert.deepStrictEqual(keys.menuItems(), []);
    state.current = rows[1];
    state.inView = new Set(["2"]);
    assert.deepStrictEqual(keys.rowForAct({}), { pane, row: rows[1], took: false });
    state.current = rowOf("card", 0, 10, { "data-card": "" });
    assert.deepStrictEqual([keys.rowForAct({}), keys.rowForAct({}, true)], [{ pane, row: null, took: false }, { pane, row: state.current, took: false }], "a card was acted on as a message, or not by Enter and o");
    state.current = rowOf("line", 0, 10, { "data-read-run": "head" });
    log.length = 0;
    assert.deepStrictEqual([keys.rowForAct({}), log], [{ pane, row: null, took: true }, ["say Read messages: Enter opens them."]], "a key about one message acted on a run's line");
    state.current = rows[3];
    log.length = 0;
    assert.deepStrictEqual([keys.rowForAct({}, false, true), log], [{ pane, row: null, took: true }, ["take 2 false", "say Selected the first message showing."]],
      "a row out of sight was acted on, or the first showing took the focus from a box");
    state.inView = new Set();
    log.length = 0;
    assert.deepStrictEqual([keys.rowForAct({}), log], [{ pane, row: null, took: false }, []]);
    state.pane = null;
    assert.deepStrictEqual([keys.keyContext({}), keys.rowForAct({})], [{ pane: null, row: null }, { pane: null, row: null, took: false }]);
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the mouse and the keys move one selection, the focus follows it only off fields and keyboard controls, and a page of scrolling moves the ring once", () => {
  const { createPointerSelection } = VIEW_SHELL;
  const log = [];
  const node = (id, parentNode = null, extra = {}) => ({ id, parentNode, nodeType: 1, hidden: false, ...extra });
  const page = node("page");
  const scrollerOf = (id) => {
    const listeners = [];
    return node(id, page, { scrollTop: 100, scrollHeight: 1000, clientHeight: 400, onscrollend: null, listeners,
      addEventListener: (type, listener) => listeners.push([type, listener]), focus: (options) => log.push(`focus ${id}${options && options.preventScroll ? " quietly" : ""}`) });
  };
  const paneOf = (name) => {
    const scroller = scrollerOf(`${name}-scroll`);
    const list = node(`${name}-list`, scroller);
    return { name, scroller: () => scroller, list: () => list, head: () => 50, lit: null, row: null, follow: null };
  };
  const channel = paneOf("channel");
  const side = paneOf("side");
  const rowIn = (pane, id, top = 100) => node(id, pane.list(), { top, getBoundingClientRect: () => ({ top }),
    setAttribute: (name, value) => log.push(`${id} ${name}=${value}`), focus: (options) => log.push(`focus ${id}${options && options.preventScroll ? " quietly" : ""}`) });
  const one = rowIn(channel, "1");
  const two = rowIn(channel, "2", 300);
  const sideRow = rowIn(side, "s");
  const header = node("side-thread-card", page);
  const transcript = node("transcript", page);
  const leftArea = node("scroll-area", page);
  const sideColumn = node("side-column", page);
  const ids = { "side-thread-card": header, transcript, "scroll-area": leftArea, "side-column": sideColumn };
  const state = { screen: "main", view: "discord", lost: false, last: channel, menu: false, moved: true, drawn: true, marks: new Map(), side: false,
    inView: new Set(["1", "2", "s"]), drawnRow: null, visible: null, threadList: false, sideUp: true };
  const selection = createPointerSelection({
    inputKinds: {
      keys: () => log.push("keys"), mouse: () => log.push("mouse"), touch: () => log.push("touch"),
      mouseMoved: () => state.moved, drawnBy: () => state.drawn,
    },
    screen: () => state.screen,
    view: () => state.view,
    focusLost: () => state.lost,
    keyPanes: () => [channel, side],
    paneFor: (held) => [channel, side].find((pane) => held && pane.scroller() === held.paneScroller) || null,
    channelPane: () => channel,
    sidePane: () => side,
    lastPane: () => state.last,
    takeLastPane: (pane) => { state.last = pane; log.push(`last ${pane.name}`); },
    rowMenuOpen: () => state.menu,
    takeRow: (pane, row, focus) => log.push(`take ${pane.name} ${row.id} ${focus}`),
    noteRow: (pane, row) => log.push(`note ${pane.name} ${row.id}`),
    drawSelection: () => log.push("draw"),
    rowOf: (pane, target) => [one, two, sideRow].find((row) => row.parentNode === pane.list() && (target === row || (target && target.parentNode === row))) || null,
    rowShown: (row) => !row.hidden,
    markOf: (pane) => state.marks.get(pane) || null,
    markNames: (mark, row) => Boolean(mark) && mark.id === row.id,
    byId: (id) => ids[id],
    leftById: (id) => ids[id],
    inSide: () => state.side,
    mainColumn: () => "main",
    paneColumn: (pane) => (pane === side ? "side" : "main"),
    inColumn: (column, act) => { log.push(`in ${column}`); act(); },
    readerActed: () => log.push("acted"),
    notePointerAct: (pane) => log.push(`pointer act ${pane.name}`),
    rowInView: (_pane, row) => state.inView.has(row.id),
    firstRowInView: () => [one, two].find((row) => state.inView.has(row.id)) || null,
    selectRow: (pane, row, ...rest) => log.push(["select", pane.name, row.id, ...rest].join(" ")),
    drawnRow: () => state.drawnRow,
    releaseRow: (pane, row) => log.push(`release ${pane.name} ${row.id}`),
    rowTimeTabStop: (row, on) => log.push(`time ${row.id} ${on}`),
    visibleScroller: () => state.visible,
    threadListOnScreen: () => state.threadList,
    sideColumnUp: () => state.sideUp,
  });
  const run = (act) => { log.length = 0; const result = act(); return result === undefined ? [...log] : [result, ...log]; };
  const was = globalThis.document;
  globalThis.document = { activeElement: null };
  try {
    assert.deepStrictEqual([run(() => selection.keysInUse()), run(() => selection.drawnBy())], [["keys"], [true]]);
    // Where the focus is decides whether a selection takes it.
    const button = (focusVisible) => node("button", page, { tagName: "BUTTON", matches: (selector) => selector === ":focus-visible" && focusVisible });
    const follows = (held) => { globalThis.document.activeElement = held; return selection.focusFollowsSelection(); };
    assert.deepStrictEqual(
      [follows(one), follows(channel.scroller()), follows(button(false)), follows(button(true)), follows({ tagName: "BUTTON" }),
        follows(node("box", page, { tagName: "TEXTAREA" })), follows(node("pick", page, { tagName: "SELECT" }))],
      [true, true, true, false, false, false, false],
      "the focus did not follow from a list or a clicked control, or followed out of a field, a picker, a control the keys reached or one the browser cannot say of");
    globalThis.document.activeElement = null;
    state.lost = true;
    assert.equal(selection.focusFollowsSelection(), true, "the focus lost to the body did not follow");
    state.lost = false;
    state.screen = "settings";
    assert.equal(selection.focusFollowsSelection(), false, "the focus followed off the main screen");
    state.screen = "main";
    globalThis.document.activeElement = channel.scroller();
    // The mouse moving over a row takes it, with the focus; held still, dragging, or over a menu, it takes nothing.
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: one })), ["mouse", "take channel 1 true"], "hover selected the row and scrolled nothing (#1144)");
    channel.lit = one;
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: node("text", one) })), ["mouse", "take channel 1 true"],
      "a move within the lit row did not hand it the focus it was to follow");
    globalThis.document.activeElement = button(true);
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: one })), ["mouse"], "a move within the lit row asked again");
    globalThis.document.activeElement = channel.scroller();
    channel.lit = null;
    state.moved = false;
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: one })), [], "a list scrolled under a still pointer was taken as a move");
    state.moved = true;
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: one, buttons: 1 })), ["mouse"], "a drag chose a message");
    state.menu = true;
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: one })), ["mouse"], "a move took the selection from an open menu");
    state.menu = false;
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: node("in-header", header) })), ["mouse"], "resting on the thread card scrolled Main to its root (#1144)");
    two.hidden = true;
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: two })), ["mouse"], "a hidden row was selected");
    two.hidden = false;
    state.screen = "settings";
    assert.deepStrictEqual(run(() => selection.onPointerMove({ target: one })), ["mouse"], "a move off the main screen selected");
    state.screen = "main";
    // A press says the pointer's kind, and one in a list or the transcript is the reader acting there.
    assert.deepStrictEqual(run(() => selection.onPointerDown({ pointerType: "mouse", target: sideRow })), ["mouse", "pointer act side", "in side", "acted"]);
    state.view = "voice";
    assert.deepStrictEqual(run(() => selection.onPointerDown({ pointerType: "pen", target: node("line", transcript) })), ["touch", "in main", "acted"]);
    assert.deepStrictEqual(run(() => selection.onPointerDown({ pointerType: "", target: node("line", transcript) })), ["in main", "acted"]);
    state.view = "discord";
    assert.deepStrictEqual([run(() => selection.onPointerDown({ pointerType: "touch", target: node("line", transcript) })), run(() => selection.onPointerDown(null))],
      [["touch"], []], "a press on the transcript of the channel's view, or no press at all, was the reader acting");
    // A click selects its row: drawn after the mouse, silently after a tap, not again when it is already.
    assert.deepStrictEqual(run(() => selection.noteClickedRow(channel, node("text", two))), ["take channel 2 true"]);
    state.marks.set(channel, { id: "2", drawn: true });
    assert.deepStrictEqual(run(() => selection.noteClickedRow(channel, two)), [], "a click on the row already drawn selected took it again");
    state.drawn = false;
    state.marks.set(channel, { id: "2", drawn: false });
    assert.deepStrictEqual([run(() => selection.noteClickedRow(channel, two)), run(() => selection.noteClickedRow(channel, one)), run(() => selection.noteClickedRow(channel, page))],
      [[], ["note channel 1"], []]);
    // The focus arriving in a pane makes it the one in use, and its row the selection.
    assert.deepStrictEqual([run(() => selection.onPaneFocus(side, { target: sideRow })), run(() => selection.onPaneFocus(channel, { target: two }))],
      [["note side s", "last side", "draw"], ["last channel", "draw"]]);
    state.drawn = true;
    state.marks.set(channel, { id: "2", drawn: true });
    channel.row = two;
    assert.deepStrictEqual([run(() => selection.onPaneFocus(channel, { target: two })), run(() => selection.onPaneFocus(channel, { target: one })), run(() => selection.onPaneFocus(channel, null))],
      [[], ["take channel 1 false"], []]);
    // A jump landing.
    assert.deepStrictEqual([run(() => selection.selectLanded(one)), run(() => selection.selectLanded(sideRow))], [["take channel 1 true"], []]);
    state.side = true;
    state.drawn = false;
    assert.deepStrictEqual(run(() => selection.selectLanded(sideRow)), ["note side s"]);
    state.side = false;
    state.drawn = true;
    // A page key's scroll: one listener per scroller, none for a key that cannot move it; at rest the ring to the first row showing.
    const area = channel.scroller();
    selection.followPageScroll(channel, two, false);
    selection.followPageScroll(channel, two, false);
    assert.deepStrictEqual([channel.follow, area.listeners.length], [two, 1], "a second page key added a second listener, or the row to follow was not kept");
    area.scrollTop = 0;
    selection.followPageScroll(channel, two, true);
    assert.equal(channel.follow, null, "PgUp at the top waited to move the ring");
    area.scrollTop = 600;
    selection.followPageScroll(channel, two, false);
    assert.equal(channel.follow, null, "PgDn at the end waited to move the ring");
    area.scrollTop = 300;
    selection.followPageScroll(channel, two, false);
    channel.row = two;
    globalThis.document.activeElement = two;
    state.inView = new Set(["1"]);
    assert.deepStrictEqual(run(() => area.listeners[0][1]()), ["select channel 1 false"]);
    assert.equal(channel.follow, null);
    assert.deepStrictEqual(run(() => area.listeners[0][1]()), [], "a scroll to end after the wait moved the ring");
    state.inView = new Set(["1", "2"]);
    selection.followPageScroll(channel, two, false);
    assert.deepStrictEqual(run(() => area.listeners[0][1]()), [], "the ring moved off a row still shown whole");
    const old = { scrollTop: 300, scrollHeight: 1000, clientHeight: 400 };
    const oldPane = { ...channel, scroller: () => old, follow: one };
    selection.followPageScroll(oldPane, two, false);
    assert.equal(oldPane.follow, null, "without a scrollend event the ring waited to follow");
    // Handing the keys to a list.
    globalThis.document.activeElement = null;
    state.last = side;
    state.drawnRow = two;
    assert.deepStrictEqual(run(() => selection.focusList(channel)), ["release channel 2", "2 tabindex=0", "time 2 true", "last channel", "draw", "focus 2 quietly"]);
    state.drawnRow = null;
    assert.deepStrictEqual(run(() => selection.focusList(side)), ["last side", "draw", "focus side-scroll quietly"]);
    state.visible = area;
    assert.deepStrictEqual(run(() => selection.focusList(null)), ["focus channel-scroll quietly"]);
    globalThis.document.activeElement = area;
    assert.deepStrictEqual(run(() => selection.focusList(null)), [], "the focus was taken again where it already was");
    // Settling the focus leaves a field on screen alone, and not one behind the thread list or in a column gone.
    const box = node("box", leftArea, { tagName: "TEXTAREA", paneScroller: channel.scroller() });
    globalThis.document.activeElement = box;
    assert.deepStrictEqual(run(() => selection.settleFocus()), []);
    state.threadList = true;
    assert.deepStrictEqual([selection.behindThreadList(box), selection.inColumnGone(box)], [true, false]);
    assert.deepStrictEqual(run(() => selection.settleFocus()), ["last channel", "draw", "focus channel-scroll quietly"], "a field behind the thread list kept the focus");
    state.threadList = false;
    const sideBox = node("side-box", sideColumn, { tagName: "TEXTAREA" });
    state.sideUp = false;
    globalThis.document.activeElement = sideBox;
    assert.deepStrictEqual([selection.inColumnGone(sideBox), selection.behindThreadList(sideBox)], [true, false]);
    state.visible = side.scroller();
    assert.deepStrictEqual(run(() => selection.settleFocus()), ["focus side-scroll quietly"], "a field in a column gone kept the focus");
    state.sideUp = true;
    state.screen = "settings";
    globalThis.document.activeElement = node("button", page, { tagName: "BUTTON", paneScroller: channel.scroller() });
    assert.deepStrictEqual(run(() => selection.settleFocus()), ["focus side-scroll quietly"], "off the main screen the focus went to a pane's list");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the selection is kept per list by message id, lit on one row of the page, brought into view the least, and put back after a redraw only where the reader is", () => {
  const { createKeySelection } = VIEW_SHELL;
  const log = [];
  const element = (id, attributes = {}, extra = {}) => ({
    id, attributes: { ...attributes }, parentNode: null, nodeType: 1, hidden: false, top: 0, bottom: 0, height: 0,
    getAttribute(name) { return name in this.attributes ? this.attributes[name] : null; },
    hasAttribute(name) { return name in this.attributes; },
    setAttribute(name, value) { this.attributes[name] = String(value); },
    removeAttribute(name) { delete this.attributes[name]; },
    getBoundingClientRect() { return { top: this.top, bottom: this.bottom, height: this.bottom - this.top }; },
    focus(options) { log.push(`focus ${this.id}${options && options.preventScroll ? " quietly" : ""}`); globalThis.document.activeElement = this; },
    ...extra,
  });
  const listOf = (id, rows, parentNode) => {
    const list = element(id, {}, { parentNode, children: rows });
    for (const row of rows) row.parentNode = list;
    return list;
  };
  const rowAt = (id, top, bottom, attributes = {}) => Object.assign(element(id, { "data-id": id, ...attributes }), { top, bottom });
  const page = element("page");
  const paneOf = (name, rows) => {
    const scroller = element(`${name}-scroll`, {}, { parentNode: page, scrollTop: 500 });
    const state = { list: listOf(`${name}-list`, rows, scroller), context: "a" };
    return { name, state, scroller: () => scroller, list: () => state.list, head: () => 100, foot: () => 500,
      context: () => state.context, marks: new Map(), row: null, lit: null, follow: null };
  };
  const channel = paneOf("channel", [rowAt("1", 50, 150), rowAt("2", 150, 300), rowAt("3", 300, 480), rowAt("4", 480, 700)]);
  const side = paneOf("side", [rowAt("t", 100, 200, { "data-card": "" }), rowAt("t", 200, 300), rowAt("u", 300, 400)]);
  const reply = paneOf("reply", []);
  const heading = element("side-heading", {}, { parentNode: page });
  const compose = element("reply-compose", {}, { parentNode: page });
  const screenEl = element("screen-settings");
  const [one, two, three, four] = channel.state.list.children;
  const [card, root, other] = side.state.list.children;
  const state = { last: channel, threadList: false, sideUp: true, docked: false, screen: "main", lost: false };
  const selection = createKeySelection({
    keyPanes: () => [channel, side],
    channelPane: () => channel,
    sidePane: () => side,
    lastPane: () => state.last,
    takeLastPane: (pane) => { state.last = pane; log.push(`last ${pane.name}`); },
    threadListOnScreen: () => state.threadList,
    sideColumnUp: () => state.sideUp,
    replyDocked: () => state.docked,
    replyPane: () => reply,
    screen: () => state.screen,
    byId: (id) => (id === "screen-settings" ? screenEl : null),
    leftById: (id) => ({ "side-heading": heading, "reply-compose": compose })[id],
    rowShown: (row) => !row.hidden,
    idsOf: (row) => (row.hasAttribute("data-card") ? [] : [row.getAttribute("data-id")]),
    shownRowFor: (row) => (row.getAttribute("data-read-run") === "member" ? three : row),
    rowTimeTabStop: (row, on) => log.push(`time ${row.id} ${on}`),
    readerActedIn: (pane) => log.push(`acted ${pane.name}`),
    putOutLanding: (row) => log.push(`landing out ${row.id}`),
    noteLinkedSelection: () => log.push("links"),
    focusLost: () => state.lost,
    listDrawCount: () => 0,
    landingGap: () => 10,
  });
  const run = (act) => { log.length = 0; const result = act(); return result === undefined ? [...log] : [result, ...log]; };
  const was = globalThis.document;
  globalThis.document = { activeElement: null };
  try {
    // Which list a key is for.
    const inChannel = element("text", {}, { parentNode: two });
    assert.deepStrictEqual([selection.paneFor(inChannel), selection.paneFor(root), selection.paneFor(element("x", {}, { parentNode: heading }))], [channel, side, side]);
    state.threadList = true;
    state.last = side;
    assert.equal(selection.paneFor(inChannel), side, "behind the thread list a key in the left column did not go to the pane used last");
    state.threadList = false;
    state.sideUp = false;
    assert.equal(selection.paneFor(root), side, "with the right column gone, a key there was not handed to the pane used last");
    side.state.list = null;
    assert.equal(selection.paneFor(root), channel, "with no list on the right, the key did not go to the first pane with one");
    side.state.list = listOf("side-list", [card, root, other], side.scroller());
    state.sideUp = true;
    state.last = channel;
    state.docked = true;
    assert.equal(selection.paneFor(element("box", {}, { parentNode: compose })), reply, "a box of a reply open in the stream was not its list's");
    state.docked = false;
    assert.equal(selection.paneFor(element("box", {}, { parentNode: compose })), channel);
    assert.deepStrictEqual([selection.visibleScroller(inChannel), selection.visibleScroller(root)], [channel.scroller(), side.scroller()]);
    globalThis.document.activeElement = root;
    assert.equal(selection.visibleScroller(), side.scroller(), "the scroller was not the focus's by default");
    globalThis.document.activeElement = null;
    state.screen = "settings";
    assert.equal(selection.visibleScroller(inChannel), screenEl, "off the main screen the scroll keys were not the screen's");
    state.screen = "main";
    // The rows and the row of a node.
    three.hidden = true;
    assert.deepStrictEqual(selection.paneRows(channel).map((row) => row.id), ["1", "2", "4"], "a hidden row was moved through");
    three.hidden = false;
    channel.state.list.children.push(element("day", {}));
    channel.state.list.children.at(-1).parentNode = channel.state.list;
    assert.deepStrictEqual(selection.paneRows(channel).map((row) => row.id), ["1", "2", "3", "4"], "a part of the list with no message was moved through");
    assert.deepStrictEqual([selection.rowOf(channel, inChannel), selection.rowOf(channel, channel.state.list.children[4]), selection.rowOf(channel, root), selection.rowOf(channel, null)],
      [two, null, null, null]);
    channel.state.list.children.pop();
    // A message's row, a card told from its root's row, a folded member shown as its run's line.
    assert.deepStrictEqual([selection.paneRowFor(side, "t"), selection.paneRowFor(side, "t", true), selection.paneRowFor(side, "t", false), selection.paneRowFor(side, null)],
      [card, card, root, null]);
    two.attributes["data-read-run"] = "member";
    assert.equal(selection.paneRowFor(channel, "2"), three, "a folded message did not stand as its run's line");
    delete two.attributes["data-read-run"];
    // Taking a row: noted per list, the one in use, drawn; the reader acting only when it is another.
    assert.deepStrictEqual(run(() => selection.takeRow(channel, two, true)),
      ["acted channel", "last channel", "landing out 2", "links", "time 2 true", "focus 2 quietly"]);
    assert.deepStrictEqual([channel.marks.get("a"), channel.row, channel.lit, two.getAttribute("tabindex"), two.getAttribute("data-selected")],
      [{ id: "2", card: false, drawn: true, index: 1 }, two, two, "0", "true"]);
    assert.deepStrictEqual(run(() => selection.takeRow(channel, two, false)), ["last channel", "links", "time 2 true"], "the same row taken again was the reader acting, or was lit again");
    assert.deepStrictEqual(run(() => selection.takeRow(channel, three, false)),
      ["time 2 false", "acted channel", "last channel", "landing out 3", "links", "time 3 true"]);
    assert.deepStrictEqual([two.hasAttribute("tabindex"), two.hasAttribute("data-selected"), channel.lit], [false, false, three], "the row before kept its tab stop or its light");
    assert.deepStrictEqual([selection.markNames(selection.markOf(channel), three), selection.markNames(null, three), selection.markedRow(channel, selection.markOf(channel))], [true, false, three]);
    assert.deepStrictEqual([selection.currentRow(channel, inChannel), selection.currentRow(channel, null), selection.drawnRow(channel)], [two, three, three]);
    // A tap: silent. The pane in use is the one lit.
    assert.deepStrictEqual(run(() => selection.noteRow(side, card)), ["acted side", "last side", "links"]);
    assert.deepStrictEqual([side.marks.get("a"), side.row, side.lit, channel.lit, selection.drawnRow(side), selection.activePane()],
      [{ id: "t", card: true, drawn: false, index: 0 }, null, null, null, null, side], "a tap drew its row, or the other column stayed lit");
    assert.equal(selection.markNames(side.marks.get("a"), root), false, "a card's selection named its root's row");
    assert.equal(selection.markNames({ id: "t", drawn: true, index: 0 }, root), true, "a selection a filter carried did not name the row by its id alone");
    side.state.list = null;
    assert.equal(selection.activePane(), channel, "the pane used last showing no list stayed the one in use");
    side.state.list = listOf("side-list", [card, root, other], side.scroller());
    state.last = channel;
    selection.drawSelection();
    assert.equal(channel.lit, three, "the pane in use did not light its selection again");
    // Where the rows are.
    assert.deepStrictEqual([selection.rowInView(channel, one), selection.rowInView(channel, four), selection.rowInView(channel, rowAt("x", 0, 100)), selection.firstRowInView(channel)],
      [true, true, false, two], "the first row shown whole was not the first between the line and the chips");
    one.top = 99; four.top = 501; four.bottom = 700;
    assert.equal(selection.firstRowInView(channel), one, "a row a pixel under the line was not counted whole");
    one.top = 50;
    assert.deepStrictEqual([selection.nearRow(channel, 9), selection.nearRow(channel, -2), selection.nearRow(reply, 0)], [four, one, null]);
    // Bringing into view the least, with the landing gap; a row taller than the room, arrived at going up, shows its foot.
    const area = channel.scroller();
    const bring = (row, from) => { area.scrollTop = 500; selection.bringIntoView(channel, row, from); return area.scrollTop; };
    assert.deepStrictEqual([bring(one), bring(two), bring(four), bring(rowAt("x", 600, 650))], [440, 500, 710, 660]);
    const tall = rowAt("tall", 0, 900);
    assert.deepStrictEqual([bring(tall), bring(tall, -1), bring(rowAt("t2", 200, 450), -1)], [390, 910, 500], "a tall row did not show its head going down and its foot going up");
    assert.deepStrictEqual(run(() => { area.scrollTop = 500; selection.selectRow(channel, four, false); return area.scrollTop; }),
      [500, "time 3 false", "acted channel", "last channel", "landing out 4", "links", "time 4 true", "focus 4 quietly"]);
    area.scrollTop = 500;
    selection.selectRow(channel, one);
    assert.equal(area.scrollTop, 440, "a row selected was not brought into view by default");
    // A redraw that took the row away: the focus back where it was, else only in the pane in use.
    const redraw = (pane, rows) => { pane.state.list = listOf(pane.state.list.id, rows, pane.scroller()); };
    const newOne = rowAt("1", 50, 150);
    globalThis.document.activeElement = one;
    redraw(channel, [newOne, rowAt("2", 150, 300)]);
    assert.deepStrictEqual(run(() => selection.keepSelection()), ["time 1 false", "time 1 true", "focus 1 quietly", "landing out 1", "links"]);
    assert.deepStrictEqual([channel.row, channel.lit, newOne.getAttribute("tabindex"), one.hasAttribute("data-selected")], [newOne, newOne, "0", false], "the new row of the message did not take the ring, or the old one kept its light");
    globalThis.document.activeElement = null;
    state.lost = true;
    selection.takeRow(side, other, false);
    state.last = side;
    redraw(channel, [rowAt("2", 150, 300), rowAt("5", 300, 400)]);
    assert.deepStrictEqual(run(() => selection.keepSelection()), ["time 1 false", "time 2 true", "links"],
      "a redraw of Main beside the reader gave Main's row the focus, or did not hand its selection to the row now where it was");
    assert.deepStrictEqual(channel.marks.get("a"), { id: "2", card: false, drawn: true, index: 0 });
    const newOther = rowAt("u", 300, 400);
    redraw(side, [card, root, newOther]);
    assert.deepStrictEqual(run(() => selection.keepSelection()), ["time u false", "time u true", "focus u quietly", "landing out u", "links"], "the pane in use did not get its focus back");
    state.lost = false;
    globalThis.document.activeElement = element("button");
    redraw(side, [card, root]);
    assert.deepStrictEqual(run(() => selection.keepSelection()), ["time u false", "links"], "with the focus elsewhere a gone message's selection was handed on");
    assert.equal(side.lit, null);
    selection.noteRow(side, root);
    side.row = root;
    const newRoot = rowAt("t", 200, 300);
    redraw(side, [card, newRoot]);
    assert.deepStrictEqual([run(() => selection.keepSelection()), newRoot.hasAttribute("tabindex"), side.row], [["time t false", "links"], false, null],
      "a silent selection was put back with a ring");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the keymap lists every key in a group, hands each to the act it names, and says the chords in the reader's words", () => {
  const { createKeymap, KEYS_PAGE, KEYS_MOVING, KEYS_MESSAGE, KEYS_VIEW, KEYS_WRITING } = VIEW_SHELL;
  const calls = [];
  const answers = { modKeyName: "Cmd", manyChatServices: false, chatServiceName: "Chat", threadOrderWords: "Newest, Oldest",
    minCollapsedLines: 0, maxCollapsedLines: 8, keyContext: { row: null }, pressRowButton: "took", afterDone: "stay" };
  const acts = new Proxy({}, { get: (_acts, name) => (...args) => {
    calls.push([name, ...args]);
    return name in answers ? answers[name] : `${String(name)} did`;
  } });
  const keymap = createKeymap(acts);
  assert.deepStrictEqual(calls, [], "making the table ran an act");
  const groups = [KEYS_PAGE, KEYS_MOVING, KEYS_MESSAGE, KEYS_VIEW, KEYS_WRITING];
  assert.deepStrictEqual([...new Set(keymap.map((binding) => binding.group))].sort(), [...groups].sort(), "a binding is in no group of the list, or a group has none");
  for (const binding of keymap) {
    assert.ok(binding.chords.length > 0 && (typeof binding.does === "string" || typeof binding.does === "function"));
    assert.equal(typeof binding.run, binding.by ? "undefined" : "function", `${JSON.stringify(binding.chords[0])}: listed only, or answered twice`);
  }
  const target = { tagName: "LI" };
  const field = { tagName: "TEXTAREA" };
  const find = (key, extra = {}) => keymap.find((binding) => binding.run && binding.chords.some((chord) =>
    chord.key === key && Boolean(chord.mod) === Boolean(extra.mod) && Boolean(chord.shift) === Boolean(extra.shift) && Boolean(chord.alt) === Boolean(extra.alt)));
  const press = (key, event = {}, on = target, chord = {}) => {
    calls.length = 0;
    const binding = find(key, chord);
    assert.ok(binding, `no binding runs ${key}`);
    return [binding.run({ key, ...event }, on), ...calls.map((call) => call.join(" "))];
  };
  const escape = { key: "Escape" };
  assert.deepStrictEqual((() => { calls.length = 0; const did = find("Escape").run(escape, target); return [did, calls[0][0], calls[0][1] === escape, calls[0][2] === target]; })(),
    ["onEscape did", "onEscape", true, true]);
  assert.deepStrictEqual([press("End"), press("Home"), press("End", { ctrlKey: true }, target, { mod: true })],
    [["listEdge did", "listEdge [object Object] true"], ["listEdge did", "listEdge [object Object] false"], ["jumpList did", "jumpList [object Object] true"]]);
  assert.deepStrictEqual([press("]", { metaKey: true }, target, { mod: true }), press("[", { ctrlKey: true }, target, { mod: true })],
    [["placeFromKeyboard did", "placeFromKeyboard 1"], ["placeFromKeyboard did", "placeFromKeyboard -1"]]);
  assert.deepStrictEqual([press("/", {}), press("/", { ctrlKey: true }, target, { mod: true })], [["searchFromKeyboard did", "searchFromKeyboard"], ["searchFromAnywhere did", "searchFromAnywhere"]]);
  // `#1038 documents-outline-popup`. g toggles the document outline; Mod+g does so from a message box too.
  assert.deepStrictEqual([press("g", {}), press("g", { ctrlKey: true }, field, { mod: true })],
    [["outlineFromKeyboard did", "outlineFromKeyboard"], ["outlineFromKeyboard did", "outlineFromKeyboard"]]);
  assert.deepStrictEqual(press("ArrowDown", { repeat: true }), ["stepFromKeyboard did", "stepFromKeyboard [object Object] 1 true"]);
  assert.deepStrictEqual(press("ArrowUp"), ["stepFromKeyboard did", "stepFromKeyboard [object Object] -1"], "↑ was given a third argument");
  assert.deepStrictEqual([press("e"), press("e", { repeat: true, ctrlKey: true }, target, { mod: true }), press("e", { repeat: true }, target, { mod: true })],
    [["doneFromKeyboard did", "doneFromKeyboard [object Object]"], [false, "keyContext [object Object]"], [false]],
    "a held e marked more than once, or a held Ctrl+E was given to the browser with a row to act on");
  assert.deepStrictEqual([press("z"), press("z", { ctrlKey: true }, target, { mod: true }), press("z", { ctrlKey: true }, field, { mod: true })],
    [["undoFromKeyboard did", "undoFromKeyboard"], ["undoFromKeyboard did", "undoFromKeyboard"], [false]], "Ctrl+Z in a box was not left to the box");
  assert.deepStrictEqual([press("c", { ctrlKey: true }, target, { mod: true }), press("c")],
    [["copyChordFromKeyboard did", "copyChordFromKeyboard"], ["copyFromKeyboard did", "copyFromKeyboard"]]);
  // `#889 voice-page-copy-link-key`. Ctrl+L (Cmd+L) and Ctrl+Shift+L (Cmd+Shift+L) copy the vibe-talk link; bare l the service's.
  assert.deepStrictEqual([press("l", { metaKey: true }, target, { mod: true }), press("l", { metaKey: true, shiftKey: true }, target, { mod: true, shift: true }),
    press("l")], [["deepLinkFromKeyboard did", "deepLinkFromKeyboard [object Object]"], ["deepLinkFromKeyboard did", "deepLinkFromKeyboard [object Object]"],
    ["copyLinkFromKeyboard did", "copyLinkFromKeyboard [object Object]"]]);
  const deepBinding = keymap.find((binding) => binding.rowControls && binding.rowControls.includes("row-copy-deep-link-button"));
  assert.equal(deepBinding.repeats, true, "a held Cmd+L's repeats would go to the address bar");
  const deepWords = () => deepBinding.does();
  answers.standalone = false;
  assert.match(deepWords(), /^Copy its vibe-talk link: .*In a browser tab, where Cmd\+L can go to the address bar, Cmd\+Shift\+L does the same$/,
    "in a browser tab the list does not name the chord the address bar cannot take");
  answers.standalone = true;
  assert.doesNotMatch(deepWords(), /browser tab/, "the installed app's list names a fallback it does not need");
  delete answers.standalone;
  assert.deepStrictEqual(press("p", {}, target, { shift: false }).slice(1), ["pressRowButton [object Object] row-pin-button"]);
  assert.equal(press("p")[0], true, "the pin key did not say it took the key when its button was pressed");
  answers.pressRowButton = null;
  assert.equal(press("p")[0], false, "the pin key took a key it pressed nothing for");
  answers.pressRowButton = "took";
  const view = (key) => press(key, { metaKey: true, repeat: true }, target, { mod: true }).slice(1)[0];
  assert.deepStrictEqual(["1", "2", "3", "4"].map(view), ["viewFromKeyboard main true", "viewFromKeyboard flat true", "viewFromKeyboard threads true", "viewFromKeyboard columns true"]);
  const link = { tagName: "A" };
  const given = (key, event = {}, chord = {}) => {
    calls.length = 0;
    find(key, chord).run({ key, ...event }, target);
    return calls.map(([name, ...args]) => [name, ...args.map((arg) => (arg === target ? "target" : arg && typeof arg === "object" ? "event" : arg))]);
  };
  assert.deepStrictEqual([given(";"), given(":", { ctrlKey: true }, { mod: true }), given("(", {}, { mod: true }), given(")", {}, { mod: true })],
    [[["foldStepFromKeyboard", 1, "event", "target"]], [["foldStepFromKeyboard", -1, "event", "target"]],
      [["collapsedLinesFromKeyboard", -1, "target"]], [["collapsedLinesFromKeyboard", 1, "target"]]]);
  assert.deepStrictEqual([given("["), given("]"), given("ArrowDown", {}, { alt: true }), given("0", { repeat: true }, { mod: true })],
    [[["columnFromKeyboard", false]], [["columnFromKeyboard", true]], [["stepChannel", "event"]], [["voiceFromKeyboard", true]]]);
  assert.deepStrictEqual(["p", "l", "m"].map((key) => given(key, {}, { shift: true })),
    [[["filterFromKeyboard", "pinned"]], [["filterFromKeyboard", "links"]], [["filterFromKeyboard", "author"]]]);
  assert.deepStrictEqual([press("Enter", { ctrlKey: true }, field, { mod: true }), press("Enter", { ctrlKey: true }, link, { mod: true }), press("Enter", { ctrlKey: true }, target, { mod: true })],
    [[false], [false], ["writeFromKeyboard did", "writeFromKeyboard"]], "Ctrl+Enter took the key from a box or a link");
  // Every act is reached by some key, and no key asks for an act the page does not give.
  const reached = new Set();
  for (const binding of keymap) {
    for (const chord of binding.chords) {
      if (!binding.run) continue;
      for (const event of [{ key: chord.key }, { key: chord.key, repeat: true, ctrlKey: true }]) {
        calls.length = 0;
        binding.run(event, target);
        binding.run(event, field);
        for (const [name] of calls) reached.add(name);
      }
    }
  }
  const words = keymap.map((binding) => (typeof binding.does === "function" ? binding.does() : binding.does)).join("\n");
  for (const [name] of calls.splice(0)) reached.add(name);
  const expected = ["bookmarkFromKeyboard", "bookmarkJumpFromKeyboard", "collapsedLinesFromKeyboard", "columnFromKeyboard",
    "columnsFromKeyboard", "composeFromKeyboard", "copyChordFromKeyboard", "copyFromKeyboard", "copyLinkFromKeyboard",
    "deepLinkFromKeyboard", "detailsFromKeyboard", "doneFromKeyboard", "filterFromKeyboard", "foldOneFromKeyboard", "foldStepFromKeyboard", "jumpList",
    "keyContext", "keysFromKeyboard", "listEdge", "mainRowsFromKeyboard", "menuFromKeyboard", "newestFromComposer",
    "nextUnreadFromKeyboard", "onEscape", "openFromKeyboard", "orderFromKeyboard", "outlineFromKeyboard", "packFromKeyboard", "pickChannel",
    "placeFromKeyboard", "pressRowButton", "readCollapseFromKeyboard", "readFromKeyboard", "readModeFromKeyboard",
    "quoteFromKeyboard",
    "replyFromKeyboard", "searchFromAnywhere", "searchFromKeyboard", "settingsFromKeyboard", "stepChannel", "stepFromKeyboard", "stepUnreadFromKeyboard",
    "threadFromKeyboard", "threadOutFromKeyboard", "threadStripFromKeyboard", "undoFromKeyboard", "unpackFromKeyboard",
    "viewFromKeyboard", "voiceFromKeyboard", "writeFromKeyboard"];
  const wording = ["afterDone", "chatServiceName", "manyChatServices", "maxCollapsedLines", "minCollapsedLines", "modKeyName", "standalone",
    "threadOrderWords"];
  assert.deepStrictEqual([...reached].sort(), [...expected, ...wording].sort(), "an act is reached by no key, or a key asks for one the page does not give");
  // What the list says asks the page for the reader's words.
  assert.ok(words.includes("Cmd+[ goes back") && words.includes("Copy its link in Chat:") && words.includes("in turn: Newest, Oldest.") &&
    words.includes("everywhere: 0 to 8 lines"), "the list of keys does not say the chords, the service, the orders or the bounds from the page");
  // `#1314 unread-step-keys`. The shortcuts help lists the unread steps.
  assert.ok(words.includes("The next unread message, skipping read ones") && words.includes("The previous unread message, skipping read ones"),
    "the list of keys does not say the Shift+arrow unread steps");
  answers.manyChatServices = true;
  assert.ok(keymap.map((binding) => (typeof binding.does === "function" ? binding.does() : "")).join("\n").includes("Copy its link in its chat service:"));
  // `#1321 after-done-setting`. The list of keys words e by the After Done choice, and never
  // promises a jump the choice does not make.
  const eWords = () => keymap.find((binding) => binding.rowControls && binding.rowControls.includes("done-button")).does();
  answers.afterDone = "stay";
  assert.ok(eWords().includes("Marked Done, the selection stays on it.") && !eWords().includes("next unread") && !eWords().includes("next message below"),
    "under Stay the list of keys promises a jump");
  answers.afterDone = "next";
  assert.ok(eWords().includes("Marked Done, on to the next message below.") && !eWords().includes("next unread"), "under Next message the list of keys does not say so");
  answers.afterDone = "unread";
  assert.ok(eWords().includes("Marked Done, on to the next unread below."), "under Next unread the list of keys does not say so");
});

test("`#624 voice-page-modularity`, `#892 voice-page-optimistic-done`: Shift+E folds first, marks Done only what is not Done and says so at once, waits for the Done to be saved before asking the service, and says its answer only over its own words", async () => {
  const { createReadCollapse } = VIEW_SHELL;
  const log = [];
  const pane = { name: "channel" };
  const row = { id: "row" };
  const state = { act: { pane, row, took: false }, ids: ["3", "4"], channel: "c1", done: new Set(), saved: true, folded: true, moveTo: null, fail: null, line: { text: "", up: false }, sayOver: null };
  const quiet = [];
  const shiftE = createReadCollapse({
    rowForAct: (target) => { log.push(`row for ${target.id}`); return state.act; },
    idsOf: (of) => {
      if (of === null) log.push("ids of nothing");
      assert.equal(of, state.act.row, "the ids were asked of another row");
      return state.ids;
    },
    runQuietly: (act) => { quiet.push(act().catch((error) => log.push(`error ${error.message}`))); },
    channelId: () => { log.push("channel?"); return state.channel; },
    foldForRead: (on, folding) => { log.push(`fold ${on.name} ${folding.id}`); return state.folded; },
    isDone: (id) => state.done.has(id),
    // Marked here at once, as the page's does; only the write is awaited.
    markRowDone: (ids, of) => { log.push(`done ${ids.join(",")} ${of.id}`); return state.fail ? Promise.reject(new Error(state.fail)) : Promise.resolve(); },
    doneSaved: async (ids) => { log.push(`saved? ${ids.join(",")}`); return state.saved; },
    markReadInService: async (channel, id) => {
      log.push(`service ${channel} ${id}`);
      if (state.moveTo) state.channel = state.moveTo;
      if (state.sayOver) state.line = { text: state.sayOver, up: true };
      return { id };
    },
    doneReadWords: (mark, also) => `already Done, read ${mark.id}${also}`,
    serviceReadWords: (mark, also) => `marked read ${mark.id}${also}`,
    localReadWords: (wasDone, also) => `${wasDone ? "already" : "marked"} here${also}`,
    setStatus: (text) => { log.push(`say ${text}`); state.line = { text, up: true }; },
    statusSays: (text) => state.line.up && state.line.text === text,
    dismissStatus: () => { log.push("unsay"); state.line = { ...state.line, up: false }; },
  });
  const press = async () => { log.length = 0; quiet.length = 0; const took = shiftE.readCollapseFromKeyboard({ id: "t" }); await Promise.all(quiet); return [took, ...log]; };
  // Said at once, before anything is awaited: `press` would show "say" after "saved?" otherwise.
  assert.deepStrictEqual(await press(), [true, "row for t", "channel?", "fold channel row", "done 3,4 row", "say marked here, and collapsed",
    "saved? 3,4", "service c1 4", "channel?", "say marked read 4, and collapsed"]);
  state.done = new Set(["3", "4"]);
  state.folded = false;
  assert.deepStrictEqual(await press(), [true, "row for t", "channel?", "fold channel row", "say already here", "saved? 3,4", "service c1 4", "channel?", "say already Done, read 4"],
    "a message Done already was marked again, or its service half was not asked");
  state.done = new Set(["3"]);
  state.saved = false;
  assert.deepStrictEqual(await press(), [true, "row for t", "channel?", "fold channel row", "done 3,4 row", "say marked here", "saved? 3,4", "unsay"],
    "the service was asked for a Done not saved, or the read said at once stayed said");
  state.saved = true;
  state.fail = "refused";
  assert.deepStrictEqual(await press(), [true, "row for t", "channel?", "fold channel row", "done 3,4 row", "say marked here", "unsay", "error refused"],
    "a Done that failed asked the service, was not said, or left the read said");
  state.fail = null;
  state.moveTo = "c2";
  assert.deepStrictEqual(await press(), [true, "row for t", "channel?", "fold channel row", "done 3,4 row", "say marked here", "saved? 3,4", "service c1 4", "channel?"], "the status line spoke over another channel");
  state.moveTo = null;
  state.channel = "c1";
  state.sayOver = "something else";
  assert.deepStrictEqual(await press(), [true, "row for t", "channel?", "fold channel row", "done 3,4 row", "say marked here", "saved? 3,4", "service c1 4", "channel?"],
    "the service's answer was said over what was said since");
  state.sayOver = null;
  state.ids = [];
  assert.deepStrictEqual(await press(), [false, "row for t"], "a row of no message was acted on");
  state.ids = ["5"];
  state.act = { pane, row: null, took: false };
  assert.deepStrictEqual(await press(), [false, "row for t"], "a key with no row acted, or asked a row's ids of nothing");
  state.ids = [];
  state.act = { pane: null, row: null, took: true };
  assert.deepStrictEqual(await press(), [true, "row for t"], "a key taken with no row to act on was given back");
  log.length = 0;
  state.ids = ["9"];
  await shiftE.readAndCollapse(pane, row, ["9"]);
  assert.deepStrictEqual(log, ["channel?", "fold channel row", "done 9 row", "say marked here", "saved? 9", "service c1 9", "channel?", "say marked read 9"]);
});

test("`#624 voice-page-modularity`: the keys' and the selection's listeners mount in the page's order, the switch restored before the keys are said, and unmount every one", () => {
  const { mountKeyListeners } = VIEW_SHELL;
  const log = [];
  const target = (name) => {
    const listeners = [];
    return { name, listeners, checked: false,
      addEventListener: (type, listener, options) => { listeners.push({ type, listener, options }); log.push(`listen ${name} ${type}${options && options.capture ? " capture" : ""}`); },
      removeEventListener: (type, listener, options) => {
        const at = listeners.findIndex((one) => one.type === type && one.listener === listener && Boolean(one.options && one.options.capture) === Boolean(options && options.capture));
        if (at >= 0) listeners.splice(at, 1);
      } };
  };
  const doc = target("document");
  const win = target("window");
  const box = target("single-keys");
  const panes = [{ name: "channel", area: target("channel-scroll") }, { name: "side", area: target("side-scroll") }].map((pane) => ({ ...pane, scroller: () => pane.area }));
  const fire = (on, type, event = {}, capture = false) => {
    for (const one of on.listeners.filter((each) => each.type === type && Boolean(each.options && each.options.capture) === capture)) one.listener(event);
  };
  const was = [globalThis.document, globalThis.window];
  globalThis.document = doc;
  globalThis.window = win;
  try {
    const mounted = mountKeyListeners({
      onPageKey: (event) => log.push(`page key ${event.key}`),
      onEverywhereKey: (event) => log.push(`everywhere key ${event.key}`),
      onPageBeforeCopy: () => log.push("before copy"),
      onPageCopy: () => log.push("copy"),
      restoreSingleKeys: () => log.push("restore single keys"),
      renderKeyHints: () => log.push("say the keys"),
      applySingleKeys: (on) => log.push(`single keys ${on}`),
      singleKeysBox: () => box,
      keyPanes: () => panes,
      noteClickedRow: (pane, node) => log.push(`click ${pane.name} ${node}`),
      onPaneFocus: (pane, event) => log.push(`focus ${pane.name} ${event.target}`),
      onPointerMove: () => log.push("pointer move"),
      onPointerDown: () => log.push("pointer down"),
    });
    assert.deepStrictEqual(log, ["listen document keydown", "listen window keydown capture", "listen document beforecopy", "listen document copy",
      "restore single keys", "say the keys", "listen single-keys change", "listen channel-scroll click", "listen channel-scroll focusin",
      "listen side-scroll click", "listen side-scroll focusin", "listen document pointermove", "listen document pointerdown"],
      "the listeners were not mounted in the page's order, or the keys were said before the switch was restored");
    log.length = 0;
    fire(doc, "keydown", { key: "j" });
    fire(win, "keydown", { key: "/" }, true);
    fire(doc, "beforecopy");
    fire(doc, "copy");
    box.checked = true;
    fire(box, "change");
    fire(panes[1].area, "click", { target: "row" });
    fire(panes[0].area, "focusin", { target: "button" });
    fire(panes[0].area, "click", null);
    fire(doc, "pointermove");
    fire(doc, "pointerdown");
    assert.deepStrictEqual(log, ["page key j", "everywhere key /", "before copy", "copy", "single keys true", "click side row", "focus channel button",
      "click channel null", "pointer move", "pointer down"]);
    mounted.unmount();
    assert.deepStrictEqual([doc, win, box, panes[0].area, panes[1].area].map((one) => one.listeners.length), [0, 0, 0, 0, 0], "a listener was left mounted");
    mounted.unmount();
  } finally {
    [globalThis.document, globalThis.window] = was;
  }
});

test("`#624 voice-page-modularity`: the page's listeners word a time, say its day ahead of the row, and write the place down before the turn when the page hides or goes, in the page's order, and unmount every one", () => {
  const { mountPageListeners } = VIEW_SHELL;
  const log = [];
  const target = (name) => {
    const listeners = [];
    return { name, listeners,
      addEventListener: (type, listener, options) => { listeners.push({ type, listener, options }); log.push(`listen ${name} ${type}${options && options.capture ? " capture" : ""}`); },
      removeEventListener: (type, listener, options) => {
        const at = listeners.findIndex((one) => one.type === type && one.listener === listener && Boolean(one.options && one.options.capture) === Boolean(options && options.capture));
        if (at >= 0) listeners.splice(at, 1);
      } };
  };
  const doc = target("document");
  const win = target("window");
  const fire = (on, type, event = {}, capture = false) => {
    for (const one of on.listeners.filter((each) => each.type === type && Boolean(each.options && each.options.capture) === capture)) one.listener(event);
  };
  let hidden = false;
  const was = [globalThis.document, globalThis.window];
  globalThis.document = doc;
  globalThis.window = win;
  try {
    const mounted = mountPageListeners({
      wordDayTitle: (event) => log.push(`word ${event.target}`),
      onTimeTap: (event) => log.push(`tap ${event.target}`),
      onTimeKey: (event) => log.push(`key ${event.key}`),
      hidden: () => hidden,
      rememberUiState: () => log.push("remember the place"),
      saveUnsettledTurns: () => log.push("save the turn"),
    });
    assert.deepStrictEqual(log, ["listen document pointerover", "listen document focusin", "listen document click capture", "listen document keydown capture",
      "listen document visibilitychange", "listen window pagehide", "listen document visibilitychange", "listen window pagehide"],
      "the listeners were not mounted in the page's order");
    log.length = 0;
    fire(doc, "pointerover", { target: "time" });
    fire(doc, "focusin", { target: "edited" });
    fire(doc, "click", { target: "time" }, true);
    fire(doc, "keydown", { key: "Enter" }, true);
    // Shown again: nothing is written down.
    fire(doc, "visibilitychange");
    hidden = true;
    fire(doc, "visibilitychange");
    fire(win, "pagehide");
    assert.deepStrictEqual(log, ["word time", "word edited", "tap time", "key Enter", "remember the place", "save the turn", "remember the place", "save the turn"]);
    mounted.unmount();
    assert.deepStrictEqual([doc, win].map((one) => one.listeners.length), [0, 0], "a listener was left mounted");
  } finally {
    [globalThis.document, globalThis.window] = was;
  }
});

test("`#624 voice-page-modularity`: the tray's and the menu's openers toggle each, then state at load what they show, in the page's order, and come down again", () => {
  const { mountMenuOpeners } = VIEW_SHELL;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const said = (name) => (...args) => log.push(`${name}${args.length ? ` ${args.length}` : ""}`);
  const openers = mountMenuOpeners({
    lookup: (id) => target(id),
    togglePrompts: said("toggle prompts"), toggleActionMenu: said("toggle menu"), shutActionMenu: said("shut menu"),
    renderReaderRow: said("draw reader row"), renderAutoplay: said("draw autoplay"), shutPrompts: said("shut prompts"),
  });
  assert.deepStrictEqual(log, ["listen prompts-open click", "listen action-menu-open click", "shut menu", "draw reader row", "draw autoplay", "shut prompts"],
    "the openers were not mounted before what they show was stated, in the page's order");
  log.length = 0;
  for (const id of ["prompts-open", "action-menu-open"]) nodes[id].listeners.forEach((one) => one.listener({ id }));
  assert.deepStrictEqual(log, ["toggle prompts", "toggle menu"], "an opener was handed the press, or reached the wrong toggle");
  openers.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: a mount's listening adds each listener with its options and takes every one down once", () => {
  const { createListening } = VIEW_DOM;
  const calls = [];
  const target = (name) => ({
    addEventListener: (type, listener, options) => calls.push(["add", name, type, listener, options]),
    removeEventListener: (type, listener, options) => calls.push(["remove", name, type, listener, options]),
  });
  const a = target("a");
  const b = target("b");
  const heard = [];
  const listening = createListening();
  const first = (event) => heard.push(event);
  const second = () => heard.push("second");
  listening.listen(a, "click", first);
  listening.listen(b, "keydown", second, { capture: true });
  assert.deepStrictEqual(calls.map(([kind, name, type, , options]) => [kind, name, type, options]),
    [["add", "a", "click", undefined], ["add", "b", "keydown", { capture: true }]]);
  calls[0][3]("event");
  assert.deepStrictEqual(heard, ["event"], "the listener added is not the one given");
  listening.unmount();
  assert.deepStrictEqual(calls.slice(2).map(([kind, name, type, listener, options]) => [kind, name, type, listener, options]),
    [["remove", "a", "click", first, undefined], ["remove", "b", "keydown", second, { capture: true }]],
    "a listener was not taken down, or taken down with other options than it was added with");
  listening.unmount();
  assert.equal(calls.length, 4, "a second unmount took listeners down again");
});

test("`#892 voice-page-optimistic-done`: key timing runs every key, times it only when asked for, from the browser's own stamp, and reports once painted", () => {
  const { createKeyTiming, chordOf, keyTimingLine } = VIEW_SHELL;
  let on = false;
  let clock = 100;
  const painted = [];
  const reports = [];
  const timing = createKeyTiming({
    enabled: () => on,
    now: () => clock,
    afterPaint: (act) => painted.push(act),
    report: (entry, start) => reports.push([keyTimingLine(entry), start]),
  });
  const ran = [];
  timing.around({ key: "E", shiftKey: true, timeStamp: 90 }, () => { ran.push("off"); clock += 5; });
  assert.deepStrictEqual([ran, painted.length, timing.recent()], [["off"], 0, []], "a key was not run, or was timed while timing is off");
  on = true;
  timing.around({ key: "E", shiftKey: true, timeStamp: 100 }, () => { ran.push("on"); clock += 12; });
  assert.deepStrictEqual(ran, ["off", "on"]);
  assert.deepStrictEqual(timing.recent().map((one) => [one.chord, one.handler, one.paint]), [["Shift+E", 17, null]],
    "the handler was not timed from the browser's stamp");
  assert.deepStrictEqual(reports, [], "a key was reported before its frame was painted");
  clock = 140;
  painted.shift()();
  assert.deepStrictEqual(reports, [["Shift+E: handler 17.0 ms, paint 40.0 ms", 100]]);
  // A key that throws is timed all the same, and the throw is the page's.
  assert.throws(() => timing.around({ key: "e", timeStamp: 0 }, () => { throw new Error("slip"); }), /slip/);
  assert.equal(timing.recent().at(-1).chord, "e");
  timing.around({ key: "Shift", shiftKey: true }, () => ran.push("shift"));
  assert.notEqual(timing.recent().at(-1).chord, "Shift+Shift", "a modifier pressed on its own was timed as a key");
  assert.equal(ran.at(-1), "shift", "a modifier pressed on its own was not run");
  for (let i = 0; i < 150; i += 1) timing.around({ key: "j" }, () => {});
  assert.equal(timing.recent().length, 100, "every key timed was kept");
  assert.deepStrictEqual([chordOf({ key: "?", shiftKey: true }), chordOf({ key: "k", ctrlKey: true }),
    chordOf({ key: "ArrowDown", shiftKey: true }), chordOf({ key: "l", metaKey: true, shiftKey: false })],
  ["?", "Ctrl+k", "Shift+ArrowDown", "Cmd+l"]);
});

test("`#624 voice-page-modularity`: the page's last document listeners are mounted in the page's order, each answering its own act, then all come down", () => {
  const { mountDocumentListeners } = VIEW_SHELL;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({ type }));
  const act = (name) => (...args) => { log.push(`${name} ${args.length}`); return name; };
  const was = globalThis.document;
  globalThis.document = target("document");
  try {
    const mounted = mountDocumentListeners({
      onVisibility: act("visibility"), closeRowMenuOutside: act("menu"),
    });
    assert.deepStrictEqual(log, ["listen document visibilitychange", "listen document pointerdown"]);
    log.length = 0;
    fire("document", "visibilitychange"); fire("document", "pointerdown");
    assert.deepStrictEqual(log, ["visibility 1", "menu 1"]);
    mounted.unmount();
    assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
  } finally {
    globalThis.document = was;
  }
});


test("`#624 voice-page-modularity`: showing a screen hides the others first, shows it last, and does what goes with the change in order", () => {
  const { createScreens, createScreenShowing, SCREEN_TITLES } = VIEW_SHELL;
  assert.deepStrictEqual({ ...SCREEN_TITLES }, { settings: "Settings", help: "Help" });
  const el = elements();
  const screens = createScreens();
  const log = [];
  const showing = createScreenShowing({
    screens, el,
    closeOverscroll: () => log.push("closeOverscroll"),
    holdChannelPlace: () => log.push("holdChannelPlace"),
    postCardBackToMain: () => log.push("postCardBackToMain"),
    placePostCard: () => log.push("placePostCard"),
    disarmClear: () => log.push("disarmClear"),
    browserReading: () => true,
    stopReading: () => log.push("stopReading"),
    closeDismissedSends: () => log.push("closeDismissedSends"),
    renderDismissedSends: () => log.push("renderDismissedSends"),
    renderJumpNewest: () => log.push("renderJumpNewest"),
    placeChipsOverComposer: () => log.push("placeChipsOverComposer"),
    renderControlBar: () => log.push("renderControlBar"),
    renderColumns: () => log.push("renderColumns"),
    pointSettingsAtReading: () => log.push("pointSettingsAtReading"),
    refreshUpstreamClear: () => log.push("refreshUpstreamClear"),
    refreshChannelHistory: () => log.push("refreshChannelHistory"),
    refreshAgentPrefix: () => log.push("refreshAgentPrefix"),
    renderIdentities: () => log.push("renderIdentities"),
    focusLost: () => false,
    settleFocus: () => log.push("settleFocus"),
    scheduleBackSync: () => log.push("scheduleBackSync"),
  });
  showing.showScreen("settings");
  assert.deepStrictEqual(log, ["closeOverscroll", "stopReading", "disarmClear",
    "renderJumpNewest", "placeChipsOverComposer", "renderControlBar", "placePostCard", "pointSettingsAtReading",
    "refreshUpstreamClear", "refreshChannelHistory", "refreshAgentPrefix", "renderDismissedSends",
    "renderIdentities", "settleFocus", "scheduleBackSync"], "leaving sign-in for Settings, in order");
  assert.deepStrictEqual([el("screen-signin").hidden, el("screen-settings").hidden, el("screen-main").hidden,
    el("screen-help").hidden], [true, false, true, true]);
  assert.deepStrictEqual([el("close-settings").hidden, el("close-help").hidden, el("topbar-title").hidden,
    el("topbar-title").textContent, el("control-pane").hidden, screens.screen()],
    [false, true, false, "Settings", true, "settings"]);
  assert.equal(el("frame-body").getAttribute("data-screen"), "settings");
  log.length = 0;
  showing.showScreen("main");
  assert.deepStrictEqual(log, ["postCardBackToMain", "closeDismissedSends",
    "placeChipsOverComposer", "renderControlBar", "renderColumns", "placePostCard", "renderJumpNewest",
    "settleFocus", "scheduleBackSync"], "back to the main screen: card back before it is shown, jump decided last");
  assert.deepStrictEqual([el("screen-main").hidden, el("control-pane").hidden, el("topbar-title").hidden],
    [false, false, true]);
  log.length = 0;
  showing.showScreen("main");
  assert.deepStrictEqual(log, ["closeDismissedSends", "placeChipsOverComposer", "renderControlBar",
    "renderColumns", "placePostCard", "renderJumpNewest", "scheduleBackSync"],
    "the screen already up: no focus settled again, nothing noted or put back");
});

test("`#624 voice-page-modularity`: the control bar shows each pack member only on its view, and the bar itself only while a member shows", () => {
  const { createControlBar, PACK_VIEWS } = VIEW_SHELL;
  assert.deepStrictEqual(PACK_VIEWS["discord-channel"], ["discord"]);
  const els = new Map();
  const el = (id) => {
    if (!els.has(id)) els.set(id, { id, hidden: false, children: [] });
    return els.get(id);
  };
  for (const id of ["view-switch", "action-menu-open", "reader-toggle", "autoplay-toggle", "open-settings", "search-toggle", "search-field", "search-count", "pinned-filter", "links-filter", "author-filter", "control-bar", "topbar", "close-settings", "topbar-title", "bar-pack", "prompts-open"]) el(id);
  el("bar-pack").children = [el("discord-channel"), el("prompts-open")];
  let view = "discord";
  const log = [];
  const bar = createControlBar({
    el,
    screen: () => "main",
    view: () => view,
    actionMenuOpen: () => false,
    renderActionMenu: () => {},
    screenHasTitle: () => false,
    readAloudAgent: () => ({}),
    columnsOffered: () => true,
    promptsOpen: () => false,
    setPromptsOpen: (open) => log.push(`prompts:${open}`),
    setActionMenuOpen: (open) => log.push(`menu:${open}`),
    searchOpen: () => false,
    renderLinkKinds: () => {},
    barWhere: () => "bottom",
  });
  bar.renderControlBar();
  assert.equal(el("discord-channel").hidden, false, "a channel member was hidden on the channel view");
  assert.equal(el("prompts-open").hidden, true, "a call member showed on the channel view");
  assert.equal(el("control-bar").hidden, false, "the bar hid while a member showed");
  view = "voice";
  bar.renderControlBar();
  assert.equal(el("discord-channel").hidden, true, "a channel member showed on the call view");
  assert.equal(el("prompts-open").hidden, false, "a call member was hidden on the call view");
});

test("`#624 voice-page-modularity`: the page hidden pauses and notes a call; back briefly resumes at rest, back after long away re-reads and resumes fresh", async () => {
  const { createPageVisibility } = VIEW_SHELL;
  const log = [];
  let hidden = true;
  let away = 0;
  const visibility = createPageVisibility({
    syncScreenWakeLock: () => log.push("wakeLock"),
    hidden: () => hidden,
    hide: (at) => log.push(`hide ${at}`),
    callSocketOpen: () => true,
    noteHiddenDuringCall: () => log.push("hiddenDuringCall"),
    pauseWhileHidden: () => log.push("pause"),
    noteVisibleAt: (at) => log.push(`visibleAt ${at}`),
    awayMs: () => away,
    resumeOutgoingRetries: () => log.push("retries"),
    catchUpHiddenArrivals: () => null,
    replyArrowsOwed: () => false,
    measureReplyArrows: () => log.push("arrows"),
    inOtherColumn: (act) => act(),
    takeStreamResume: () => null,
    pollPaused: () => false,
    pollDueAt: () => 0,
    visibleRefreshMs: 60000,
    view: () => "discord",
    threading: () => true,
    configApplied: () => true,
    selectedChannelId: () => "c1",
    refreshQuietly: (run) => () => { log.push("quiet"); void run(); },
    refreshAfterLiveMutation: () => log.push("mutation"),
    replayStarted: () => "since",
    loadDiscordVisible: async () => { log.push("loadDiscord"); },
    readSideColumn: async (reason, since) => { log.push(`side ${reason} ${since}`); },
    resumeLiveStream: () => log.push("resumeStream"),
    resumePolling: (mode) => log.push(`poll ${mode}`),
    now: () => 1000,
  });
  visibility.onVisibility();
  assert.deepStrictEqual(log, ["wakeLock", "hide 1000", "hiddenDuringCall", "pause"]);
  log.length = 0;
  hidden = false;
  visibility.onVisibility();
  assert.deepStrictEqual(log, ["wakeLock", "visibleAt 1000", "retries", "resumeStream", "poll rest"]);
  log.length = 0;
  away = 120000;
  visibility.onVisibility();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepStrictEqual(log, ["wakeLock", "visibleAt 1000", "retries", "quiet", "loadDiscord", "poll fresh", "side visible since", "resumeStream"]);
});
