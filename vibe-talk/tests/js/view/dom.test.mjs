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
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: n goes to the next unread row below the selection or the screen's top, and e marks Done and moves on without taking the focus from a box", () => {
  const { createDoneKeys } = VIEW_SHELL;
  const log = [];
  const rowOf = (id, attributes = {}) => ({ id, attributes, getAttribute(name) { return name === "data-id" ? id : this.attributes[name] ?? null; }, hasAttribute(name) { return name in this.attributes; } });
  let rows = [rowOf("1"), rowOf("2"), rowOf("3"), rowOf("c4", { "data-card": "" }), rowOf("5")];
  const state = { read: new Set(["1", "2", "3"]), done: new Set(), inView: new Set(["2", "3", "c4", "5"]), row: null, took: true, again: true };
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
