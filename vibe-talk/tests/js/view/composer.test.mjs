// Tests for web/src/view/composer/ on its own: the module built as the page build builds it and
// held directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over
// the same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The message boxes' keys and growth (web/src/view/composer). */
const VIEW_COMPOSER = await moduleForTests("src/view/composer/index.ts");

test("`#624 voice-page-modularity`: what a key does in a message box, and a paste's own new line, from the module alone", () => {
  const { createComposerKeys, PASTE_ENTER_MS } = VIEW_COMPOSER;
  let now = 1000;
  let enter = true;
  const keys = createComposerKeys({ now: () => now, enterSends: () => enter });
  const box = {};
  let prevented = 0;
  const key = (over = {}) => ({ key: "Enter", preventDefault: () => { prevented += 1; }, ...over });
  const send = () => "sent";
  assert.equal(keys.onKey(key(), send, box), "sent");
  assert.equal(keys.onKey(key({ shiftKey: true }), send, box), undefined, "Shift+Enter sent");
  assert.equal(keys.onKey(key({ isComposing: true }), send, box), undefined, "an Enter accepting a composed word sent");
  assert.equal(keys.onKey(key({ keyCode: 229 }), send, box), undefined);
  assert.equal(keys.onKey(key({ repeat: true }), send, box), undefined, "a held key's repeat sent again");
  assert.equal(keys.onKey(key({ key: "a" }), send, box), undefined);
  enter = false;
  assert.equal(keys.onKey(key(), send, box), undefined, "a bare Enter sent with Enter to send off");
  assert.equal(keys.onKey(key({ metaKey: true }), send, box), "sent", "the chord did not send with the setting off");
  // An Enter within PASTE_ENTER_MS of a paste or a run of text in the same box is that paste's new line.
  enter = true;
  keys.noteInsertion(box, { type: "paste" });
  assert.equal(keys.onKey(key(), send, box), undefined, "a paste's own new line sent it");
  assert.equal(keys.onKey(key(), send, {}), "sent", "a paste in one box held another's Enter");
  now += PASTE_ENTER_MS + 1;
  assert.equal(keys.onKey(key(), send, box), "sent");
  keys.noteInsertion(box, { inputType: "insertText", data: "x" });
  assert.equal(keys.insertedJustNow(box), false, "one typed character counted as a paste");
  keys.noteInsertion(box, { inputType: "insertText", data: "two words" });
  assert.equal(keys.insertedJustNow(box), true);
  keys.noteInsertion(box, { inputType: "insertText", data: "ab", isComposing: true });
  assert.ok(prevented > 0);
});

test("`#624 voice-page-modularity`: the boxes that grow, on stand-in elements, from the module alone", () => {
  const { createComposerGrow, COMPOSER_GROW_BOXES, COMPOSER_GROW_IDS } = VIEW_COMPOSER;
  const made = new Map();
  const element = (id) => {
    if (!made.has(id)) {
      const attributes = new Set();
      made.set(id, { id, checked: false, scrollTop: 0, clientHeight: 400, drawn: true, removed: [],
        setAttribute: (name) => attributes.add(name), removeAttribute: (name) => attributes.delete(name),
        hasAttribute: (name) => attributes.has(name), getClientRects() { return this.drawn ? [{}] : []; },
        getBoundingClientRect: () => ({ top: 0, bottom: 20, height: 20 }),
        style: { removeProperty(name) { made.get(id).removed.push(name); } } });
    }
    return made.get(id);
  };
  const record = { kept: [], followed: [], kept_in_view: [] };
  let focused = null;
  let atFoot = false;
  const grow = createComposerGrow({
    lookup: element, listOf: (field) => element(field.id === "side-compose-text" ? "side-scroll" : "scroll-area"),
    atBottom: () => atFoot, followFoot: (area) => record.followed.push(area.id),
    keepBoxInView: (box, area) => record.kept_in_view.push([box.id, area.id]), activeElement: () => focused,
    bottomSlack: 24, fieldSizing: true, keep: (on) => { if (on === "refuse") throw new Error("refused"); record.kept.push(on); },
  });
  assert.equal(grow.on(), false, "the boxes grew before Grow was ticked");
  grow.render(true);
  assert.equal(element("screen-main").hasAttribute("data-composer-grow"), true);
  assert.deepStrictEqual(COMPOSER_GROW_IDS.map((id) => element(id).checked), [true, true, true, true], "a checkbox did not say the one choice");
  assert.deepStrictEqual(COMPOSER_GROW_BOXES.map((id) => element(id).removed), [["height"], ["height"], ["height"], ["height"]],
    "a dragged height was not let go as the boxes began to grow");
  assert.equal(grow.shown(element("channel-compose-text")), true);
  element("channel-compose-grow").drawn = false;
  assert.equal(grow.shown(element("channel-compose-text")), false, "a box grew where its checkbox is not drawn");
  // A list at its foot is put back there as Grow changes; storage refused still honours the choice.
  atFoot = true;
  grow.apply("refuse", element("compose-grow"));
  assert.deepStrictEqual(record.followed, ["scroll-area", "side-scroll"]);
  // Only the box with the focus is kept in view as it grows.
  grow.render(true);
  grow.keepInView(element("side-compose-text"));
  assert.deepStrictEqual(record.kept_in_view, [], "a box without the focus moved its list");
  focused = element("side-compose-text");
  atFoot = false;
  grow.keepInView(element("side-compose-text"));
  assert.deepStrictEqual(record.kept_in_view, [["side-composer", "side-scroll"]]);
});

test("`#624 voice-page-modularity`: a focused field with the visual viewport a keyboard short is a keyboard, and the stylesheet is told", () => {
  const { createOnScreenKeyboard, KEYBOARD_SETTLE_MS } = VIEW_COMPOSER;
  const attrs = new Map();
  const props = new Map();
  const root = {
    clientWidth: 400, clientHeight: 800,
    setAttribute: (name, value) => attrs.set(name, value), getAttribute: (name) => attrs.get(name) ?? null,
    style: { setProperty: (name, value) => props.set(name, value) },
  };
  const field = { tagName: "TEXTAREA", id: "channel-compose-text", type: "textarea", isContentEditable: false, getClientRects: () => [] };
  const els = {
    "read-speed-box": { id: "read-speed-box" }, "speed-popover": { hidden: true }, "keyboard-debug": { checked: false },
    "keyboard-debug-readout": { hidden: true, style: {} },
  };
  let focus = null;
  let viewport = { height: 800, scale: 1, offsetTop: 0 };
  const timers = [];
  const keyboard = createOnScreenKeyboard({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    lookup: (id) => els[id] || { id, getClientRects: () => [] },
    el: (id) => els[id],
    viewport: () => viewport, innerHeight: () => 800, scrollY: () => 0, root: () => root,
    activeElement: () => focus, virtualKeyboard: () => undefined, computedStyle: () => null, closeSpeedPopover: () => {},
  });
  keyboard.noteKeyboard();
  assert.equal(attrs.get("data-keyboard"), undefined, "nothing focused, nothing up");
  focus = field;
  viewport = { height: 450, scale: 1, offsetTop: 0 };
  keyboard.noteKeyboard();
  assert.equal(attrs.get("data-keyboard"), "open", "a field in focus and the viewport 350px short");
  assert.equal(timers.at(-1).ms, KEYBOARD_SETTLE_MS, "the jump's moment lasts the settle time");
  viewport = { height: 760, scale: 1, offsetTop: 0 };
  keyboard.noteKeyboard();
  assert.equal(attrs.get("data-keyboard"), "closed", "a toolbar's worth is no keyboard");
  keyboard.showKeyboardDebug(true);
  assert.equal(els["keyboard-debug"].checked, true);
  assert.equal(els["keyboard-debug-readout"].hidden, false);
  assert.match(els["keyboard-debug-readout"].textContent, /^keyboard closed {2}box channel-compose-text \(focus\)/);
  keyboard.showKeyboardDebug(false);
  assert.equal(els["keyboard-debug-readout"].hidden, true);
});

test("`#821 voice-page-read-aloud-react`: dictation writes unsure then settled words after what the box held, says it listens, and falls back to the keyboard's microphone", () => {
  const { createDictation, DICTATION_LISTENING, DICTATION_UNAVAILABLE } = VIEW_COMPOSER;
  const box = { value: "Yes,", getAttribute: () => null, setAttribute: () => {}, removeAttribute: () => {} };
  const said = [];
  const heard = [];
  let settled = 0;
  let made = null;
  class Fake {
    constructor() { made = this; this.started = false; this.aborted = false; }
    start() { this.started = true; }
    abort() { this.aborted = true; }
  }
  const deps = (recognition) => ({
    recognition: () => recognition, lang: () => "en-GB", box: () => box,
    say: (text) => said.push(text), listening: (on) => heard.push(on), settled: () => { settled += 1; },
  });
  const dictation = createDictation(deps(Fake));
  assert.equal(dictation.start(), true);
  assert.deepStrictEqual([made.started, made.lang, made.interimResults, made.continuous], [true, "en-GB", true, false]);
  assert.deepStrictEqual([said.at(-1), heard.at(-1), dictation.active()], [DICTATION_LISTENING, true, true]);
  const result = (words, isFinal) => ({ resultIndex: 0, results: [Object.assign([{ transcript: words }], { isFinal })] });
  made.onresult(result("the replica", false));
  assert.deepStrictEqual([box.value, settled], ["Yes, the replica", 0], "unsure words show, and are not kept yet");
  made.onresult(result("the replica is done", true));
  assert.deepStrictEqual([box.value, settled], ["Yes, the replica is done", 1], "settled words are kept as the draft");
  made.onend();
  assert.deepStrictEqual([heard.at(-1), said.at(-1), dictation.active()], [false, "", false]);
  // Refused (no microphone, no permission): the line says to use the keyboard's.
  dictation.start();
  made.onerror({ error: "not-allowed" });
  assert.deepStrictEqual([said.at(-1), dictation.active()], [DICTATION_UNAVAILABLE, false]);
  // Stopped as the box closes, mid-utterance: the recognition is let go, the words in the box kept, and what
  // it says after (its own "aborted") changes nothing.
  dictation.start();
  made.onresult(result("and the dashboards", false));
  dictation.stop();
  assert.deepStrictEqual([made.aborted, dictation.active(), box.value], [true, false, "Yes, the replica is done and the dashboards"]);
  const quiet = said.length;
  made.onerror({ error: "aborted" });
  made.onend();
  assert.equal(said.length, quiet, "a recognition already let go still spoke");
  // No recognition in this browser at all.
  const none = createDictation(deps(null));
  assert.equal(none.start(), false);
  assert.equal(said.at(-1), DICTATION_UNAVAILABLE);
});

test("`#829 voice-page-dictation-polish`: the keyboard stays down while dictation listens, and comes back on a tap, a stop, the end or a failure", () => {
  const { createDictation } = VIEW_COMPOSER;
  const attributes = new Map();
  const box = {
    value: "",
    getAttribute: (name) => (attributes.has(name) ? attributes.get(name) : null),
    setAttribute: (name, value) => attributes.set(name, String(value)),
    removeAttribute: (name) => attributes.delete(name),
  };
  let made = null;
  class Fake {
    constructor() { made = this; }
    start() {}
    abort() {}
  }
  const deps = (recognition) => ({
    recognition: () => recognition, lang: () => "en-GB", box: () => box, say: () => {}, listening: () => {}, settled: () => {},
  });
  const mode = () => box.getAttribute("inputmode");
  const dictation = createDictation(deps(Fake));
  // Before the box takes the focus, and through the listening that follows.
  dictation.quietKeyboard();
  assert.equal(mode(), "none", "the keyboard is not kept down before the box takes the focus");
  dictation.start();
  assert.equal(mode(), "none", "the keyboard comes up while dictation listens");
  dictation.showKeyboard();
  assert.deepStrictEqual([mode(), dictation.active()], [null, true], "a tap in the box did not bring the keyboard back alone");
  made.onend();
  dictation.start();
  assert.equal(mode(), "none");
  made.onend();
  assert.equal(mode(), null, "the keyboard stays down after the recognition ended");
  dictation.start();
  made.onerror({ error: "network" });
  assert.equal(mode(), null, "the keyboard stays down after the recognition failed");
  // A second reply opened while one listens: that listening stopped first, the keyboard is down before the
  // box takes the focus again.
  dictation.start();
  dictation.stop();
  dictation.quietKeyboard();
  assert.equal(mode(), "none", "a reply opened while one listened lets the keyboard up before its focus");
  dictation.start();
  dictation.stop();
  // The box's own mode is put back as it was.
  box.setAttribute("inputmode", "text");
  dictation.start();
  dictation.stop();
  assert.equal(mode(), "text", "stopping did not put the box's own inputmode back");
  // Where the browser cannot listen, the keyboard is never kept down: it is the way to speak.
  box.removeAttribute("inputmode");
  const none = createDictation(deps(null));
  none.quietKeyboard();
  assert.equal(mode(), null, "the keyboard is kept down where nothing can listen");
  // Kept down for a start that then fails: given back.
  class Refusing extends Fake { start() { throw new Error("not-allowed"); } }
  const refused = createDictation(deps(Refusing));
  refused.quietKeyboard();
  assert.equal(refused.start(), false);
  assert.equal(mode(), null, "a start that failed left the keyboard down");
});

test("`#624 voice-page-modularity`: the quoted words' and the quick-reply box's listeners mount in the page's order, reach what each names, and come down again", () => {
  const { mountQuickReplyListeners } = VIEW_COMPOSER;
  const log = [];
  const nodes = {};
  const target = (name, parentNode = null) => (nodes[name] ||= { name, parentNode, listeners: [],
    addEventListener(type, listener, options) {
      this.listeners.push({ type, listener, options });
      log.push(`listen ${name} ${type}${options && options.capture ? " capture" : ""}${options && options.passive ? " passive" : ""}`);
    },
    removeEventListener(type, listener, options) {
      this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener ||
        Boolean(one.options && one.options.capture) !== Boolean(options && options.capture));
    } });
  const fire = (name, type, event = {}) => nodes[name].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  const doc = target("document");
  const viewport = target("viewport");
  const event = (fields = {}) => ({ ...fields, preventDefault() { log.push(`prevent ${fields.key || fields.n}`); },
    stopPropagation() { log.push(`stop ${fields.key || fields.n}`); } });
  let open = true;
  let held = false;
  const was = globalThis.document;
  globalThis.document = doc;
  try {
    const mounted = mountQuickReplyListeners({
      byId: (id) => target(id),
      // The left column's own elements, kept apart from the page's: two columns can hold two of a name.
      leftById: (id) => target(`left:${id}`),
      noteQuoteSelection: (...args) => log.push(`note selection ${args.length}`),
      pressSelectionChip: (...args) => log.push(`chip ${args.length}`),
      sendQuickReply: (...args) => { log.push(`send ${args.length}`); return "sent"; },
      cancelQuickReply: () => log.push("cancel"),
      answerQuickReplyDiscard: (discard) => log.push(`discard ${discard}`),
      closeQuickReply: () => log.push("close"),
      onBoxKey: (key) => { log.push(`box key ${key.key}`); return "composer"; },
      watchBoxInsertions: (box) => log.push(`watch ${box.name}`),
      noteQuickReplyInput: () => log.push("input"),
      showKeyboard: () => log.push("keyboard"),
      stopDictation: () => log.push("stop dictation"),
      pressDraftChip: () => log.push("draft chip"),
      reactToHeldSentence: () => log.push("react"),
      quickReplyOpen: () => open,
      putDownQuickReply: () => log.push("put down"),
      quickReplyListMoved: (moved) => log.push(`list moved ${moved}`),
      visualViewport: () => viewport,
      placeQuickReply: () => log.push("place box"),
      selectionHeld: () => held,
      placeSelectionChip: () => log.push("place chip"),
    });
    assert.deepStrictEqual(log, ["listen document selectionchange",
      "listen left:selection-chip mousedown", "listen left:selection-chip click", "listen left:read-steer-quote mousedown", "listen left:read-steer-quote click",
      "listen quick-reply-send click", "listen quick-reply-cancel click", "listen quick-reply-discard-yes click",
      "listen quick-reply-discard-yes keydown", "listen quick-reply-discard-no keydown", "listen quick-reply-discard-no click",
      "listen quick-reply-text keydown", "watch quick-reply-text", "listen quick-reply-text input", "listen quick-reply-text pointerdown",
      "listen quick-reply-mic mousedown", "listen quick-reply-mic click", "listen left:draft-chip click", "listen read-react click",
      "listen document pointerdown capture", "listen document scroll capture passive", "listen viewport resize",
      "listen document scroll capture passive"], "a listener was added out of the page's order");
    log.length = 0;
    fire("document", "selectionchange", event({ n: 1 }));
    fire("left:selection-chip", "mousedown", event({ n: 2 }));
    fire("left:read-steer-quote", "click", event({ n: 3 }));
    fire("quick-reply-send", "click", event({ n: 4 }));
    fire("quick-reply-cancel", "click");
    fire("quick-reply-discard-yes", "click");
    fire("quick-reply-discard-no", "click");
    // Esc on the question closes the box and goes no further; another key, or Esc mid-composition, is not its.
    fire("quick-reply-discard-no", "keydown", event({ key: "Escape" }));
    fire("quick-reply-discard-yes", "keydown", event({ key: "Enter" }));
    fire("quick-reply-discard-yes", "keydown", event({ key: "Escape", isComposing: true }));
    assert.deepStrictEqual(fire("quick-reply-text", "keydown", event({ key: "Escape" })), [undefined]);
    assert.deepStrictEqual(fire("quick-reply-text", "keydown", event({ key: "Enter" })), ["composer"], "the box's key did not answer what the message boxes' keys answer");
    fire("quick-reply-text", "input");
    fire("quick-reply-text", "pointerdown");
    fire("quick-reply-mic", "mousedown", event({ n: 5 }));
    fire("quick-reply-mic", "click");
    fire("left:draft-chip", "click");
    fire("read-react", "click");
    assert.deepStrictEqual(log, ["note selection 0", "prevent 2", "chip 0", "send 0", "cancel", "discard true", "discard false",
      "prevent Escape", "stop Escape", "close", "prevent Escape", "stop Escape", "close", "box key Enter", "input", "keyboard",
      "prevent 5", "stop dictation", "draft chip", "react"]);
    log.length = 0;
    // A tap outside puts the box down; one inside it or on any chip that opens it does not, nor any tap while it is
    // shut. The chips are the left column's, React's included, though its press is heard on the page's own.
    const inside = target("send-label", target("quick-reply"));
    fire("document", "pointerdown", { target: inside });
    for (const chip of ["selection-chip", "read-steer-quote", "draft-chip", "read-react"]) {
      fire("document", "pointerdown", { target: target(`${chip}-label`, target(`left:${chip}`)) });
    }
    assert.deepStrictEqual(log, [], "a tap in the box or on a chip put it down");
    fire("document", "pointerdown", { target: nodes["read-react"] });
    fire("document", "pointerdown", { target: target("elsewhere") });
    open = false;
    fire("document", "pointerdown", { target: target("elsewhere") });
    fire("document", "scroll", { target: "channel" });
    fire("viewport", "resize");
    held = true;
    fire("document", "scroll", { target: "side" });
    assert.deepStrictEqual(log, ["put down", "put down", "list moved channel", "place box", "list moved side", "place chip"]);
    mounted.unmount();
    assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
  } finally {
    globalThis.document = was;
  }
});

test("`#624 voice-page-modularity`: the grow switches are drawn as kept, heard from the box that changed, three boxes fitted as typed in, and every box observed to keep Send in view, then all come down", () => {
  const { mountComposerGrowListeners, COMPOSER_GROW_IDS, COMPOSER_GROW_BOXES } = VIEW_COMPOSER;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, checked: false, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).forEach((one) => one.listener({ type }));
  const observers = [];
  class Observer {
    constructor(callback) { this.callback = callback; this.watched = []; observers.push(this); log.push("observer"); }
    observe(node) { this.watched.push(node); log.push(`observe ${node.id}`); }
    disconnect() { this.watched = []; log.push("disconnect"); }
  }
  const mounted = mountComposerGrowListeners({
    lookup: (id) => target(id),
    storedComposerGrow: () => { log.push("stored"); return true; },
    renderComposerGrow: (on) => log.push(`draw ${on}`),
    applyComposerGrow: (on, grow) => log.push(`apply ${on} ${grow.id}`),
    fitGrowingBox: (field) => log.push(`fit ${field.id}`),
    keepGrowingBoxInView: (field) => log.push(`keep ${field.id}`),
    resizeObserver: () => Observer,
  });
  assert.deepStrictEqual(log, ["stored", "draw true", ...COMPOSER_GROW_IDS.map((id) => `listen ${id} change`),
    "listen channel-compose-text input", "listen side-compose-text input", "listen reply-text input",
    "observer", ...COMPOSER_GROW_BOXES.map((id) => `observe ${id}`)], "the switches were not drawn first, or a box was missed");
  log.length = 0;
  nodes["side-compose-grow"].checked = true;
  fire("side-compose-grow", "change");
  fire("reply-grow", "change");
  fire("reply-text", "input");
  observers[0].callback([{ target: nodes["compose-text"] }, { target: nodes["side-compose-text"] }]);
  assert.deepStrictEqual(log, ["apply true side-compose-grow", "apply false reply-grow", "fit reply-text", "keep compose-text", "keep side-compose-text"]);
  log.length = 0;
  mounted.unmount();
  assert.deepStrictEqual(log, ["disconnect"]);
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: with no ResizeObserver the grow switches and the typed boxes are still heard", () => {
  const { mountComposerGrowListeners } = VIEW_COMPOSER;
  const heard = [];
  const node = (id) => ({ id, addEventListener: (type) => heard.push(`${id} ${type}`), removeEventListener() {} });
  const nothing = () => undefined;
  const mounted = mountComposerGrowListeners({ lookup: node, storedComposerGrow: () => false, renderComposerGrow: nothing,
    applyComposerGrow: nothing, fitGrowingBox: nothing, keepGrowingBoxInView: nothing, resizeObserver: () => undefined });
  assert.equal(heard.length, 7);
  mounted.unmount();
});
