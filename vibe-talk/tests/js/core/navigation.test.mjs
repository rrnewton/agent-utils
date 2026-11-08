// Tests for web/src/core/navigation/ on its own: the module built as the page build builds it and
// held directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over
// the same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** Where the reader is: the selection's one owner (web/src/core/navigation). */
const NAVIGATION = await moduleForTests("src/core/navigation/index.ts");

// --- the selection's owner (`#624 voice-page-modularity`) ----------------------------------------

test("`#624 voice-page-modularity`: the selection starts on no channel, and every choice reaches every subscriber in order, read-only", () => {
  const navigation = NAVIGATION.createNavigation();
  assert.deepStrictEqual(navigation.snapshot(), { channelId: "" });
  const heard = [];
  navigation.subscribe((snapshot) => heard.push(["first", snapshot.channelId]));
  const stop = navigation.subscribe((snapshot) => heard.push(["second", snapshot.channelId]));
  navigation.selectChannel("200");
  navigation.selectChannel("200");
  stop();
  navigation.selectChannel("300");
  assert.deepStrictEqual(heard, [["first", "200"], ["second", "200"], ["first", "200"], ["second", "200"], ["first", "300"]],
    "a choice of the channel already chosen must still redraw what shows it");
  const snapshot = navigation.snapshot();
  assert.equal(snapshot.channelId, "300");
  assert.throws(() => { "use strict"; snapshot.channelId = "400"; }, TypeError, "a snapshot could be written to");
  assert.equal(navigation.snapshot().channelId, "300");
});

// --- places and their addresses (`#423 back-key-consistency`, `#462 webapp-deep-links`) ----------

test("`#624 voice-page-modularity`: a place's key ignores where a thread came from, and Settings, the list and a list's thread are over another", () => {
  const { visitKey, placeIsOver } = NAVIGATION;
  const place = (fields) => ({ screen: "main", channel: "C1", layout: "one", view: "main", thread: null, origin: null, side: null, ...fields });
  assert.equal(visitKey(null), "");
  assert.equal(visitKey(place({ view: "thread", thread: "T1", origin: "list" })), visitKey(place({ view: "thread", thread: "T1", origin: "main" })));
  assert.equal(visitKey({ screen: "help", channel: "C1", under: place({}), from: "keys", entry: null }), "help:keys:one:main::");
  assert.equal(placeIsOver(place({})), false);
  assert.equal(placeIsOver(place({ layout: "list" })), true);
  assert.equal(placeIsOver(place({ layout: "two", side: null })), false);
  assert.equal(placeIsOver(place({ layout: "two", side: "T1" })), true);
  assert.equal(placeIsOver(place({ view: "thread", thread: "T1", origin: "list" })), true);
  assert.equal(placeIsOver({ screen: "settings", channel: "C1", under: null, from: null, entry: null }), true);
});

test("`#624 voice-page-modularity`: the visits note a new place after the one the reader is at, drop what was ahead, and start afresh on another channel", () => {
  const visits = NAVIGATION.createVisits();
  const place = (fields) => ({ screen: "main", channel: "C1", layout: "one", view: "main", thread: null, origin: null, side: null, ...fields });
  assert.deepStrictEqual(visits.position(), { at: -1, length: 0 });
  visits.note(null);
  assert.deepStrictEqual(visits.position(), { at: -1, length: 0 }, "no place (the call's view) is noted as none");
  visits.note(place({}));
  visits.note(place({ view: "flat" }));
  visits.note(place({ view: "flat" }));
  assert.deepStrictEqual(visits.position(), { at: 1, length: 2 }, "the same place twice is noted once");
  const shown = [];
  const go = (to) => { shown.push(to.view); visits.note(to); };
  assert.equal(visits.walk(-1, NAVIGATION.visitKey(place({ view: "flat" })), () => true, go), true);
  assert.deepStrictEqual(shown, ["main"]);
  assert.equal(visits.arriving(), false, "arriving holds only while the walk shows the place");
  assert.deepStrictEqual(visits.position(), { at: 0, length: 2 }, "a place landed on by a walk is noted as none");
  visits.note(place({ layout: "list", view: null }));
  assert.deepStrictEqual(visits.position(), { at: 1, length: 2 }, "a new place drops the places ahead");
  visits.note(place({ channel: "C2" }));
  assert.deepStrictEqual(visits.position(), { at: 0, length: 1 }, "another channel starts afresh");
});

test("`#624 voice-page-modularity`: a close onto the place before is a step back, a walk passes what cannot be shown and where the reader is, and the visits keep PLACES_KEPT", () => {
  const { createVisits, visitKey, PLACES_KEPT } = NAVIGATION;
  const place = (fields) => ({ screen: "main", channel: "C1", layout: "one", view: "main", thread: null, origin: null, side: null, ...fields });
  const visits = createVisits();
  visits.note(place({}));
  visits.note(place({ layout: "list", view: null }));
  visits.note(place({}));
  assert.deepStrictEqual(visits.position(), { at: 0, length: 2 }, "the list closing onto Main is a step back, the list kept ahead");
  const arrivals = [];
  const go = (to) => { arrivals.push([to.layout, visits.arriving()]); visits.note(to); };
  assert.equal(visits.walk(1, visitKey(place({})), (to) => to.layout !== "list", go), false, "nothing showable ahead");
  assert.equal(visits.walk(-1, visitKey(place({})), () => true, go), false, "nothing behind");
  assert.equal(visits.walk(1, visitKey(place({})), () => true, go), true);
  assert.deepStrictEqual(arrivals, [["list", true]]);
  const long = createVisits();
  for (let at = 0; at < PLACES_KEPT + 5; at += 1) long.note(place({ view: "thread", thread: `T${at}`, origin: "main" }));
  assert.deepStrictEqual(long.position(), { at: PLACES_KEPT - 1, length: PLACES_KEPT });
});

test("`#624 voice-page-modularity`: a link is read from the address once, counts the reader's choices from then, and signing out forgets it", () => {
  let search = "?c=C1&v=Thread&t=T1&m=discord%3AC1%3AM1";
  let arriving = false;
  let reads = 0;
  const links = NAVIGATION.createDeepLinks({ search: () => { reads += 1; return search; }, arriving: () => arriving });
  links.noteReaderMove();
  assert.equal(links.readerMoved(), false, "choices before the link was read are not counted against it");
  const wanted = links.wanted();
  assert.deepStrictEqual({ ...wanted, message: { ...wanted.message } },
    { channel: "C1", view: "thread", thread: "T1", message: { provider: "discord", channel: "C1", message: "M1", thread: null } });
  search = "?c=C2";
  assert.equal(links.wanted(), wanted, "the address is read once a load");
  assert.equal(reads, 1);
  arriving = true;
  links.noteReaderMove();
  arriving = false;
  links.arrive(() => links.noteReaderMove());
  assert.equal(links.showing(), false, "an arrival ends with its run");
  assert.equal(links.readerMoved(), false, "an arrival, or Back and Forward, is no choice");
  links.noteReaderMove();
  assert.equal(links.readerMoved(), true);
  assert.equal(links.readerMoves(), 2);
  links.setFocus(wanted.message);
  links.setApplying(true);
  const epoch = links.epoch();
  links.signOut();
  assert.equal(links.wanted(), null);
  assert.equal(links.focus(), null);
  assert.equal(links.applied(), true, "no apply runs after signing out");
  assert.equal(links.readerMoved(), false);
  assert.notEqual(links.epoch(), epoch, "an apply waiting from before signing out sees another epoch");
});

test("`#624 voice-page-modularity`: a plain open, or an address the host cannot give, is no link", () => {
  assert.equal(NAVIGATION.createDeepLinks({ search: () => "", arriving: () => false }).wanted(), null);
  assert.equal(NAVIGATION.createDeepLinks({ search: () => { throw new Error("no window"); }, arriving: () => false }).wanted(), null);
  assert.equal(NAVIGATION.createDeepLinks({ search: () => "?v=nowhere", arriving: () => false }).wanted(), null, "an unknown view says nothing");
});

test("`#624 voice-page-modularity`: a jump is a level over what was open, a later ticket ends one under way, and signing out forgets them", () => {
  const { createLinkJumps, ownLevel } = NAVIGATION;
  const jumps = createLinkJumps();
  const spot = { place: null, column: "main", pinned: false, anchor: null, offset: 0, focus: null };
  assert.deepStrictEqual(jumps.levels(["list"]), ["list"], "no jump, no level");
  const jump = { spot, below: ["list"], landed: null };
  const ticket = jumps.take(jump);
  assert.equal(jumps.current(ticket), true);
  assert.deepStrictEqual(jumps.levels(["thread", "menu:row", "settings"]), ["list", "jump", "menu:row", "settings"],
    "while under way only what the reader opens is a level");
  jump.landed = ["thread"];
  assert.deepStrictEqual(jumps.levels(["thread", "help"]), ["list", "jump", "help"]);
  jumps.returnStarted();
  assert.deepStrictEqual(jumps.levels([]), ["list", "jump"]);
  assert.deepStrictEqual(jump.landed, ["thread"], "a return putting a place back forgets nothing it opened");
  jumps.returnEnded();
  jumps.levels([]);
  assert.deepStrictEqual(jump.landed, [], "a level the jump opened that was closed is forgotten");
  const back = jumps.nextTicket();
  assert.equal(jumps.current(ticket), false, "a return ends the jump under way");
  assert.equal(jumps.undo(), jump);
  assert.equal(jumps.holds(jump), false);
  jumps.take({ spot, below: [], landed: null });
  jumps.forget();
  assert.equal(jumps.top(), undefined);
  assert.equal(jumps.current(back + 1), false, "forgetting ends the jump under way");
  assert.equal(ownLevel("menu:x") && ownLevel("help") && !ownLevel("thread"), true);
});

test("`#1082 documents-back-chain`: a document jump absorbs its document, and a second document is a level over it", () => {
  const { createLinkJumps } = NAVIGATION;
  const jumps = createLinkJumps();
  const spot = { place: null, column: "main", pinned: false, anchor: "m1", offset: 0, focus: null };
  // Channel -> document A: the jump carries its document, lands on its level.
  const jump = { spot, below: [], landed: null, doc: "doc-a" };
  jumps.take(jump);
  assert.deepStrictEqual(jumps.levels(["document"]), ["jump"],
    "while the document opens, it is the jump's, not a level of its own");
  jump.landed = ["document"];
  assert.deepStrictEqual(jumps.levels(["document"]), ["jump"], "the landed document stays the jump's one level");
  // Document B opens from A: the chain is two document levels deep, one over the jump.
  assert.deepStrictEqual(jumps.levels(["document", "document"]), ["jump", "document"],
    "the second document is not a level over the jump");
  assert.deepStrictEqual(jumps.levels(["document", "document", "menu:pace"]), ["jump", "document", "menu:pace"],
    "a menu is not on top of the chain");
  // B closed: back to the jump alone, its document still named for the return.
  assert.deepStrictEqual(jumps.levels(["document"]), ["jump"]);
  assert.equal(jumps.undo(), jump);
  assert.equal(jump.doc, "doc-a");
});

test("`#624 voice-page-modularity`: the Back entries push one per level opened, go back through those closed, and a Back closes down to its entry", async () => {
  const { createBackStack, BACK_STATE_KEY, BACK_TRAVERSAL_MS } = NAVIGATION;
  const entries = [{ state: null }];
  let at = 0;
  const history = {
    get state() { return entries[at].state; },
    pushState(state) { entries.splice(at + 1); entries.push({ state }); at += 1; },
    replaceState(state) { entries[at] = { state }; },
    back() { this.go(-1); },
    go(delta) { traversals.push(delta); },
  };
  const traversals = [];
  const timers = [];
  let levels = [];
  const closed = [];
  const notes = [];
  const stack = createBackStack({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: (h) => { timers[h - 1] = null; } },
    session: "s1",
    history: () => history,
    index: () => at,
    mayPush: () => true,
    levels: () => [...levels],
    close: (tag) => { closed.push(tag); levels = levels.slice(0, -1); },
    failed: (message) => notes.push(["failed", message]),
    notePlace: () => notes.push("place"),
    syncAddress: () => notes.push("address"),
    applyLink: () => notes.push("link"),
  });
  const turn = () => new Promise((resolve) => setImmediate(resolve));
  stack.schedule();
  await turn();
  assert.deepStrictEqual(notes, [], "nothing is kept in step before the start");
  assert.equal(stack.start(), true);
  assert.equal(history.scrollRestoration, "manual");
  assert.deepStrictEqual(history.state, { [BACK_STATE_KEY]: { session: "s1", depth: 0, tag: "top" } });
  levels = ["list", "thread"];
  stack.schedule();
  stack.schedule();
  await turn();
  assert.deepStrictEqual(notes, ["place", "address", "link"], "two asks in one task are one sync");
  assert.deepStrictEqual(entries.map((entry) => entry.state[BACK_STATE_KEY].tag), ["top", "list", "thread"]);
  levels = ["list"];
  stack.schedule();
  await turn();
  assert.deepStrictEqual(traversals, [-1], "a level closed by its own control is gone back through");
  assert.equal(timers[0].ms, BACK_TRAVERSAL_MS);
  at = 1;
  stack.pop(history.state);
  assert.deepStrictEqual(closed, [], "the page's own going-back closes nothing more");
  assert.equal(timers[0], null, "landing stops the wait");
  stack.pop(entries[0].state);
  at = 0;
  assert.deepStrictEqual(closed, ["list"], "a Back closes down to the depth of the entry it lands on");
});

test("`#624 voice-page-modularity`: the Back entries take an earlier load's entry at its depth, clamped by the browser's index", () => {
  const { createBackStack, BACK_STATE_KEY } = NAVIGATION;
  let state = { [BACK_STATE_KEY]: { session: "s0", depth: 5, tag: "thread" } };
  const history = { get state() { return state; }, pushState() {}, replaceState(next) { state = next; }, back() {}, go() {} };
  const stack = createBackStack({
    clock: { setTimeout: () => 0, clearTimeout: () => {} }, session: "s1", history: () => history, index: () => 2,
    mayPush: () => true, levels: () => [], close: () => {}, failed: () => {}, notePlace: () => {}, syncAddress: () => {}, applyLink: () => {},
  });
  stack.start();
  assert.deepStrictEqual(state, { [BACK_STATE_KEY]: { session: "s1", depth: 2, tag: "thread" } });
  assert.equal(createBackStack({
    clock: { setTimeout: () => 0, clearTimeout: () => {} }, session: "s1", history: () => null, index: () => null,
    mayPush: () => true, levels: () => [], close: () => {}, failed: () => {}, notePlace: () => {}, syncAddress: () => {}, applyLink: () => {},
  }).start(), false, "a host with no history has nothing to listen to");
});

test("`#624 voice-page-modularity`: the layout is the window's, else the device's, and remembers what two columns and the list were chosen from", () => {
  const { createLayoutChoice, COLUMNS_KEY, LAYOUT_BEFORE_KEY } = NAVIGATION;
  const storage = (map, refuse = false) => ({
    getItem: (key) => { if (refuse) throw new Error("SecurityError"); return map.has(key) ? map.get(key) : null; },
    setItem: (key, value) => { if (refuse) throw new Error("QuotaExceededError"); map.set(key, value); },
    removeItem: (key) => map.delete(key),
  });
  const device = new Map([[COLUMNS_KEY, "two"]]);
  const windowed = new Map();
  const fresh = createLayoutChoice({ device: storage(device), window: storage(windowed) });
  assert.equal(fresh.chosen(), "two", "a new window opens in the device's layout");
  assert.equal(fresh.beforeColumns(), "one");
  assert.deepStrictEqual(fresh.listBefore(), { layout: "one" });
  fresh.noteListBefore(() => assert.fail("two columns need no view"));
  assert.deepStrictEqual(fresh.listBefore(), { layout: "two", columns: "one" });
  fresh.choose("list");
  assert.equal(fresh.beforeColumns(), "list", "a one-column layout is what two columns go back to");
  assert.equal(device.get(COLUMNS_KEY), "list");
  assert.deepStrictEqual(JSON.parse(windowed.get(LAYOUT_BEFORE_KEY)), { columns: "list", list: { layout: "two", columns: "one" } });
  fresh.noteListBefore(() => null);
  assert.deepStrictEqual(fresh.listBefore(), { layout: "two", columns: "one" }, "a thread opened from the list is not noted");
  const one = { layout: "one", channel: "C1", view: "thread", thread: "T1", origin: "main" };
  fresh.noteListBefore(() => one);
  fresh.choose("one");
  const reloaded = createLayoutChoice({ device: storage(new Map()), window: storage(windowed) });
  assert.equal(reloaded.chosen(), "one", "a reload of the window keeps its own layout");
  assert.deepStrictEqual(reloaded.listBefore(), one);
  reloaded.restoreBeforeColumns("bogus");
  assert.equal(reloaded.beforeColumns(), "one");
  reloaded.restoreBeforeColumns("list");
  assert.equal(reloaded.beforeColumns(), "list");
  windowed.set(LAYOUT_BEFORE_KEY, JSON.stringify({ list: { layout: "one", channel: "C1", view: "thread", thread: "", origin: "main" } }));
  assert.deepStrictEqual(createLayoutChoice({ device: storage(new Map()), window: storage(windowed) }).listBefore(), { layout: "one" },
    "a kept way back that cannot be gone back to is none");
  const refused = createLayoutChoice({ device: storage(new Map(), true), window: storage(new Map(), true) });
  assert.equal(refused.chosen(), "one");
  refused.choose("two");
  assert.equal(refused.chosen(), "two", "refused storage still honours the choice for the session");
});

test("`#624 voice-page-modularity`: a place reads back from the query it writes, and a bad escape or an absurd value says nothing", () => {
  const { deepLinkParams, deepLinkPart, deepLinkQuery, DEEP_LINK_VIEWS } = NAVIGATION;
  const ref = { provider: "slack", channel: "C 1", message: "M1", thread: "T1" };
  const query = deepLinkQuery({ screen: "main", channel: "C 1", layout: "one", view: "thread", thread: "T1", origin: null, side: null }, ref);
  assert.equal(query, "c=C%201&v=thread&t=T1&m=slack%3AC%25201%3AM1%3AT1", "the ref's parts are escaped, then the whole value again");
  const params = deepLinkParams(`?${query}`);
  assert.equal(params.get("c"), "C 1");
  assert.equal(params.get("v"), "thread");
  assert.ok(DEEP_LINK_VIEWS.includes(params.get("v")));
  assert.equal(deepLinkQuery({ screen: "main", channel: "C1", layout: "list", view: null, thread: null, origin: null, side: null }, ref), "c=C1&v=threads",
    "a ref to another channel is not carried");
  assert.equal(deepLinkQuery({ screen: "main", channel: "C1", layout: "one", view: "flat", thread: null, origin: null, side: null }), "c=C1&v=all");
  const odd = deepLinkParams("a=%E0%A4%A&b=x+y&c&d=" + "z".repeat(600));
  assert.equal(odd.has("a"), false);
  assert.equal(odd.get("b"), "x y");
  assert.equal(deepLinkPart(odd, "c"), null);
  assert.equal(deepLinkPart(odd, "d"), null);
  assert.equal(deepLinkPart(odd, "b"), "x y");
});

test("`#624 voice-page-modularity`: the UI-state record read back, checked, brought forward and merged, and the views chosen per channel", () => {
  const { createUiState, UI_STATE_KEY, UI_STATE_CHANNELS } = NAVIGATION;
  const store = (entries = {}) => {
    const kept = new Map(Object.entries(entries));
    return { kept, getItem: (key) => (kept.has(key) ? kept.get(key) : null), setItem: (key, value) => kept.set(key, value), removeItem: (key) => kept.delete(key) };
  };
  const record = (fields) => JSON.stringify({ v: 2, identity: "me", view: "discord", todo: false, channel: "C1", place: null, channels: [], ...fields });
  let identity = "me";
  const owner = (device, window) => createUiState({
    deviceStorage: () => device, windowStorage: () => window, identity: () => identity,
    viewKey: (view, thread) => (thread ? `${view}:${thread}` : view),
  });
  // Nothing kept: nothing to reopen on, and settled at once.
  const empty = owner(store(), store());
  assert.equal(empty.saved(), null);
  assert.equal(empty.settled(), true);
  // The window's own record first, the device's choices behind its own, the device's read mode.
  const device = store({ [UI_STATE_KEY]: record({ todo: true, read: "hide", channels: [{ channel: "C2", channelView: "main", thread: null }] }) });
  const window = store({ [UI_STATE_KEY]: record({ channels: [{ channel: "C1", channelView: "flat", thread: null }, { channel: "", channelView: "main", thread: null }] }) });
  const merged = owner(device, window);
  assert.equal(merged.settled(), false);
  assert.equal(merged.saved().todo, true);
  assert.equal(merged.saved().read, "hide");
  assert.deepStrictEqual(merged.saved().channels.map((one) => one.channel), ["C1", "C2"], "a damaged choice was kept, or the device's were lost");
  // Another credential's record, or one that does not parse, is dropped from its store.
  const theirs = store({ [UI_STATE_KEY]: record({ identity: "them" }) });
  assert.equal(owner(theirs, store()).saved(), null);
  assert.equal(theirs.kept.has(UI_STATE_KEY), false);
  const broken = store({ [UI_STATE_KEY]: "{" });
  owner(broken, store());
  assert.equal(broken.kept.has(UI_STATE_KEY), false);
  // A version 1 record is brought forward: the channel's view kept where chosen, the place under its view key.
  const old = store({ [UI_STATE_KEY]: JSON.stringify({ v: 1, identity: "me", view: "discord", todo: false, channel: "C3",
    channelView: "thread", chosen: true, thread: { id: "T1", origin: "flat", summary: null }, place: { atNewest: false, anchor: "9", offset: 4 } }) });
  const forward = owner(old, store()).saved();
  assert.equal(forward.v, 2);
  assert.equal(forward.place.view, "thread:T1");
  assert.deepStrictEqual(forward.channels.map((one) => one.channel), ["C3"]);
  // The views chosen: the newest first, one per channel, a change of credential forgetting them.
  const choices = owner(store(), store());
  for (let at = 0; at < UI_STATE_CHANNELS + 3; at += 1) choices.remember({ channel: `C${at}`, channelView: "main", thread: null });
  choices.remember({ channel: "C5", channelView: "flat", thread: null });
  assert.equal(choices.chosenViewsNow().length, UI_STATE_CHANNELS);
  assert.equal(choices.chosenView("C5").channelView, "flat");
  assert.equal(choices.chosenViewsNow()[0].channel, "C5");
  choices.forgetChosen((channel) => channel === "C5");
  assert.equal(choices.chosenView("C5"), undefined);
  identity = "someone else";
  assert.deepStrictEqual(choices.chosenViewsNow(), []);
  // Forgotten on signing out, in both stores.
  identity = "me";
  const kept = store({ [UI_STATE_KEY]: record({}) });
  const signedOut = owner(kept, store());
  signedOut.holdPlace({ key: "k", place: { view: "main", atNewest: true, anchor: null, offset: 0 } });
  signedOut.forget();
  assert.equal(signedOut.saved(), null);
  assert.equal(signedOut.heldPlace(), null);
  assert.equal(signedOut.settled(), true);
  assert.equal(kept.kept.has(UI_STATE_KEY), false);
});

test("`#624 voice-page-modularity`: a view entered again goes back to its offset unless it was left at the newest, and says whether it did", () => {
  const { createViewPlaces } = NAVIGATION;
  const places = createViewPlaces();
  // A first entry goes to the newest.
  assert.deepStrictEqual([places.enter("discord"), places.restored()], [null, false]);
  places.leave("discord", { top: 840, atNewest: false });
  places.leave("voice", { top: 1200, atNewest: true });
  assert.deepStrictEqual([places.enter("voice"), places.restored()], [null, false], "a view left at the newest was not taken to it");
  assert.deepStrictEqual([places.enter("discord"), places.restored()], [840, true], "a view left in its backlog was not put back");
  places.leave("discord", { top: 900, atNewest: true });
  assert.deepStrictEqual([places.enter("discord"), places.restored()], [null, false]);
});
