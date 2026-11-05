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
