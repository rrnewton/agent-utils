// The call owners can be constructed before the rest of the page is initialized. Their effects
// belong to explicit actions and initialization seams, not construction (#624 voice-page-modularity).

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

const VIEW_VOICE = await moduleForTests("src/view/voice/index.ts");

for (const name of ["createVoiceCall", "createVoiceTranscript", "createReadAloud"]) {
  test(`\`#624 voice-page-modularity\`: ${name} waits for explicit actions before reading dependencies or starting effects`, () => {
    const attempts = [];
    const refuse = (what) => () => {
      attempts.push(what);
      throw new Error(`construction reached ${what}`);
    };
    const unreadable = (what) => new Proxy({}, {
      get(_target, key) { return refuse(`${what}.${String(key)}`)(); },
    });
    const replacements = [
      [globalThis, "document", unreadable("document")],
      [globalThis, "window", unreadable("window")],
      [globalThis, "setTimeout", refuse("setTimeout")],
      [globalThis, "setInterval", refuse("setInterval")],
      [globalThis, "requestAnimationFrame", refuse("requestAnimationFrame")],
      [globalThis, "fetch", refuse("fetch")],
      [Date, "now", refuse("Date.now")],
      [Math, "random", refuse("Math.random")],
    ];
    const saved = replacements.map(([object, key]) => Object.getOwnPropertyDescriptor(object, key));
    try {
      for (const [object, key, value] of replacements) {
        Object.defineProperty(object, key, { configurable: true, writable: true, value });
      }
      const owner = VIEW_VOICE[name](unreadable("dependencies"));
      for (const key of Object.keys(owner)) {
        const held = owner[key];
        if (typeof held === "function") {
          assert.equal(owner[key], held, `${name}.${key} changed callback identity`);
        }
      }
      // An owner catching and hiding an eager failure is still an owner that ran too early.
      assert.deepEqual(attempts, [], "construction attempted an effect before its explicit action");
    } finally {
      replacements.forEach(([object, key], index) => {
        if (saved[index]) Object.defineProperty(object, key, saved[index]);
        else delete object[key];
      });
    }
  });
}

test("`#886 voice-page-autoplay`: off by default and kept; on, a read heard to its end goes on to the next unread row below it in its view, and stops at the end or when halted", async () => {
  const { createAutoplay, AUTOPLAY_WORDS } = VIEW_VOICE;
  const toggle = { attributes: {}, getAttribute(name) { return this.attributes[name] ?? null; }, setAttribute(name, value) { this.attributes[name] = value; } };
  const label = { textContent: "" };
  const row = (id, { read = false, card = false, threadRead = false, ids = card ? [] : [id], root = null } = {}) => ({
    id, read, ids, isConnected: true, ...(root ? { cardRoot: { id: root } } : {}),
    getAttribute: (name) => (name === "data-id" ? id : null),
    hasAttribute: (name) => (name === "data-card" ? card : name === "data-thread-read" ? threadRead : false),
  });
  const main = [row("1"), row("2", { read: true }), row("3"), row("4", { ids: ["4", "5"] }), row("6", { read: true })];
  // A card's own id is its thread's; it reads its thread's first message, whose id a chat service may write otherwise.
  const cards = [row("t1", { card: true, root: "r1" }), row("t2", { card: true, threadRead: true, root: "r2" }), row("t3", { card: true, root: "r3" })];
  const panes = { main, cards };
  let order = ["main", "cards"];
  let stored = false;
  const kept = [];
  const read = [];
  const said = [];
  const archived = Promise.resolve();
  const autoplay = createAutoplay({
    byId: (id) => ({ "read-autoplay": toggle, "autoplay-toggle-label": label })[id],
    stored: () => stored,
    keep: (on) => kept.push(on),
    panes: () => order,
    paneRows: (pane) => panes[pane],
    idsOf: (one) => one.ids,
    rowIsRead: (one) => one.read,
    paneRowFor: (pane, id, card) => panes[pane].find((one) => one.id === id && one.hasAttribute("data-card") === card) || null,
    read: (one) => read.push(one.redrawn ? `${one.id} redrawn` : one.id),
    setStatus: (text) => said.push(text),
  });
  autoplay.render();
  assert.deepStrictEqual([autoplay.on(), toggle.attributes["aria-pressed"], label.textContent], [false, "false", AUTOPLAY_WORDS.off]);
  assert.equal(autoplay.nextAfter(["1"]), null, "off, a read went on");
  autoplay.setOn(true);
  assert.deepStrictEqual([kept, toggle.attributes["aria-pressed"], label.textContent], [[true], "true", "Autoplay: on"]);
  stored = false;
  assert.equal(autoplay.on(), true, "the choice made here was lost to the storage's answer");
  // Below the read, skipping what is read: 1 goes on to 3, past the read 2.
  assert.equal(autoplay.nextAfter(["1"])(archived), true);
  assert.deepStrictEqual(read, ["3"]);
  // A combined row read as its messages: 4 and 5 go on past the read 6 to nothing, said once the archive is said.
  const end = autoplay.nextAfter(["4", "5"]);
  assert.equal(end(archived), false, "past the last unread a read went on");
  assert.deepStrictEqual(said, [], "the end was said before the archive's own lines");
  await archived;
  assert.deepStrictEqual(said, ["Autoplay: nothing unread below."]);
  // The message read is found as one of a combined row's messages too: 5 is in 4's row, and goes on to nothing.
  assert.equal(autoplay.nextAfter(["5"])(archived), false);
  // In the thread list, a card found by its thread's first message, on to the next card with unread replies.
  assert.equal(autoplay.nextAfter(["r1"])(archived), true);
  assert.deepStrictEqual(read, ["3", "t3"], "a card's read did not go on past the card whose thread is read");
  // The pane the reader works in first: the same id in two panes goes on in the first.
  panes.cards.unshift(row("3"));
  order = ["cards", "main"];
  autoplay.nextAfter(["3"])(archived);
  order = ["main", "cards"];
  // Halted (Hang up): the read in progress goes on to nothing, once; the next read goes on again.
  autoplay.halt("1");
  assert.equal(autoplay.nextAfter(["1"]), null, "a halted read went on");
  autoplay.nextAfter(["1"])(archived);
  // A halt for another read leaves this one going on; Stop's forgets one.
  autoplay.halt("9");
  autoplay.nextAfter(["1"])(archived);
  autoplay.halt("1");
  autoplay.halt(null);
  autoplay.nextAfter(["1"])(archived);
  // Found again after the archive redrew the list: the new row for the same id is the one read.
  const going = autoplay.nextAfter(["1"]);
  panes.main[2] = { ...row("3"), redrawn: true };
  going(archived);
  assert.deepStrictEqual(read, ["3", "t3", "t1", "3", "3", "3", "3 redrawn"], "a read went on to the wrong row");
  // A row gone from the page by the time it is read: nothing read, and the caller told so.
  const gone = autoplay.nextAfter(["1"]);
  const [left] = panes.main.splice(2, 1);
  left.isConnected = false;
  assert.equal(gone(archived), false, "a row gone from the page was read, or the caller was told a read started");
  assert.equal(read.length, 7, "a row gone from the page was read");
  autoplay.setOn(false);
  assert.deepStrictEqual([kept, toggle.attributes["aria-pressed"], label.textContent], [[true, false], "false", "Autoplay: off"]);
});

test("`#624 voice-page-modularity`: the read-aloud controls reach the act each names in the page's order, Read is Pause, Stop or the mode by the read's state, and come down again", () => {
  const { mountReadControls } = VIEW_VOICE;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = {}) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  let inProgress = false;
  let pending = null;
  let mode = false;
  let autoplaying = false;
  const controls = mountReadControls({
    byId: (id) => target(id),
    win: () => target("window"),
    stepReadSpeed: (direction) => log.push(`pace ${direction}`),
    commitReadSpeedBox: (...args) => log.push(`pace box ${args.length}`),
    stepReading: (delta) => { log.push(`block ${delta}`); return "stepped"; },
    stepSection: (delta) => log.push(`section ${delta}`),
    onReadGripDown: (event) => { log.push(`grip down ${event.n}`); return "grip"; },
    onReadGripMove: (event) => log.push(`grip move ${event.n}`),
    onReadGripUp: (...args) => log.push(`grip up ${args.length}`),
    onReadGripKey: (event) => log.push(`grip key ${event.key}`),
    moveReadSteer: (offset) => log.push(`home ${offset}`),
    placeReadSteer: (...args) => log.push(`place ${args.length}`),
    autoplayOn: () => autoplaying,
    setAutoplay: (on) => { autoplaying = on; log.push(`autoplay ${on}`); },
    forgetAutoplayHalt: () => log.push("halt forgotten"),
    readInProgress: () => inProgress,
    toggleReadingPaused: () => log.push("pause"),
    pendingRead: () => pending,
    readingMode: () => mode,
    setReadingMode: (on) => { mode = on; log.push(`mode ${on}`); },
    leaveReadingMode: (...args) => log.push(`leave ${args.length}`),
  });
  assert.deepStrictEqual(log, ["listen read-speed-down click", "listen read-speed-up click", "listen read-speed-box change",
    "listen read-speed-box blur", "listen read-speed-box keydown", "listen read-prev click", "listen read-next click",
    "listen read-section-prev click", "listen read-section-next click", "listen read-steer-grip pointerdown",
    "listen read-steer-grip pointermove", "listen read-steer-grip pointerup", "listen read-steer-grip pointercancel",
    "listen read-steer-grip lostpointercapture", "listen read-steer-grip keydown", "listen read-steer-grip dblclick",
    "listen read-autoplay click", "listen window resize", "listen read-aloud click", "listen read-aloud-stop click",
    "listen read-aloud-stop click"], "a listener was added out of the page's order");
  log.length = 0;
  fire("read-speed-down", "click");
  fire("read-speed-up", "click");
  fire("read-speed-box", "change", { n: 1 });
  fire("read-speed-box", "blur", { n: 1 });
  let refused = 0;
  const key = (name) => ({ key: name, preventDefault: () => { refused += 1; } });
  for (const name of ["Enter", "ArrowUp", "ArrowDown", "a"]) fire("read-speed-box", "keydown", key(name));
  assert.equal(refused, 3, "the pace box kept a key's default it takes, or took one it does not");
  assert.deepStrictEqual(fire("read-prev", "click"), [undefined], "a step answered what it did, which the page's never has");
  fire("read-next", "click");
  fire("read-section-prev", "click");
  fire("read-section-next", "click");
  assert.deepStrictEqual(fire("read-steer-grip", "pointerdown", { n: 1 }), ["grip"]);
  fire("read-steer-grip", "pointermove", { n: 2 });
  for (const type of ["pointerup", "pointercancel", "lostpointercapture"]) fire("read-steer-grip", type, { n: 3 });
  fire("read-steer-grip", "keydown", { key: "Home" });
  fire("read-steer-grip", "dblclick");
  fire("read-autoplay", "click");
  fire("window", "resize", { n: 4 });
  assert.deepStrictEqual(log, ["pace -1", "pace 1", "pace box 0", "pace box 0", "pace box 0", "pace 1", "pace -1", "block -1", "block 1",
    "section -1", "section 1", "grip down 1", "grip move 2", "grip up 0", "grip up 0", "grip up 0", "grip key Home", "home null",
    "autoplay true", "place 0"]);
  log.length = 0;
  // Read: the mode with nothing playing; Stop while a read is fetched; Pause or Resume while one plays or is held.
  fire("read-aloud", "click");
  pending = "7";
  fire("read-aloud", "click");
  inProgress = true;
  fire("read-aloud", "click");
  fire("read-aloud-stop", "click", { n: 5 });
  assert.deepStrictEqual(log, ["mode true", "leave 0", "pause", "leave 0", "halt forgotten"]);
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the call's controls reach the act each names in the page's order, answer what it answers, and come down again", () => {
  const { mountCallControls } = VIEW_VOICE;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = {}) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  const controls = mountCallControls({
    byId: (id) => target(id),
    onTalk: (...args) => { log.push(`talk ${args.length}`); return "talked"; },
    hangUp: (...args) => { log.push(`hang up ${args.length}`); return "hung up"; },
    haltAutoplay: (...args) => log.push(`halt ${args.length}`),
    clearView: (...args) => { log.push(`clear ${args.length}`); return "cleared"; },
    forgetConversations: (...args) => { log.push(`forget ${args.length}`); return "forgotten"; },
    toggleSpeaker: (...args) => { log.push(`speaker ${args.length}`); return "speaker"; },
    retryStart: (...args) => { log.push(`retry ${args.length}`); return "retried"; },
  });
  assert.deepStrictEqual(log, ["listen talk click", "listen hang-up click", "listen hang-up click", "listen clear-view click",
    "listen forget-conversations click", "listen speaker click", "listen retry-start click"], "a listener was added out of the page's order");
  log.length = 0;
  assert.deepStrictEqual(fire("talk", "click", { n: 1 }), ["talked"], "Talk did not answer what the call's own Talk answers");
  // Hang up ends the call, and then the going on of a read (`#886 voice-page-autoplay`).
  // (The halt answers the log's length, as the stand-in's push does.)
  assert.deepStrictEqual(fire("hang-up", "click", { n: 2 }), ["hung up", 3]);
  assert.deepStrictEqual(fire("clear-view", "click", { n: 3 }), ["cleared"]);
  assert.deepStrictEqual(fire("forget-conversations", "click", { n: 4 }), ["forgotten"]);
  assert.deepStrictEqual(fire("speaker", "click", { n: 5 }), ["speaker"]);
  assert.deepStrictEqual(fire("retry-start", "click", { n: 6 }), ["retried"]);
  assert.deepStrictEqual(log, ["talk 0", "hang up 0", "halt 0", "clear 0", "forget 0", "speaker 0", "retry 0"], "an act was handed the event, which none of the page's takes");
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the page's buttons beside the call reach the act each names in the page's order, the press handed to the acts that take it, and come down again", () => {
  const { mountPageButtons } = VIEW_VOICE;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, value: "", listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = { id }) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  const act = (name) => (...args) => { log.push(`${name}${args.map((one) => ` ${one && one.id ? `event:${one.id}` : one}`).join("")}`); return name; };
  const buttons = mountPageButtons({
    lookup: (id) => target(id),
    dismissBanner: act("dismiss banner"), dismissStatus: act("dismiss status"), sendPostProposal: act("send post"),
    cancelPostProposal: act("cancel post"), togglePostDraft: act("more"), openSettings: act("open settings"), clearError: act("clear error"),
    readOnDevice: act("read on device"), toggleAudioSource: act("toggle source"), setWitVoice: act("voice"), setReadAudioSource: act("source"),
    closeSettings: act("close settings"),
  });
  assert.deepStrictEqual(log, ["listen dismiss-banner click", "listen dismiss-status click", "listen post-confirm-send click",
    "listen post-confirm-cancel click", "listen post-confirm-more click", "listen open-settings click", "listen dismiss-error click",
    "listen read-on-device click", "listen audio-source click", "listen settings-wit-voice change", "listen settings-audio-source change",
    "listen close-settings click"], "a button was heard out of the page's order");
  log.length = 0;
  for (const id of ["dismiss-banner", "dismiss-status", "post-confirm-send", "post-confirm-cancel", "post-confirm-more", "open-settings",
    "dismiss-error", "read-on-device", "audio-source", "close-settings"]) fire(id, "click");
  nodes["settings-wit-voice"].value = "aria";
  fire("settings-wit-voice", "change");
  nodes["settings-audio-source"].value = "device";
  assert.deepStrictEqual(fire("settings-audio-source", "change"), ["source"], "what an act answers did not reach the dispatcher");
  assert.deepStrictEqual(log, ["dismiss banner event:dismiss-banner", "dismiss status event:dismiss-status", "send post event:post-confirm-send",
    "cancel post event:post-confirm-cancel", "more", "open settings", "clear error event:dismiss-error", "read on device event:read-on-device",
    "toggle source", "close settings event:close-settings", "voice aria", "source device"]);
  buttons.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the call's box and the canned prompts are restored as kept and heard in the page's order, each prompt's button sending its own, and come down again", () => {
  const { mountCallComposer } = VIEW_VOICE;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, value: "", listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({ type }));
  const entries = [{ field: "prompt-a", button: "send-a", key: "a" }, { field: "prompt-b", button: "send-b", key: "b" }];
  const composer = mountCallComposer({
    lookup: (id) => target(id),
    sendTyped: (...args) => { log.push(`send ${args.length}`); return "sending"; },
    storedComposeDraft: () => { log.push("stored draft"); return "half a thought"; },
    composerFocused: () => log.push("focused"),
    composeInput: () => log.push("input"),
    cannedPrompts: entries,
    storedCannedPrompts: () => { log.push("stored prompts"); return { a: "what's new?", b: "summarise" }; },
    cannedPromptsChanged: (...args) => log.push(`prompts changed ${args.length}`),
    pressCannedPrompt: (entry) => log.push(`press ${entry.key}`),
  });
  assert.deepStrictEqual(log, ["listen send-text click", "stored draft", "listen compose-text focus", "listen compose-text input",
    "stored prompts", "listen prompt-a input", "listen prompt-a change", "listen send-a click",
    "stored prompts", "listen prompt-b input", "listen prompt-b change", "listen send-b click"], "restored or heard out of the page's order");
  assert.deepStrictEqual([nodes["compose-text"].value, nodes["prompt-a"].value, nodes["prompt-b"].value], ["half a thought", "what's new?", "summarise"]);
  log.length = 0;
  assert.deepStrictEqual(fire("send-text", "click"), ["sending"], "Send did not answer what the send answers");
  fire("compose-text", "focus");
  fire("compose-text", "input");
  fire("prompt-b", "input");
  fire("prompt-a", "change");
  fire("send-b", "click");
  fire("send-a", "click");
  assert.deepStrictEqual(log, ["send 0", "focused", "input", "prompts changed 0", "prompts changed 0", "press b", "press a"]);
  composer.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});
