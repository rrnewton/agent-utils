// Tests for web/src/core/search/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's search, over the same
// module, is tested in ../page/search.mjs and ../page/shell.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** The search language and the search sent to the server (web/src/core/search). */
const SEARCH = await moduleForTests("src/core/search/index.ts");

/** A clock the test runs: timers kept until `fire` runs the due ones. */
function testClock() {
  const timers = new Map();
  let next = 1;
  return {
    timers,
    setTimeout: (run, ms) => { timers.set(next, { run, ms }); return next++; },
    clearTimeout: (handle) => { timers.delete(handle); },
    fire: () => { const due = [...timers.values()]; timers.clear(); for (const { run } of due) run(); },
  };
}

test("`#624 voice-page-modularity`: the server search waits for the text to rest, drops a late answer to an old search, and pages on", async () => {
  const { createServerSearch, SERVER_SEARCH_DELAY_MS } = SEARCH;
  const clock = testClock();
  const asked = [];
  let renders = 0;
  const answers = [];
  const owner = createServerSearch({
    clock,
    pathFor: (more, resume) => `/search?more=${more}&resume=${resume}`,
    fetchAnswer: (path) => { asked.push(path); return answers.shift()(); },
    render: () => { renders += 1; },
    guard: (fn) => { fn(); },
  });
  // The same (empty) search: drawn again, nothing asked.
  owner.schedule("", true);
  assert.equal(renders, 1);
  assert.equal(clock.timers.size, 0);
  // A new search: drawn at once as loading, asked once the text rests.
  owner.schedule("k1", true);
  assert.equal(owner.state().loading, true);
  assert.equal([...clock.timers.values()][0].ms, SERVER_SEARCH_DELAY_MS);
  // Typing on: the first wait is forgotten, and only the last search is asked.
  owner.schedule("k2", true);
  assert.equal(clock.timers.size, 1);
  const hit = (id) => ({ id, channel_id: "C" });
  answers.push(async () => ({ hits: [hit("1"), hit("2")], scopes: [{ examined: 5 }], resume_below_ms: 900 }));
  clock.fire();
  await new Promise((done) => setImmediate(done));
  assert.deepStrictEqual(asked, ["/search?more=false&resume=null"]);
  assert.deepStrictEqual(owner.state().hits.map((one) => one.id), ["1", "2"]);
  assert.equal(owner.state().examined, 5);
  assert.equal(owner.state().resume, 900);
  assert.equal(owner.state().loading, false);
  // The next page: a hit already held is not held twice.
  answers.push(async () => ({ hits: [hit("2"), hit("3")], scopes: [{ examined: 4 }], resume_below_ms: null }));
  await owner.more();
  assert.deepStrictEqual(owner.state().hits.map((one) => one.id), ["1", "2", "3"]);
  assert.equal(owner.state().examined, 9);
  assert.equal(owner.state().resume, null);
  await owner.more();
  assert.equal(asked.length, 2, "a search with no next page asked again");
  // A late answer to an old search is dropped; a failure says why.
  let release;
  answers.push(() => new Promise((done) => { release = done; }));
  const late = owner.run(false);
  owner.schedule("k3", false);
  release({ hits: [hit("9")], scopes: [], resume_below_ms: null });
  await late;
  assert.deepStrictEqual(owner.state().hits, [], "an answer to an older search was kept");
  assert.equal(owner.state().loading, false, "a search the server's copy cannot answer was left loading");
  answers.push(async () => { throw { detail: "busy" }; });
  await owner.run(false);
  assert.equal(owner.state().failed, "busy");
  // Forgotten: the same key starts afresh.
  owner.forget();
  owner.schedule("k3", true);
  assert.equal(owner.state().failed, "");
  assert.equal(owner.state().key, "k3");
});

test("`#624 voice-page-modularity`: gathering older history asks, polls while the walk goes on, says why it failed, and stops", async () => {
  const { createGathering, SEARCH_FURTHER_POLL_MS } = SEARCH;
  const clock = testClock();
  const said = [];
  const states = [];
  const gathering = createGathering({
    clock,
    prioritize: async (channel) => { said.push(["prioritize", channel]); return states.shift(); },
    fetchState: async (channel) => { said.push(["state", channel]); return states.shift(); },
    render: () => said.push(["render"]),
    grew: () => said.push(["grew"]),
    guard: (fn) => { fn(); },
  });
  states.push({ walking: true, oldest_ms: 100, complete: false });
  await gathering.start("C1");
  assert.equal(gathering.state().channel, "C1");
  assert.equal(gathering.state().state.oldest_ms, 100);
  assert.equal([...clock.timers.values()][0].ms, SEARCH_FURTHER_POLL_MS);
  // The walk reached older messages: the search is asked again from the start, and the poll goes on.
  states.push({ walking: true, oldest_ms: 50, complete: false });
  clock.fire();
  await new Promise((done) => setImmediate(done));
  assert.deepStrictEqual(said.filter(([what]) => what === "grew").length, 1);
  assert.equal(clock.timers.size, 1);
  // Nothing new and the walk over: drawn, and no more polling.
  states.push({ walking: false, oldest_ms: 50, complete: false });
  clock.fire();
  await new Promise((done) => setImmediate(done));
  assert.equal(clock.timers.size, 0);
  assert.equal(gathering.state().timer, null);
  // A server that gathers nothing for the channel says so; stop forgets it all.
  const refusing = createGathering({
    clock, prioritize: async () => { throw { code: "history_not_cached" }; }, fetchState: async () => ({}),
    render: () => {}, grew: () => {}, guard: (fn) => { fn(); },
  });
  await refusing.start("C2");
  assert.equal(refusing.state().failed, "the server does not gather history for this channel");
  assert.equal(clock.timers.size, 0, "a refused gathering polled");
  refusing.stop();
  assert.deepStrictEqual({ ...refusing.state() }, { channel: "", run: 0, state: null, timer: null, failed: "" });
});

test("`#624 voice-page-modularity`: the search field's state, and its hits marked once the typing pauses", () => {
  const { createSearchField, SEARCH_MARK_DELAY_MS } = SEARCH;
  assert.equal(SEARCH_MARK_DELAY_MS, 120);
  const timers = new Map();
  let next = 0;
  let redraws = 0;
  const field = createSearchField({
    clock: { setTimeout: (run, ms) => { next += 1; timers.set(next, { run, ms }); return next; }, clearTimeout: (h) => timers.delete(h) },
    redraw: (apply) => { redraws += 1; apply(); },
  });
  assert.deepStrictEqual([field.open(), field.query(), field.marked()], [false, "", ""]);
  field.setOpen(true);
  field.setQuery("ru");
  field.markSoon();
  field.setQuery("run");
  field.markSoon();
  assert.equal(timers.size, 1, "each key did not put the marking off");
  assert.equal([...timers.values()][0].ms, SEARCH_MARK_DELAY_MS);
  [...timers.values()][0].run();
  assert.deepStrictEqual([field.marked(), redraws], ["run", 1]);
  // Marked already: nothing armed, nothing redrawn.
  field.markSoon();
  field.markNow();
  assert.deepStrictEqual([timers.size, redraws], [0, 1]);
  // Enter: at once, and the pending wait goes.
  field.setQuery("runner");
  field.markSoon();
  field.markNow();
  assert.deepStrictEqual([field.marked(), redraws, timers.size], ["runner", 2, 0], "Enter left the wait armed");
  field.setMarked("");
  assert.equal(field.marked(), "");
});

test("`#624 voice-page-modularity`: a query is parsed once per change of its text, with its words to mark", () => {
  const queries = SEARCH.createParsedQueries();
  const first = queries.parsed("kestrel -owl");
  assert.equal(first.query, "kestrel -owl");
  assert.deepStrictEqual(first.terms, SEARCH.searchPositiveTerms(SEARCH.searchQueryParse("kestrel -owl")));
  assert.equal(queries.parsed("kestrel -owl"), first, "the same text was parsed again");
  const blank = queries.parsed("   ");
  assert.equal(blank.node, null, "spaces are no search");
  assert.notEqual(queries.parsed("kestrel -owl"), first, "a change of text and back parses afresh");
});

test("`#624 voice-page-modularity`: the Links filter is the search field's, off until turned on", () => {
  const field = SEARCH.createSearchField({ clock: { setTimeout: () => 0, clearTimeout: () => {} }, redraw: (apply) => apply() });
  assert.equal(field.links(), false);
  field.setLinks(true);
  assert.equal(field.links(), true);
  field.setOpen(true);
  field.setQuery("deploy");
  assert.equal(field.links(), true, "the text and the filter are separate");
  field.setLinks(false);
  assert.equal(field.links(), false);
});

test("`#624 voice-page-modularity`: the cards' walk reads All back within its bounds, counts its finds, and says how it ended", async () => {
  const { createRepliesWalk } = SEARCH;
  let now = 0;
  let cards = ["T0"];
  let offered = true;
  let holding = true;
  const reads = [];
  const pages = [];
  const said = [];
  let renders = 0;
  const walks = createRepliesWalk({
    now: () => now, pages: 3, budgetMs: 15000, hits: 2, tapMs: 500,
    offered: () => offered,
    endOthers: () => [],
    cursor: () => null,
    key: () => "k1",
    canon: () => "store",
    channel: () => "C1",
    cardsFound: () => cards,
    controller: () => null,
    matcher: () => (message) => message.text.includes("kestrel"),
    threadOf: (message) => message.thread,
    isRoot: (message) => message.root === true,
    allLoaded: () => false,
    holds: () => holding,
    heldNow: () => "held",
    read: async (channel, before) => { reads.push([channel, before]); return pages.shift(); },
    fold: (page, newest, held) => { if (newest) assert.equal(held, "held"); },
    render: () => { renders += 1; },
    failed: () => said.push("failed"),
    ending: (walk, reason) => `${reason}: ${walk.found} found, ${walk.matched.size} matched, ${walk.read} read`,
  });
  pages.push(
    { messages: [], has_more: true, next_before: "c1" },
    { messages: [{ thread: "T1", text: "kestrel", root: false }], has_more: true, next_before: "c2" },
    { messages: [{ thread: "T2", text: "owl", root: false }], has_more: true, next_before: "c3" },
  );
  const going = walks.search();
  assert.notEqual(walks.current(), null);
  assert.equal(await walks.search(), undefined, "one walk at a time");
  cards = ["T0", "T1", "T2"];
  await going;
  assert.deepStrictEqual(reads, [["C1", null], ["C1", "c1"], ["C1", "c2"]], "All's newest first where it has no cursor, then back");
  assert.equal(walks.current(), null);
  assert.equal(walks.noteFor(() => "k1"), "done: 2 found, 1 matched, 2 read", "it ended on the page bringing its second find");
  assert.equal(walks.noteFor(() => "k2"), "", "a change of search forgets the note");
  assert.ok(renders > 0);
  offered = false;
  await walks.search();
  assert.equal(walks.current(), null, "nothing where the cards do not offer it");
  offered = true;
  pages.push({ messages: [], has_more: true, next_before: "c1" });
  let release;
  pages.push(new Promise((resolve) => { release = resolve; }));
  const stopped = walks.search();
  await new Promise((resolve) => setImmediate(resolve));
  walks.pressed();
  assert.equal(walks.current().stopping, false, "a press in the walk's first moments is no Stop");
  now = 600;
  walks.pressed();
  assert.equal(walks.current().stopping, true);
  release({ messages: [], has_more: true, next_before: "c9" });
  await stopped;
  assert.match(walks.noteFor(() => "k1"), /^stopped:/);
  pages.push({ messages: [], has_more: true, next_before: "c1" });
  holding = true;
  const taken = walks.search();
  holding = false;
  await taken;
  assert.equal(walks.noteFor(() => "k1"), "", "a walk something else took over says nothing");
  holding = true;
  pages.push(Promise.reject(new Error("refused")));
  await assert.rejects(walks.search(), /refused/);
  assert.deepStrictEqual(said, ["failed"]);
  assert.match(walks.noteFor(() => "k1"), /^failed:/);
});
