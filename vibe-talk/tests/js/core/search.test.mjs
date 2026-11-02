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
