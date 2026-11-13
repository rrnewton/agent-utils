// Tests for web/src/view/settings/ on its own: the module built as the page build builds it and held
// directly, handed elements of its own with no page around them (`#624 voice-page-modularity`). The
// whole page's Settings and Help, folded, are tested in ../page/settings.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The Settings and Help screens as the DOM draws them (web/src/view/settings). */
const VIEW_SETTINGS = await moduleForTests("src/view/settings/index.ts");
/** The lists the folds are drawn from (web/src/present/settings). */
const PRESENT_SETTINGS = await moduleForTests("src/present/settings/index.ts");

/** An element with attributes, listeners, a parent and `hidden`: what the folds touch. */
class Node {
  constructor(id, parentNode = null) {
    this.id = id;
    this.parentNode = parentNode;
    this.hidden = false;
    this.open = false;
    this.textContent = "";
    this.attributes = new Map();
    this.listeners = new Map();
    this.scrolled = [];
  }
  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }
  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }
  removeAttribute(name) {
    this.attributes.delete(name);
  }
  addEventListener(type, listener) {
    (this.listeners.get(type) || this.listeners.set(type, []).get(type)).push(listener);
  }
  fire(type) {
    for (const listener of this.listeners.get(type) || []) listener({ type });
  }
  scrollIntoView(how) {
    this.scrolled.push(how);
  }
}

/** Settings with every fold's button and body, each group inside its own section's body. */
function settingsPage() {
  const nodes = new Map();
  const screen = new Node("screen-settings");
  nodes.set("fold-settings-all", new Node("fold-settings-all", screen));
  let section = screen;
  for (const slug of PRESENT_SETTINGS.SETTINGS_FOLDS) {
    const parent = slug.startsWith("settings-") ? screen : section;
    const button = new Node(`fold-${slug}`, parent);
    const body = new Node(`fold-${slug}-body`, parent);
    nodes.set(button.id, button);
    nodes.set(body.id, body);
    if (slug.startsWith("settings-")) section = body;
  }
  let closed = 0;
  const folds = VIEW_SETTINGS.mountSettingsFolds({ intoView: (element, block, inline) => { if (typeof element.scrollIntoView === "function") element.scrollIntoView(inline === undefined ? { block } : { block, inline }); }, lookup: (id) => nodes.get(id), closeDismissed: () => { closed += 1; } });
  return { nodes, folds, closed: () => closed };
}

test("`#624 voice-page-modularity`: Settings mounts folded, a heading opens and folds its group, and Expand all counts only the folds on the page", () => {
  const { nodes, folds, closed } = settingsPage();
  const all = nodes.get("fold-settings-all");
  for (const slug of PRESENT_SETTINGS.SETTINGS_FOLDS) {
    assert.equal(nodes.get(`fold-${slug}`).getAttribute("aria-expanded"), "false");
    assert.equal(nodes.get(`fold-${slug}-body`).getAttribute("hidden"), "until-found");
  }
  assert.equal(all.textContent, "Expand all");
  nodes.get("fold-microphone").fire("click");
  assert.equal(folds.open("microphone"), true);
  assert.equal(nodes.get("fold-microphone-body").getAttribute("hidden"), null);
  assert.deepStrictEqual(nodes.get("fold-microphone").scrolled, [{ block: "nearest" }], "the heading pressed was not kept in view");
  // Folding a fold that holds the pop-out closes it; another fold does not.
  nodes.get("fold-dismissed").fire("click");
  nodes.get("fold-dismissed").fire("click");
  assert.equal(closed(), 1);
  // The browser's find opens a body: the button says so.
  nodes.get("fold-typing-body").fire("beforematch");
  assert.equal(folds.open("typing"), true);
  // A group hidden is not on the page, and Expand all does not wait for it.
  nodes.get("fold-settings-all").fire("click");
  assert.equal(all.textContent, "Fold all");
  folds.set("provider", false);
  assert.equal(all.textContent, "Expand all");
  nodes.get("fold-provider").parentNode.hidden = true;
  assert.equal(folds.shown("provider"), true, "a folded body around it is not hiding it");
  const group = new Node("provider-group", nodes.get("fold-provider").parentNode);
  group.hidden = true;
  nodes.get("fold-provider").parentNode = group;
  assert.equal(folds.shown("provider"), false);
  folds.renderAll();
  assert.equal(all.textContent, "Fold all");
  // Fold all folds every fold, the hidden one's too, and closes the pop-out.
  all.fire("click");
  for (const slug of PRESENT_SETTINGS.SETTINGS_FOLDS) assert.equal(folds.open(slug), false, `Fold all left ${slug} open`);
  assert.equal(closed(), 2);
});

test("`#624 voice-page-modularity`: Help opens at one entry, folds the rest, and its button counts only the entries on the screen", () => {
  const nodes = new Map([["fold-help", new Node("fold-help")]]);
  for (const slug of PRESENT_SETTINGS.helpEntrySlugs()) nodes.set(`help-${slug}`, new Node(`help-${slug}`));
  const help = VIEW_SETTINGS.mountHelpFolds({ intoView: (element, block, inline) => { if (typeof element.scrollIntoView === "function") element.scrollIntoView(inline === undefined ? { block } : { block, inline }); }, lookup: (id) => nodes.get(id), byId: (id) => nodes.get(id) || null });
  const button = nodes.get("fold-help");
  assert.equal(help.entries().length, PRESENT_SETTINGS.HELP_TOPICS.length + 2);
  help.openAt("keyboard");
  assert.deepStrictEqual(help.entries().filter((entry) => entry.open).map((entry) => entry.id), ["help-keyboard"]);
  assert.deepStrictEqual(nodes.get("help-keyboard").scrolled, [{ block: "start" }]);
  assert.equal(button.textContent, "Expand all");
  button.fire("click");
  assert.ok(help.entries().every((entry) => entry.open));
  assert.equal(button.textContent, "Fold all");
  // An entry the reader folds is heard; a hidden one is not counted.
  nodes.get("help-storage").open = false;
  nodes.get("help-storage").fire("toggle");
  assert.equal(button.textContent, "Expand all");
  nodes.get("help-storage").hidden = true;
  help.render();
  assert.equal(button.textContent, "Fold all");
  // No slug: every entry folded, nothing scrolled to.
  help.openAt(null);
  assert.ok(help.entries().every((entry) => !entry.open));
  assert.deepStrictEqual(nodes.get("help-keyboard").scrolled, [{ block: "start" }]);
});

test("`#624 voice-page-modularity`: Save marks itself, puts itself back after a save, and a revert waiting is dropped when it is marked again", () => {
  const { mountSaveTokenButton } = VIEW_SETTINGS;
  const button = { textContent: "Save token", className: "", disabled: false };
  const looked = [];
  const timers = new Map();
  let next = 0;
  const save = mountSaveTokenButton({
    lookup: (id) => { looked.push(id); return button; },
    setTimeout: (run, ms) => { next += 1; timers.set(next, { run, ms }); return next; },
    clearTimeout: (timer) => { timers.delete(timer); },
  });
  assert.deepStrictEqual(looked, [], "mounting it touched the page");
  save.markSave("Saving…", "", true);
  assert.deepStrictEqual([button.textContent, button.className, button.disabled, looked], ["Saving…", "", true, ["save-token"]]);
  let reverted = 0;
  const revert = () => { reverted += 1; save.markSave("Save token", "", false); };
  save.markSave("Saved", "ok", false);
  save.revertLater(revert, 2500);
  assert.deepStrictEqual([...timers.values()].map(({ ms }) => ms), [2500]);
  // Marked again before it fires: the revert waiting is dropped.
  save.markSave("Saving…", "", true);
  assert.equal(timers.size, 0, "a second mark left the revert waiting");
  save.markSave("Saved", "ok", false);
  save.revertLater(revert, 2500);
  const [[handle, { run }]] = [...timers.entries()];
  timers.delete(handle);
  run();
  assert.deepStrictEqual([reverted, button.textContent, button.className, button.disabled], [1, "Save token", "", false]);
});

test("`#624 voice-page-modularity`: the screen's sliders, steps, reload and grip mount in the page's order, the build said between them, reach what each names, and come down again", () => {
  const { mountScreenControls } = VIEW_SETTINGS;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, value: `${id} value`, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = {}) => nodes[id].listeners.filter((one) => one.type === type).forEach((one) => one.listener(event));
  const said = (name) => (...args) => log.push(`${name} ${args.map((one) => (one && one.n) || one).join(" ")}`.trim());
  const controls = mountScreenControls({
    lookup: (id) => target(id),
    readingWidthChanged: said("width"),
    textSizeChanged: said("size"),
    stepTextSize: said("size step"),
    collapsedLinesChanged: said("lines"),
    stepCollapsedLines: said("lines step"),
    cardMaxLinesChanged: said("card"),
    stepCardMaxLines: said("card step"),
    reloadPage: said("reload"),
    showPageBuild: said("show build"),
    quoteReadPercentChanged: said("quote"),
    stepQuoteReadPercent: said("quote step"),
    markReadFirstPercentChanged: said("first"),
    stepMarkReadFirstPercent: said("first step"),
    markReadLongWordsChanged: said("long"),
    stepMarkReadLongWords: said("long step"),
    gripDown: said("grip down"),
    gripMove: said("grip move"),
    gripUp: said("grip up"),
    gripKey: said("grip key"),
  });
  assert.deepStrictEqual(log, ["listen reading-width input", "listen msg-scale input", "listen msg-scale-smaller click",
    "listen msg-scale-larger click", "listen collapsed-lines input", "listen collapsed-lines-smaller click", "listen collapsed-lines-larger click",
    "listen card-max-lines input", "listen page-update click", "show build", "listen card-max-lines-smaller click",
    "listen card-max-lines-larger click", "listen quote-read-percent input", "listen quote-read-percent-smaller click",
    "listen quote-read-percent-larger click", "listen mark-read-first-percent input", "listen mark-read-first-percent-smaller click",
    "listen mark-read-first-percent-larger click", "listen mark-read-long-words input", "listen mark-read-long-words-smaller click",
    "listen mark-read-long-words-larger click", "listen width-grip pointerdown", "listen width-grip pointermove", "listen width-grip pointerup",
    "listen width-grip pointercancel", "listen width-grip keydown"], "a listener was added out of the page's order, or the build said elsewhere");
  log.length = 0;
  // A slider says its value as it is when it moves, not as it was at the mount.
  const moved = { "reading-width": "72", "msg-scale": "110", "collapsed-lines": "5", "card-max-lines": "4", "quote-read-percent": "60",
    "mark-read-first-percent": "40", "mark-read-long-words": "250" };
  for (const [id, now] of Object.entries(moved)) {
    nodes[id].value = now;
    fire(id, "input");
  }
  for (const id of ["msg-scale", "collapsed-lines", "card-max-lines", "quote-read-percent", "mark-read-first-percent", "mark-read-long-words"]) {
    fire(`${id}-smaller`, "click");
    fire(`${id}-larger`, "click");
  }
  fire("page-update", "click", { n: "press" });
  fire("width-grip", "pointerdown", { n: "down" });
  fire("width-grip", "pointermove", { n: "move" });
  fire("width-grip", "pointerup", { n: "up" });
  fire("width-grip", "pointercancel", { n: "cancel" });
  fire("width-grip", "keydown", { n: "ArrowLeft" });
  assert.deepStrictEqual(log, ["width 72", "size 110", "lines 5", "card 4", "quote 60", "first 40", "long 250", "size step -1", "size step 1", "lines step -1", "lines step 1", "card step -1", "card step 1",
    "quote step -1", "quote step 1", "first step -1", "first step 1", "long step -1", "long step 1", "reload", "grip down down", "grip move move", "grip up", "grip up", "grip key ArrowLeft"]);
  controls.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: Settings' switches and choices are restored as kept before they are heard, in the page's order, reach what each names, and come down again", () => {
  const { mountSettingsSwitches } = VIEW_SETTINGS;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, value: "", checked: undefined, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).forEach((one) => one.listener({ type }));
  const said = (name, answer) => (...args) => { log.push(`${name}${args.length ? ` ${args.join(" ")}` : ""}`); return answer; };
  const media = target("media");
  const switches = mountSettingsSwitches({
    lookup: (id) => target(id),
    storedMicSettings: said("stored mic", { echoCancellation: false, noiseSuppression: true, autoGainControl: false }),
    micSettingsChanged: said("mic changed"),
    refreshAudioDevices: said("refresh devices"),
    audioDevicesChanged: said("devices changed"),
    renderAudioRoute: said("say route"),
    mediaDevices: () => media,
    storedBarPlacement: said("stored bar", "top"),
    barPlacementChanged: said("bar changed"),
    storedSlackOpen: said("stored slack", "app"),
    slackOpenChanged: said("slack changed"),
    cycleReadNew: said("cycle read new"),
    chooseReadNew: said("choose read new"),
    renderReadNew: said("say read new"),
    renderLiveState: said("say live"),
    resumeWanted: said("stored resume", true),
    resumeToggled: said("resume toggled"),
  });
  assert.deepStrictEqual(log, ["stored mic", "listen mic-echo-cancellation change", "listen mic-noise-suppression change", "listen mic-auto-gain change",
    "refresh devices", "listen audio-input-device change", "listen audio-output-device change", "listen audio-route-toggle change", "listen media devicechange",
    "stored bar", "listen bar-placement change", "stored slack", "listen slack-open-in change", "listen read-new click", "listen relay-to-agent change",
    "say read new", "say live", "stored resume", "listen resume-toggle change"], "a control was restored or heard out of the page's order");
  assert.deepStrictEqual(["mic-echo-cancellation", "mic-noise-suppression", "mic-auto-gain"].map((id) => nodes[id].checked), [false, true, false]);
  assert.deepStrictEqual([nodes["bar-placement"].value, nodes["slack-open-in"].value, nodes["resume-toggle"].checked], ["top", "app", true],
    "a choice was not restored as kept");
  log.length = 0;
  fire("mic-noise-suppression", "change");
  fire("audio-input-device", "change");
  fire("audio-output-device", "change");
  fire("audio-route-toggle", "change");
  fire("media", "devicechange");
  fire("bar-placement", "change");
  fire("slack-open-in", "change");
  fire("read-new", "click");
  // The select names its mode as it is when it changes.
  nodes["relay-to-agent"].value = "gist";
  fire("relay-to-agent", "change");
  fire("resume-toggle", "change");
  assert.deepStrictEqual(log, ["mic changed", "devices changed", "devices changed", "say route", "refresh devices", "say route", "bar changed",
    "slack changed", "cycle read new", "choose read new gist", "resume toggled"]);
  switches.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: Settings' switches mount where the browser has no media devices to listen to", () => {
  const { mountSettingsSwitches } = VIEW_SETTINGS;
  const heard = [];
  const node = () => ({ value: "", addEventListener: (type) => heard.push(type), removeEventListener() {} });
  const nothing = () => undefined;
  mountSettingsSwitches({
    lookup: node, storedMicSettings: () => ({ echoCancellation: true, noiseSuppression: true, autoGainControl: true }),
    micSettingsChanged: nothing, refreshAudioDevices: nothing, audioDevicesChanged: nothing, renderAudioRoute: nothing,
    mediaDevices: () => undefined, storedBarPlacement: () => "bottom", barPlacementChanged: nothing, storedSlackOpen: () => "browser",
    slackOpenChanged: nothing, cycleReadNew: nothing, chooseReadNew: nothing, renderReadNew: nothing, renderLiveState: nothing,
    resumeWanted: () => false, resumeToggled: nothing,
  });
  assert.ok(!heard.includes("devicechange"), "a device change was listened for with no media devices");
});

test("`#624 voice-page-modularity`: every setting is restored as kept, once, in the page's order", () => {
  const { restoreStoredSettings } = VIEW_SETTINGS;
  const order = ["readingWidth", "textSize", "collapsedLines", "cardMaxLines", "quoteReadPercent", "markRead", "readSpeed", "ownRead", "combine",
    "hideEmpty", "richHeaders", "inAppLinks", "openInView", "afterDone", "summarizeCollapsed", "enterSends"];
  const log = [];
  const deps = Object.fromEntries([...order].reverse().map((name) => [name, (...args) => log.push(`${name}${args.length ? ` ${args.length}` : ""}`)]));
  assert.equal(restoreStoredSettings(deps), undefined);
  assert.deepStrictEqual(log, order, "a setting was restored out of the page's order, twice, or handed something");
});

test("`#624 voice-page-modularity`: sign-in's and pairing's listeners mount in the page's order, a submitted form doing nothing and a second link offered, and come down again", () => {
  const { mountSignInControls } = VIEW_SETTINGS;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = {}) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  const act = (name) => (...args) => { log.push(`${name}${args.length ? ` ${args.join(" ")}` : ""}`); return name; };
  const codes = ["CODE-2", null];
  const was = globalThis.window;
  globalThis.window = target("window");
  try {
    const controls = mountSignInControls({
      lookup: (id) => target(id),
      saveToken: act("save"), forgetToken: act("forget"), redeemPairingInput: act("redeem input"), redeemPairing: act("redeem"),
      declinePairing: act("decline"), takePairingCode: () => { log.push("take code"); return codes.shift(); }, pairingOpened: act("offer"),
    });
    assert.deepStrictEqual(log, ["listen signin-form submit", "listen save-token click", "listen forget-token click", "listen pair-submit click",
      "listen pair-confirm click", "listen pair-dismiss click", "listen window hashchange"], "a listener was added out of the page's order");
    log.length = 0;
    let prevented = 0;
    fire("signin-form", "submit", { preventDefault: () => { prevented += 1; } });
    assert.equal(prevented, 1, "a submitted form was let through");
    assert.deepStrictEqual(["save-token", "forget-token", "pair-submit", "pair-confirm", "pair-dismiss"].flatMap((id) => fire(id, "click")),
      ["save", "forget", "redeem input", "redeem", "decline"], "what an act answers did not reach the dispatcher");
    // A link with a code is offered; a fragment change without one offers nothing.
    fire("window", "hashchange");
    fire("window", "hashchange");
    assert.deepStrictEqual(log, ["save", "forget", "redeem input", "redeem", "decline", "take code", "offer CODE-2", "take code"]);
    controls.unmount();
    assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
  } finally {
    globalThis.window = was;
  }
});

test("`#624 voice-page-modularity`: the reading preferences' switches reach what each names in the page's order, Enter sends read as the switch now says, and come down again", () => {
  const { mountPreferenceSwitches } = VIEW_SETTINGS;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, checked: false, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener({ type }));
  const act = (name) => (...args) => { log.push(`${name}${args.length ? ` ${args.join(" ")}` : ""}`); return name; };
  const switches = mountPreferenceSwitches({
    lookup: (id) => target(id),
    toggleSpeedPopover: act("pace"), applyOwnRead: act("own apply"), undoOwnRead: act("own undo"), markOwnReadChanged: act("own"),
    hideEmptyChanged: act("empty"), combineChanged: act("combine"), richHeadersChanged: act("headers"), inAppLinksChanged: act("links"),
    openInViewChanged: act("open in view"), afterDoneChanged: act("after done"), summariesChanged: act("summaries"), enterSendsChanged: act("enter sends"),
    markReadChanged: act("mark"), markReadQuoteGuardChanged: act("mark size"), markReadFoldGuardChanged: act("mark fold"),
    markReadNonquoteChanged: act("mark plain"), markReadFinalGuardChanged: act("mark final"), markReadFirstGuardChanged: act("mark first"),
  });
  const order = [["read-speed", "click"], ["own-read-apply", "click"], ["own-read-undo", "click"], ["mark-own-read", "change"],
    ["hide-empty-messages", "change"], ["combine-messages", "change"], ["rich-headers", "change"], ["in-app-links", "change"],
    ["open-in-view", "change"], ["after-done", "change"], ["summarize-collapsed", "change"], ["enter-sends", "change"],
    ["mark-read-on-reply", "change"], ["mark-read-quote-guard", "change"], ["mark-read-fold-guard", "change"],
    ["mark-read-nonquote", "change"], ["mark-read-final-guard", "change"], ["mark-read-first-guard", "change"]];
  assert.deepStrictEqual(log, order.map(([id, type]) => `listen ${id} ${type}`), "a switch was heard out of the page's order");
  log.length = 0;
  nodes["enter-sends"].checked = true;
  assert.deepStrictEqual(order.flatMap(([id, type]) => fire(id, type)),
    ["pace", "own apply", "own undo", "own", "empty", "combine", "headers", "links", "open in view", "after done", "summaries", "enter sends",
     "mark", "mark size", "mark fold", "mark plain", "mark final", "mark first"]);
  assert.deepStrictEqual(log, ["pace", "own apply", "own undo", "own", "empty", "combine", "headers", "links", "open in view", "after done", "summaries",
    "enter sends true", "mark", "mark size", "mark fold", "mark plain", "mark final", "mark first"], "an act was handed the event, or Enter sends not read as the switch says");
  switches.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: Help is closed, opened at its top, and opened at each topic's entry, in the page's order, and comes down again", () => {
  const { mountHelpLinks } = VIEW_SETTINGS;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const links = mountHelpLinks({ lookup: (id) => target(id), closeHelp: (...args) => log.push(`close ${args.length}`), showHelp: (topic) => log.push(`show ${topic}`) });
  const topics = log.filter((line) => line.startsWith("listen help-link-")).map((line) => line.slice("listen help-link-".length));
  assert.ok(topics.length > 0, "no topic's ? was heard");
  assert.deepStrictEqual(log, ["listen close-help", "listen open-help", ...topics.map((topic) => `listen help-link-${topic}`)]);
  log.length = 0;
  for (const id of ["close-help", "open-help", `help-link-${topics[0]}`]) nodes[id].listeners.forEach((one) => one.listener({ type: "click" }));
  assert.deepStrictEqual(log, ["close 0", "show null", `show ${topics[0]}`]);
  links.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: the channel box's listeners are mounted in the page's order, each answering its own act, then all come down", () => {
  const { mountChannelBoxListeners } = VIEW_SETTINGS;
  const log = [];
  const nodes = {};
  const target = (id) => (nodes[id] ||= { id, listeners: [],
    addEventListener(type, listener) { this.listeners.push({ type, listener }); log.push(`listen ${id} ${type}`); },
    removeEventListener(type, listener) { this.listeners = this.listeners.filter((one) => one.type !== type || one.listener !== listener); } });
  const fire = (id, type, event = { type }) => nodes[id].listeners.filter((one) => one.type === type).map((one) => one.listener(event));
  const act = (name) => (...args) => { log.push(`${name} ${args.length}`); return name; };
  const box = mountChannelBoxListeners({
    lookup: (id) => target(id),
    selectChannel: act("select"), changeSelectedChannel: act("change channel"), channelPicked: act("picked"),
    chooseSettingsService: act("service"), changeSettingsChannel: act("settings channel"),
    renderClearChoice: act("clear choice"), saveClear: act("save clear"), renderHistoryChoice: act("history choice"),
    saveHistory: act("save history"), prioritizeHistory: act("prioritize"), pushClear: act("push clear"),
    openRename: act("open rename"), cancelRename: act("cancel rename"), openAddChannel: act("open add"),
    cancelAddChannel: act("cancel add"), addChannel: act("add"), openBrowse: act("open browse"),
    closeBrowse: act("close browse"), scheduleDirectorySearch: act("schedule search"), directorySearchKey: act("search key"),
    directoryMore: act("more"), directoryRetry: act("retry"), removeChannel: act("remove"), saveAlias: act("save alias"),
    clearAlias: act("clear alias"), addNoiseRule: act("add noise"), noiseRuleKey: act("noise key"),
    saveSendSetting: act("ask before send"), renderPrompt: act("prompt"), saveInstructions: act("save instructions"),
    saveSystemPrompt: act("save system"), restoreSystemPrompt: act("restore system"), renderPrefix: act("prefix"),
    savePrefix: act("save prefix"), resetPrefix: act("reset prefix"), cacheSwitched: act("cache switched"),
    askErase: act("ask erase"), answerEraseYes: act("erase yes"), answerEraseNo: act("erase no"),
  });
  assert.deepStrictEqual(log, ["listen discord-channel change", "listen discord-channel change", "listen discord-channel change",
    "listen settings-service change", "listen settings-channel change", "listen upstream-clear-mode change",
    "listen upstream-clear-save click", "listen channel-history-mode change", "listen channel-history-save click",
    "listen channel-history-prioritize click", "listen upstream-clear-push click", "listen rename-channel click",
    "listen cancel-rename click", "listen open-add-channel click", "listen cancel-add-channel click",
    "listen add-channel click", "listen open-browse-channels click", "listen close-browse-channels click",
    "listen channel-directory-search input", "listen channel-directory-search keydown", "listen channel-directory-more click",
    "listen channel-directory-retry click", "listen remove-channel click", "listen save-alias click",
    "listen add-noise-rule click", "listen voice-ask-before-send change", "listen voice-instructions input",
    "listen voice-system-prompt input", "listen save-voice-instructions click", "listen save-voice-system-prompt click",
    "listen restore-voice-system-prompt click", "listen voice-post-prefix input", "listen save-voice-post-prefix click",
    "listen reset-voice-post-prefix click", "listen message-cache-enabled change", "listen message-cache-erase click",
    "listen message-cache-confirm-yes click", "listen message-cache-confirm-no click", "listen noise-rule-new keydown",
    "listen clear-alias click"], "mounted out of the page's order");
  log.length = 0;
  fire("discord-channel", "change"); fire("settings-channel", "change"); fire("rename-channel", "click");
  fire("channel-directory-search", "keydown", { key: "Enter" }); fire("noise-rule-new", "keydown", { key: "Enter" });
  fire("voice-instructions", "input"); fire("clear-alias", "click");
  assert.deepStrictEqual(log, ["select 0", "change channel 0", "picked 0", "settings channel 0", "open rename 0",
    "search key 1", "noise key 1", "prompt 0", "clear alias 0"]);
  box.unmount();
  assert.ok(Object.values(nodes).every((one) => one.listeners.length === 0), "a listener outlived unmount");
});

test("`#624 voice-page-modularity`: client-config puts its facts and settings into force in the page's order", () => {
  const { createClientConfig } = VIEW_SETTINGS;
  const log = [];
  const act = (name) => (...args) => { log.push(`${name}:${args.map(String).join(",")}`); };
  const owner = (name, extra = {}) => new Proxy(extra, { get: (t, k) => (k in t ? t[k] : act(`${name}.${String(k)}`)), set: (t, k, v) => { log.push(`${name}.${String(k)}=${String(v && v.playback || v)}`); return true; } });
  const readAloud = owner("read", { storedWitVoice: () => "wit-1", storedReadAudioSource: () => "", readSpeed: 1 });
  const clientConfig = createClientConfig({
    el: (id) => ({ set textContent(v) { log.push(`el ${id}=${v}`); } }),
    setTitle: act("title"), readAloud: () => readAloud,
    providerFacts: owner("facts"), channelDirectory: owner("directory", { providerName: () => "Slack", channels: () => [] }),
    serverFacts: owner("server"), whoIsWho: owner("who"),
    learnProviders: act("learnProviders"), noteSelfAuthor: act("selfAuthor"),
    voiceProvider: owner("voiceProvider", { current: () => ({ name: "Agent" }) }),
    voiceCall: () => owner("call", { session: {} }),
    settingsReadouts: owner("readouts"),
    channelSettings: owner("channelSettings"),
    fillChannelSelect: act("fill"), restoreChannelComposer: act("restoreComposer"),
    summaryRows: owner("summaries", { disabledByServer: () => "", mode: () => false }),
    setSummaryMode: act("summaryMode"), storedSummarizeCollapsed: () => false,
    renderScrollTools: act("scrollTools"), applyChannelProvider: act("applyProvider"),
    upstreamClearBox: owner("upstream"), callResume: owner("resume"),
    voiceTranscript: () => ({}),
    speechHoldFrom: (h) => h, renderResumeState: act("resumeState"),
    noiseSettings: owner("noise"), agentSettings: owner("agentSettings"), agentPrefix: owner("prefix"),
    serverCacheBox: owner("cache"), seedServerThreadHues: act("hues"),
    selectedChannelId: () => "C1", startChannelStream: act("stream"),
    settingsSync: () => owner("sync"), messageCache: owner("messageCache"),
    uiState: { chosenViewsNow: () => [], forgetChosen: act("forgetChosen") },
    saveUiState: act("saveUi"), keyHints: owner("hints"),
  });
  clientConfig.apply({ threading_supported: true, channels: [{ id: "C1" }], client_settings_supported: true });
  assert.ok(log.indexOf("facts.setThreading:true") < log.indexOf("directory.setChannels:[object Object]"), "threading settled after channels");
  assert.ok(log.indexOf("fill:discord-channel") < log.indexOf("channelSettings.renderChannelBox:undefined") || log.includes("channelSettings.renderChannelBox:"), "pickers filled before the box");
  assert.ok(log.indexOf("stream:C1") < log.indexOf("server.markConfigApplied:") && log.indexOf("server.markConfigApplied:") < log.indexOf("messageCache.saveCacheShell:[object Object]"), "config not marked applied after the stream, before the cache shell");
});

test("`#624 voice-page-modularity`: sign-in shows the main screen on a network failure and sign-in on a refusal", async () => {
  const { createSignIn, SAVE_LABEL } = VIEW_SETTINGS;
  assert.equal(SAVE_LABEL, "Save token");
  const make = (config) => {
    const log = [];
    const sign = createSignIn({
      el: () => ({ set textContent(v) { log.push(`state:${v}`); } }),
      field: () => ({ value: "" }), token: () => "tok", signedOutByRename: () => false,
      config, applyClientConfig: () => log.push("config"),
      forgetMessages: () => log.push("forget"), showError: (t) => log.push(`error:${t}`),
      setStatus: (t) => log.push(`status:${t}`), refusalSentence: (e) => `refused:${e.message}`,
      showScreen: (n) => log.push(`screen:${n}`), loadFreshAt: () => null,
      setChannelFreshness: (s) => log.push(`fresh:${s}`), payReopenRead: () => log.push("reopen-read"),
      channelContextKey: () => "k", threading: () => false,
      reconcileChannelSnapshot: () => log.push("reconcile"), clearError: () => {},
      reopenSavedView: () => log.push("reopen"), setState: (s) => log.push(`set:${s}`),
      voiceTranscript: () => ({ loadStoredConversation: () => log.push("stored") }),
      startSettingsSync: () => log.push("sync"), settleUnconfirmedSends: () => log.push("settle"),
      credentialStore: () => ({ keep() {}, forget() {} }), stopOutgoingSends: () => {},
      markSave: () => {}, revertSaveLater: () => {},
      settingsSync: () => ({ forget() {} }),
      deepLinks: { signOut() {}, noteWritten() {} }, clearLinkedMark() {}, forgetLinkJumps() {},
      backHistory: () => null, location: () => null, stopChannelStream: () => {},
    });
    return { sign, log };
  };
  const refused = make(() => Promise.reject(Object.assign(new Error("no"), { refused: true, status: 401 })));
  assert.equal(await refused.sign.signIn(), false);
  assert.ok(refused.log.includes("screen:signin") && refused.log.includes("forget"), "a refusal kept the main screen");
  const down = make(() => Promise.reject(Object.assign(new Error("down"), { network: true })));
  assert.equal(await down.sign.signIn(), true);
  assert.ok(down.log.includes("screen:main") && down.log.includes("reopen-read"), "a network failure bounced to sign-in");
});

test("`#624 voice-page-modularity`: the settings sync glue makes neither owner until first use, and starts only through its view", () => {
  const { createSettingsSyncGlue } = VIEW_SETTINGS;
  const log = [];
  const glue = createSettingsSyncGlue({
    createSync: (apply, status) => { log.push("make sync"); return { apply, status, refresh: () => Promise.resolve(), onFrame() {}, forget() {}, setSupported() {}, agreed: () => null }; },
    mountView: (sync) => { log.push("make view"); assert.equal(typeof sync, "function"); return { apply: (k) => log.push(`apply:${k}`), status: (t) => log.push(`status:${t}`), start: () => log.push("start") }; },
  });
  assert.deepStrictEqual(log, [], "an owner was made at construction");
  glue.applySyncedSetting("READ_MODE_KEY");
  glue.startSettingsSync();
  assert.deepStrictEqual(log, ["make view", "apply:READ_MODE_KEY", "start"], "the view was not made first, on first use");
  glue.settingsSync();
  glue.settingsSync();
  assert.deepStrictEqual(log.filter((line) => line === "make sync"), ["make sync"], "the reconciler was made twice");
});

test("`#624 voice-page-modularity`: Help's way in shows the screen, then opens the entry asked for; the stored screen values are the device's copies", () => {
  const { mountHelpScreen, createStoredScreen } = VIEW_SETTINGS;
  const log = [];
  const screen = mountHelpScreen({
    showScreen: (name) => log.push(`screen ${name}`),
    openAt: (slug) => log.push(`open ${slug}`),
  });
  screen.showHelp("keyboard");
  screen.showHelp(null);
  assert.deepStrictEqual(log, ["screen help", "open keyboard", "screen help", "open null"]);
  const stored = createStoredScreen({ msgScale: () => 125, readingWidth: () => 72 });
  assert.equal(stored.storedMsgScale(), 125);
  assert.equal(stored.storedReadingWidth(), 72);
});

test("`#1398 reading-width-handle-gate`: the handle shows only past the default reading width, and full width applies below it", () => {
  const { readingGripShown, effectiveReadingWidth, READING_GRIP_THRESHOLD_CH } = VIEW_SETTINGS;
  // The threshold is the default reading width (DEFAULT_READING_CH, 72ch): a column no
  // wider than the width a reader gets by default reads best at full width.
  assert.equal(READING_GRIP_THRESHOLD_CH, 72);
  // Fed measured widths: at 8px per character the threshold stands at 576px.
  assert.equal(readingGripShown(575, 8), false, "a column under the threshold still showed the handle");
  assert.equal(readingGripShown(576, 8), false, "a column exactly at the threshold showed the handle");
  assert.equal(readingGripShown(577, 8), true, "a column past the threshold hid the handle");
  // The same decision at a larger type size (10px per character, threshold 720px): a
  // squeezed 700px column hides it, a 900px one-column desk keeps it.
  assert.equal(readingGripShown(700, 10), false);
  assert.equal(readingGripShown(900, 10), true);
  // An unmeasured column (no layout yet, or a refused measurement) keeps the handle.
  assert.equal(readingGripShown(0, 8), true);
  assert.equal(readingGripShown(1200, 0), true);
  assert.equal(readingGripShown(Number.NaN, 8), true);
  // Hysteresis: below the threshold the handle hides from either state, above the
  // threshold plus its band it shows from either state, and inside the band (72–78ch,
  // 720–780px at 10px per character) it keeps the state it had — so a column resized
  // through the boundary does not flip the control on the way.
  const { readingGripVisibility, READING_GRIP_HYSTERESIS_CH } = VIEW_SETTINGS;
  assert.equal(READING_GRIP_HYSTERESIS_CH, 6);
  assert.equal(readingGripVisibility(true, 700, 10), false, "a clearly squeezed column kept the handle");
  assert.equal(readingGripVisibility(false, 700, 10), false);
  assert.equal(readingGripVisibility(false, 900, 10), true, "a clearly wide column did not earn the handle back");
  assert.equal(readingGripVisibility(true, 900, 10), true);
  assert.equal(readingGripVisibility(true, 750, 10), true, "a column in the band lost the handle it had");
  assert.equal(readingGripVisibility(false, 750, 10), false, "a column in the band gained a handle it had not earned");
  assert.equal(readingGripVisibility(true, 720, 10), true, "a column at the threshold flipped");
  assert.equal(readingGripVisibility(false, 780, 10), false, "a column at the band's top flipped");
  assert.equal(readingGripVisibility(false, 0, 10), false, "an unmeasured column changed state");
  assert.equal(readingGripVisibility(true, Number.NaN, 10), true, "an unmeasured column changed state");
  // Below the threshold the remembered narrower width is kept, not applied: the column
  // is applied at full width (the widest allowed, which cannot cap a column this size).
  assert.equal(effectiveReadingWidth(54, false), 120, "a squeezed column still applied the remembered width");
  assert.equal(effectiveReadingWidth(54, true), 54, "a wide column did not apply the remembered width");
  assert.equal(effectiveReadingWidth(120, false), 120);
});

test("`#1398 reading-width-handle-gate`: the handle stands on the message column's own edge, never the screen's middle", () => {
  const { readingGripLeft } = VIEW_SETTINGS;
  // Docked at 1280px: the screen runs 0..1280, the message column ends at 700 and the
  // Discuss panel owns the rest. Centred on the screen the handle would stand at
  // 640 + half the reading width, inside the panel; on the column it stands at 700.
  assert.equal(readingGripLeft({ left: 0, right: 700, width: 700 }, { left: 0, width: 1280 }), 700);
  // The same column with the screen offset (a safe-area inset): only the screen's
  // left edge moves the answer.
  assert.equal(readingGripLeft({ left: 24, right: 724, width: 700 }, { left: 24, width: 1280 }), 700);
  // Two columns on a wide screen: the main column's edge, not the second column's.
  assert.equal(readingGripLeft({ left: 0, right: 1720, width: 1720 }, { left: 0, width: 3440 }), 1720);
});
