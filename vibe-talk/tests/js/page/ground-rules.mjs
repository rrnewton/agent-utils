// The app page's tests: the suite's own rules, the numbers the page is tuned by, and the page's names.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/ground-rules.mjs.

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { loadTool } from "../../../web/build.mjs";
import { WEB, shellScripts } from "../../../web/page_sources.mjs";

import { CHANNEL, PAGE_CODE, PAGE_SCRIPTS, PRELUDE, SCRIPT, SCRIPT_FILE, SERVER_API, TUNING_BANDS,
  brokenScript, message, newPage, showDiscord, signIn, sourceConstant, test } from "./fixture.mjs";

test("`#624 voice-page-modularity`: nothing is awaited at the top of a feature's file", () => {
  // See the loads near PRELUDE in ./fixture.mjs: a feature's tests are defined as its file loads, so
  // a test defined above a top-level await may run while the file waits there, before the helpers
  // below it exist; and the files after it would load while earlier tests run.
  const here = dirname(fileURLToPath(import.meta.url));
  const files = readdirSync(here).filter((name) => name.endsWith(".mjs") && name !== "fixture.mjs");
  assert.ok(files.length > 1, "the suite has no feature files");
  for (const name of files) {
    const lines = readFileSync(join(here, name), "utf8").split("\n");
    assert.ok(lines.some((line) => /^test\(/.test(line)), `${name} defines no test`);
    const late = lines.filter((line) => /^(?:const|let|var) \S+ = (?:\(\s*)?await\b|^await\b/.test(line));
    assert.deepStrictEqual(late, [], name);
  }
});

test("`#624 voice-page-modularity`: every page here runs the scripts the shell loads, in its order, and a control's needle is counted", () => {
  // `#624 voice-page-modularity`: the shell loads the page's script alone; the contract's validators
  // come to it through the prelude, with the other modules.
  assert.deepStrictEqual(PAGE_SCRIPTS, [SCRIPT_FILE]);
  // The tags, not the prose: a commented-out tag loads nothing, and a version query is not the name.
  assert.deepStrictEqual(shellScripts('<!-- <script src="/old.js"></script> --><script src="/a.js?v=1"></script>'), ["a.js"]);
  assert.throws(() => shellScripts("<script>inline();</script>"), /names no local file/);
  assert.throws(() => shellScripts('<script src="//elsewhere.example/a.js"></script>'), /names no local file/);
  // A needle that occurs twice is refused unless the control says it means the first of two.
  assert.throws(() => brokenScript("  if (chosen) rememberChosenView(chosen);", ""), /different number of times/);
});

test("every number this page is tuned by stays inside a band that says what would be wrong", () => {
  for (const [name, [low, high, why]] of Object.entries(TUNING_BANDS)) {
    const value = sourceConstant(name);
    assert.ok(
      value >= low && value <= high,
      `${name} is ${value}, outside ${low}..${high}: ${why}`
    );
  }
  // The two that are a RELATION rather than a range. A default outside the offered range is a
  // width the reader cannot get back to once they have moved the handle.
  assert.ok(
    sourceConstant("MIN_READING_CH") < sourceConstant("MAX_READING_CH"),
    "the narrowest column the reader may choose is not narrower than the widest"
  );
  assert.ok(
    sourceConstant("DEFAULT_READING_CH") >= sourceConstant("MIN_READING_CH") &&
      sourceConstant("DEFAULT_READING_CH") <= sourceConstant("MAX_READING_CH"),
    "the default reading width is not one the reader is allowed to choose"
  );
});

test("...and the table covers EVERY constant, so a new one cannot escape by being new", () => {
  // This is the part that generalises. Pinning the constants that happened to be noticed leaves
  // the next one unpinned, and the next one is exactly where the same defect comes back.
  const declared = [...PAGE_CODE.matchAll(/^const ([A-Z][A-Z0-9_]*) = (\d+);$/gm)].map(
    (m) => m[1]
  );
  assert.ok(declared.length >= 13, `only ${declared.length} constants found — the scan is broken`);
  const unpinned = declared.filter((name) => !(name in TUNING_BANDS));
  assert.deepStrictEqual(
    unpinned,
    [],
    `the page tunes itself by ${unpinned.join(", ")} and nothing here says what value of ` +
      "them would be wrong, so widening one of them is a change no test can see"
  );
  const gone = Object.keys(TUNING_BANDS).filter((name) => !declared.includes(name));
  assert.deepStrictEqual(gone, [], `the table pins ${gone.join(", ")}, which no longer exist`);
});

// --- the page's own names, and the browser's (`#624 voice-page-modularity`) ----------------------

/**
 * The browser's globals, as the pinned TypeScript's DOM declarations name them (every `declare var`,
 * `function`, `const` and `let` in lib.dom.d.ts), and the ones the page's browsers have that those
 * declarations do not yet: `navigation`, the Navigation API (Chromium), which `historyIndex` reads.
 */
const DOM_GLOBALS = new Set([...readFileSync(join(WEB, "..", "contract", "node_modules", "typescript", "lib", "lib.dom.d.ts"),
  "utf8").matchAll(/^declare (?:var|function|const|let) (\w+)/gm)].map((found) => found[1]).concat(["navigation"]));

test("`#624 voice-page-modularity`: no top-level name of the legacy script or its prelude hides a browser global", () => {
  // A classic script's top-level function or var becomes a property of `window`, over the
  // browser's own: a `function navigation()` in voice.js replaced the Navigation API that
  // `historyIndex` reads `window.navigation` for, which only a real browser showed. The prelude's
  // constants hide a global from every unqualified reference in the page as surely.
  const ts = loadTool("typescript");
  const file = ts.createSourceFile("voice.js", SCRIPT, ts.ScriptTarget.Latest, true, ts.ScriptKind.JS);
  const names = new Set(PRELUDE ? PRELUDE.names : []);
  for (const statement of file.statements) {
    if ((ts.isFunctionDeclaration(statement) || ts.isClassDeclaration(statement)) && statement.name) names.add(statement.name.text);
    if (ts.isVariableStatement(statement)) {
      for (const declaration of statement.declarationList.declarations) {
        if (ts.isIdentifier(declaration.name)) names.add(declaration.name.text);
      }
    }
  }
  assert.ok(names.size > 1000 && DOM_GLOBALS.size > 500 && DOM_GLOBALS.has("navigation") && names.has("selectedChannelId"),
    "the scan found too few names to mean anything");
  // `stop` (the call's teardown) has hidden `window.stop` for as long as the page has had calls, and
  // nothing in the page means the browser's; it is the one name allowed, and only it.
  const KNOWN = new Set(["stop"]);
  const hiding = [...names].filter((name) => DOM_GLOBALS.has(name) && !KNOWN.has(name));
  assert.deepStrictEqual(hiding, [], `these page names hide the browser's globals of the same name: ${hiding.join(", ")}`);
  for (const name of KNOWN) assert.ok(names.has(name), `${name} no longer needs its exception: take it off the list`);
});

test("`#624 voice-page-modularity`: the bar's picker shows the selection, and a value put on it with no change chooses nothing", async () => {
  const other = { id: "1110000000000000002", label: "build noise", writable: false };
  const page = newPage();
  page.channels = [{ ...CHANNEL }, other];
  await signIn(page);
  await showDiscord(page, [message({ content: "hello" })]);
  assert.equal(page.el("discord-channel").value, CHANNEL.id);
  // A value with no change event, which no reader can make in a browser: the selection stays.
  page.el("discord-channel").value = other.id;
  assert.equal(page.pure("selectedChannelId()"), CHANNEL.id);
  // The reader's choice, with its change: the selection first, then the picker shows it.
  await page.el("discord-channel").dispatch("change");
  await page.settle();
  assert.equal(page.pure("selectedChannelId()"), other.id);
  // A channel the picker does not list, as a link can name, chooses none, as a <select> does.
  page.pure("(chooseChannel('9990000000000000009'), 0)");
  assert.deepStrictEqual([page.pure("selectedChannelId()"), page.el("discord-channel").value], ["", ""]);
});

test("`#624 voice-page-modularity`: page.pure reaches only what is still the legacy script's own, and the prelude's names only as its bindings", () => {
  const page = newPage();
  assert.equal(page.pure("selectedChannelId()"), "", "a binding of the legacy script's own was refused");
  assert.throws(() => page.pure("readPieces"), /reaches readPieces, which the page takes from web\/src/);
  assert.throws(() => page.pure('JSON.stringify(messageLinkRef("slack:C1:2"))'), /reaches messageLinkRef/);
  assert.equal(typeof page.preludeBinding("readPieces"), "function");
  assert.throws(() => page.preludeBinding("selectedChannelId"), /not a name the page takes from its modules/);
});

test("`#624 voice-page-modularity`: each column keeps its own records of its older pages, reply arrows, reads, entries, pins, scroll tools, filters, lists, rows, selection, boxes, Done marks, scroll places and gatherings, reached through the column worked on", () => {
  const page = newPage();
  const same = (expression) => page.pure(expression);
  assert.equal(same("olderPages() === columns.main.older"), true, "the left column's record was not the one worked on");
  assert.equal(same("inColumn(columns.side, () => olderPages()) === columns.side.older"), true,
    "the right column's record was not the one worked on inside it");
  assert.equal(same("columns.main.older !== columns.side.older"), true, "the columns shared one record");
  // Written in one column, read in it; the other's untouched.
  same("inColumn(columns.side, () => { olderPages().cursor = 'side-cursor'; olderPages().moreAbove = true; })");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.older.cursor, columns.side.older.moreAbove, olderPages().cursor])")),
    ["side-cursor", true, null]);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshOlderPages())")),
    { moreAbove: false, cursor: null, inFlight: false, step: null, deferred: false });
  // The same for its reply arrows.
  assert.equal(same("inColumn(columns.side, () => replyArrows()) === columns.side.arrows && replyArrows() === columns.main.arrows"), true);
  same("inColumn(columns.side, () => { replyArrows().held += 1; })");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.arrows.held, replyArrows().held])")), [1, 0]);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshReplyArrows())")),
    { back: null, landed: null, tap: null, settling: null, held: 0, owed: false, countsDrawn: false });
  // And its reads' state.
  assert.equal(same("inColumn(columns.side, () => loadState()) === columns.side.load && loadState() === columns.main.load"), true);
  same("inColumn(columns.side, () => { ++loadState().generation; loadState().freshness = 'slow'; })");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.load.generation, columns.side.load.freshness, loadState().freshness])")),
    [1, "slow", "fresh"]);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshLoadState())")),
    { freshness: "fresh", freshAt: 0, refreshing: false, lineTimer: null, generation: 0 });
  // And entering a view.
  assert.equal(same("inColumn(columns.side, () => entryState()) === columns.side.entry && entryState() === columns.main.entry"), true);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshEntryState())")),
    { threadOwed: null, threadAwaitsRead: false, readOwed: null, landing: null, backGesture: null });
  // And its Pinned filter.
  same("inColumn(columns.side, () => { pinsView().only = true; })");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.pins.only, pinsView().only])")), [true, false]);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshPinsView())")), { only: false, drawn: "" });
  // And its scroll tools' pass.
  same("inColumn(columns.side, () => { scrollTools().held += 1; })");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.tools.held, scrollTools().held])")), [1, 0]);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshScrollTools())")), { held: 0, owed: false });
  // And its filters' hold.
  assert.equal(same("inColumn(columns.side, () => filterState()) === columns.side.filter && filterState() === columns.main.filter"), true);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshFilterState())")), { hold: null });
  // And what its list has drawn.
  same("inColumn(columns.side, () => { listState().backlog = 3; })");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.list.backlog, listState().backlog])")), [3, 0]);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshListState())")),
    { rowsGathered: false, outgoingKey: "", newestId: null, runRows: 0, backlog: 0 });
  // And the rows its list was drawn from: a fresh pair of lists for each column.
  assert.equal(same("columns.side.rows.messages !== columns.main.rows.messages"), true, "the columns shared one list of rows");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshTimelineRows())")), { messages: [], threads: [] });
  // And what it shows: the right column starts on a thread, as its swapped fields did.
  assert.equal(same("inColumn(columns.side, () => selection()) === columns.side.selection && selection() === columns.main.selection"), true);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify(freshSelection())")), { view: "thread", threadId: null, thread: null, origin: "main" });
  // And its message box and what it shows as Done: a fresh empty set for each column.
  assert.equal(same("columns.side.done.archived !== columns.main.done.archived && doneState() === columns.main.done"), true);
  assert.equal(same("inColumn(columns.side, () => composerState()) === columns.side.composer"), true);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([freshComposerState(), freshDoneState().archived.size])")), [{ key: "" }, 0]);
  // And where its list stands: the channel list's entries of the shared lookups are the column's, the voice list's the page's.
  same("inColumn(columns.side, () => { jumpNewestWanted.discord = 3; inScrollback.discord = true; }); jumpNewestWanted.voice = 2");
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.place, jumpNewestWanted.discord, inScrollback.discord, jumpNewestWanted.voice, inColumn(columns.side, () => jumpNewestWanted.voice)])")),
    [{ jumpWanted: 3, inScrollback: true }, false, false, 2, 2]);
  // And what is gathered in its list; and nothing is swapped through the module any more.
  assert.equal(same("inColumn(columns.side, () => gatherState()) === columns.side.gather && gatherState() === columns.main.gather"), true);
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([freshGatherState(), typeof COLUMN_STATE])")), [{ gathered: null }, "undefined"]);
  // A column put aside has been worked on and left; the one taken up has not.
  assert.deepStrictEqual(JSON.parse(same("JSON.stringify([columns.side.left, inColumn(columns.side, () => [columns.side.left, columns.main.left])])")),
    [true, [false, true]]);
});

// --- the requests module, on its own ports (`#624 voice-page-modularity`) -----------------------

/**
 * web/src/core/api on ports and dependencies a test supplies: every request it sends, every timer
 * it sets, every cancellation it makes, and every call it makes back into the page, recorded.
 * `answers` is what the transport does with each request in turn: a `{status, text}` answer, or an
 * Error to fail to reach the server with.
 */
function apiOnPorts(answers = []) {
  const record = { sent: [], timers: [], cleared: [], cancellations: [], refused: 0, asked: [] };
  const ports = {
    http: {
      fetch: async (path, init) => {
        record.sent.push({ path, init });
        const answer = answers.shift() ?? { status: 200, text: "{}" };
        if (answer instanceof Error) throw answer;
        return { status: answer.status, ok: answer.status >= 200 && answer.status < 300, text: async () => answer.text };
      },
    },
    clock: {
      setTimeout: (run, ms) => { record.timers.push({ run, ms }); return record.timers.length; },
      clearTimeout: (handle) => { record.cleared.push(handle); },
    },
    cancellation: {
      create: () => {
        const cancellation = { signal: { aborted: false }, abort() { this.signal.aborted = true; } };
        record.cancellations.push(cancellation);
        return cancellation;
      },
    },
  };
  const server = SERVER_API.createApi(ports, {
    token: () => "tok",
    readRefused: () => { record.refused += 1; },
    asking: (type) => { record.asked.push({ type, sentBefore: record.sent.length }); },
    answerer: () => "Discord",
  });
  return { server, record };
}

test("`#624 voice-page-modularity`: the requests module sends through its transport port, with the token, and a body only when there is one", async () => {
  const { server, record } = apiOnPorts([{ status: 200, text: '{"a":1}' }, { status: 200, text: "" }]);
  assert.deepStrictEqual(await server.request("/api/v1/x"), { a: 1 });
  assert.equal(await server.request("/api/v1/y", { method: "POST", body: { b: 2 } }), null, "an empty answer is null");
  assert.deepStrictEqual(record.sent.map((s) => s.path), ["/api/v1/x", "/api/v1/y"]);
  assert.deepStrictEqual(record.sent[0].init, { method: "GET", headers: { Authorization: "Bearer tok" } },
    "a GET says it carries JSON, or carries a signal it was not given");
  assert.deepStrictEqual(record.sent[1].init,
    { method: "POST", headers: { Authorization: "Bearer tok", "Content-Type": "application/json" }, body: '{"b":2}' });
});

test("`#624 voice-page-modularity`: the requests module keeps the error taxonomy, and ends the device's copy only on a refused token or read", async () => {
  const refusal = (status) => ({ status, text: JSON.stringify({ error: "nope", detail: "Not yours." }) });
  for (const [status, method, refused] of [[401, "POST", 1], [403, "GET", 1], [403, "POST", 0], [500, "GET", 0]]) {
    const { server, record } = apiOnPorts([refusal(status)]);
    const error = await server.request("/api/v1/x", { method }).then(() => null, (e) => e);
    assert.ok(error, `HTTP ${status} to a ${method} succeeded`);
    assert.equal(error.message, `HTTP ${status} nope: Not yours.`);
    assert.deepStrictEqual([error.status, error.code, error.detail, error.refused],
      [status, "nope", "Not yours.", status === 401 || status === 403]);
    assert.deepStrictEqual(error.body, { error: "nope", detail: "Not yours." });
    assert.equal(record.refused, refused, `HTTP ${status} to a ${method}: the page's copy was dropped ${record.refused} times`);
  }
  // Not this server speaking: a proxy's page, a gateway's JSON, the network.
  const proxy = await apiOnPorts([{ status: 502, text: "<html>" }]).server.request("/x").catch((e) => e);
  assert.deepStrictEqual([proxy.message, proxy.gatewayStatus], ["vibe-talk returned non-JSON (HTTP 502)", 502]);
  const garbled = await apiOnPorts([{ status: 200, text: "<html>" }]).server.request("/x").catch((e) => e);
  assert.equal(garbled.gatewayStatus, undefined, "a garbled success was worded as a gateway's failure");
  const gateway = await apiOnPorts([{ status: 504, text: '{"message":"x"}' }]).server.request("/x").catch((e) => e);
  assert.deepStrictEqual([gateway.message, gateway.code], ["HTTP 504 error: (no detail)", "error"]);
  const unreachable = await apiOnPorts([new TypeError("Failed to fetch")]).server
    .request("/x", { method: "PUT" }).catch((e) => e);
  assert.equal(unreachable.network, true);
  assert.match(unreachable.message, /^Could not reach vibe-talk while saving your change\./);
  assert.equal(unreachable.cause.message, "Failed to fetch");
});

test("`#624 voice-page-modularity`: a decoded request is told to the page before it is sent, and a malformed answer names the type", async () => {
  const { server, record } = apiOnPorts([{ status: 200, text: '{"nope":true}' }]);
  const error = await server.decoded("ApiErrorBody", "/x").catch((e) => e);
  assert.deepStrictEqual(record.asked, [{ type: "ApiErrorBody", sentBefore: 0 }]);
  assert.equal(record.sent.length, 1, "the decoded request was told to the page but never sent");
  assert.match(error.message, /^vibe-talk returned a malformed answer — .*ApiErrorBody/);
  assert.ok(error.cause, "the contract's own sentence was dropped");
});

test("`#624 voice-page-modularity`: a request given up on by `within` times out on the clock port, aborted unless the caller keeps it", async () => {
  // Aborted: the one cancellation made, its signal handed to the request, and aborted at the limit.
  const { server, record } = apiOnPorts();
  let signal = null;
  const waited = server.within(20_000, (s) => { signal = s; return new Promise(() => {}); }).catch((e) => e);
  assert.equal(record.timers.length, 1);
  assert.equal(record.timers[0].ms, 20_000);
  assert.equal(signal, record.cancellations[0].signal, "the request was not handed the cancellation's signal");
  record.timers[0].run();
  const error = await waited;
  assert.deepStrictEqual([error.message, error.timedOut], ["Discord did not answer within 20 seconds.", true]);
  assert.equal(signal.aborted, true, "the request given up on was not aborted");
  assert.deepStrictEqual(record.cleared, [1], "the timer was not cleared");
  // Kept: no cancellation at all, and the request still on the wire handed back.
  const kept = apiOnPorts();
  const still = new Promise(() => {});
  let handed = null;
  const keptWait = kept.server.within(1_500, (s) => { assert.equal(s, undefined); return still; }, (answer) => { handed = answer; })
    .catch((e) => e);
  kept.record.timers[0].run();
  assert.match((await keptWait).message, /within 2 seconds/);
  assert.equal(handed, still, "the request still on the wire was not handed to the caller");
  assert.equal(kept.record.cancellations.length, 0, "a request the caller keeps was given a cancellation");
  // Answered in time: the answer, and the timer cleared.
  const quick = apiOnPorts();
  assert.equal(await quick.server.within(5_000, async () => "ok"), "ok");
  assert.deepStrictEqual(quick.record.cleared, [1]);
});
