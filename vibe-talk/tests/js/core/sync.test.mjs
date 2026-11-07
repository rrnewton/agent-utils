// Tests for web/src/core/sync/ on its own: the module built as the page build builds it and held
// directly, with no page around it (`#624 voice-page-modularity`). The page's behaviour over the
// same module is tested in ../voice_page.test.mjs.

import { test } from "node:test";
import assert from "node:assert/strict";

import { moduleForTests } from "../../../web/build.mjs";

/** How the page keeps its channels current: reads, streams and their order (web/src/core/sync). */
const SYNC = await moduleForTests("src/core/sync/index.ts");

test("`#624 voice-page-modularity`: a Server-Sent Events stream read into frames, across chunk boundaries, from the module alone", async () => {
  const { parseEventFrame, readEventStream } = SYNC;
  assert.deepStrictEqual(parseEventFrame("id: 7\nevent: message_create\ndata: {\"a\":\ndata: 1}\r"),
    { id: "7", event: "message_create", data: '{"a":\n1}' });
  assert.deepStrictEqual(parseEventFrame(": keep-alive"), { id: null, event: "message", data: "" }, "a comment was a frame with data");
  assert.deepStrictEqual(parseEventFrame("data"), { id: null, event: "message", data: "" });
  const reading = (chunks, { stopAfter = Infinity } = {}) => {
    let read = 0;
    let cancelled = 0;
    const reader = {
      read: async () => (read < chunks.length ? { done: false, value: chunks[read++] } : { done: true }),
      cancel: () => { cancelled += 1; return Promise.reject(new Error("refused")); },
    };
    const frames = [];
    const decoder = { decode: (bytes, options) => { assert.deepStrictEqual(options, { stream: true }); return bytes; } };
    const done = readEventStream(reader, () => read <= stopAfter, (frame) => {
      frames.push(frame);
      return frame.event === "reset" ? "reset" : undefined;
    }, decoder);
    return done.then((how) => ({ how, frames, cancelled: () => cancelled }));
  };
  // A frame split across chunks waits for its blank line; a partial last frame is never handed on.
  const split = await reading(["data: one\n", "\nid: 2\nda", "ta: two\n\n: ping\n\ndata: partial"]);
  assert.deepStrictEqual(split.frames.map((frame) => [frame.id, frame.data]), [[null, "one"], ["2", "two"], [null, ""]]);
  assert.equal(split.how, "ended");
  assert.equal((await reading(["event: reset\n\n", "data: after\n\n"])).how, "reset", "a reset frame did not end the read as a reset");
  // Stopped: the reader is cancelled, its refusal swallowed, and no later frame is handed on.
  const stopped = await reading(["data: a\n\n", "data: b\n\n"], { stopAfter: 1 });
  assert.deepStrictEqual(stopped.frames.map((frame) => frame.data), ["a"]);
  assert.deepStrictEqual([stopped.how, stopped.cancelled()], ["ended", 1]);
});

test("`#624 voice-page-modularity`: a read asked for while another is out joins the one queued, and streams' generations, from the module alone", () => {
  const { joinQueuedReads, createStreamGeneration } = SYNC;
  assert.deepStrictEqual(joinQueuedReads({ reason: "reset" }, { reason: "live", keepPosition: true }),
    { reason: ["reset", "live"], keepPosition: true }, "a reason was dropped, or the newest options lost");
  assert.deepStrictEqual(joinQueuedReads({ reason: ["poll", "live"] }, { reason: "live" }), { reason: ["poll", "live"] });
  assert.deepStrictEqual(joinQueuedReads({ reason: "poll" }, { reason: "poll" }), { reason: "poll" }, "one reason was not left as one");
  assert.deepStrictEqual(joinQueuedReads({ reason: "view", quiet: true, readAs: "old" }, { reason: "poll", readAs: "new" }),
    { reason: ["view", "poll"], readAs: "new", quiet: true }, "quiet was not kept from the read already queued");
  assert.deepStrictEqual(joinQueuedReads(undefined, undefined), { reason: undefined });
  const generation = createStreamGeneration();
  const started = generation.current();
  generation.bump();
  assert.deepStrictEqual([started, generation.current()], [0, 1]);
});

test("`#624 voice-page-modularity`: the live events counted per channel, from the module alone", () => {
  const { createLiveEvents } = SYNC;
  const live = createLiveEvents();
  assert.deepStrictEqual([live.seen("c"), live.news("c"), live.removals("c")], [0, 0, 0]);
  live.noteEvent("c");
  live.noteEvent("c");
  live.noteNews("c");
  live.noteRemoval("d");
  assert.deepStrictEqual([live.seen("c"), live.news("c"), live.removals("c"), live.removals("d"), live.seen("d")], [2, 1, 0, 1, 0],
    "one channel's or one kind's events were counted as another's");
});

test("`#624 voice-page-modularity`: views held unpolled after a read given up on, from the module alone", () => {
  const { createReadHolds, SERVER_READ_BOUND_MS } = SYNC;
  let now = 1000;
  const holds = createReadHolds({ now: () => now });
  const hold = holds.hold("c main", "c", 1000);
  holds.hold("c threads", "c", 1000);
  holds.hold("d main", "d", 1000);
  // Only a read whose every reason is the poll waits.
  assert.deepStrictEqual([holds.held("c main", "poll"), holds.held("c main", ["poll", "poll"]), holds.held("c main", ["poll", "reset"]),
    holds.held("c main", undefined), holds.held("e main", "poll")], [true, true, false, false, false]);
  assert.equal(holds.channelWaits("c"), true);
  // An answer takes away its own hold, not one set again since.
  holds.release("c main", { channel: "c", until: 0 });
  assert.equal(holds.held("c main", "poll"), true, "an answer took away a hold that was not its own");
  holds.release("c main", hold);
  assert.equal(holds.held("c main", "poll"), false);
  // Live news ends a channel's holds; the server's bound ends the rest.
  holds.liftChannel("c");
  assert.deepStrictEqual([holds.channelWaits("c"), holds.channelWaits("d")], [false, true]);
  now = 1000 + SERVER_READ_BOUND_MS;
  assert.equal(holds.held("d main", "poll"), false, "a hold past the server's bound still held the poll");
  holds.hold("d main", "d", 1000);
  assert.equal(holds.channelWaits("d"), false, "a hold past its bound kept the placing read waiting");
});

test("`#624 voice-page-modularity`: which replayed events still wait for a read, from the module alone", () => {
  const { createReplayLedger } = SYNC;
  const ledger = createReplayLedger();
  assert.deepStrictEqual([ledger.started(), ledger.start(), ledger.start(), ledger.started()], [0, 1, 2, 2]);
  ledger.noteAttached();
  assert.equal(ledger.attachedAfter(), 2);
  // Nothing landed yet: both wait.
  assert.equal(ledger.awaitRead("m1", "c", 2), false);
  assert.equal(ledger.awaitRead("message_update:m2", "c", 2), false);
  assert.deepStrictEqual([ledger.waiting(), ledger.mutationAwaits()], [2, true]);
  // A delta of additions begun after them answers the new message, not the edit.
  const read = ledger.start();
  ledger.settle(read, { messages: [], threads: [], delta: { complete: false } }, "c");
  assert.deepStrictEqual([ledger.waiting(), ledger.mutationAwaits()], [1, true], "an incomplete delta answered an edit");
  // A failed read answers for nothing; a complete one answers the edit.
  ledger.settle(ledger.start(), null, "c");
  assert.equal(ledger.waiting(), 1);
  ledger.settle(ledger.start(), { messages: [], delta: null }, "c");
  assert.deepStrictEqual([ledger.waiting(), ledger.mutationAwaits()], [0, false]);
  // Once a read past an arrival has landed, a later replay of it needs no read; another channel's is forgotten.
  assert.equal(ledger.awaitRead("m3", "c", 3), true, "a landed read did not answer an event it began after");
  assert.equal(ledger.awaitRead("m4", "other", 99), false);
  ledger.settle(ledger.start(), null, "c");
  assert.equal(ledger.waiting(), 0, "an event of a channel no longer on screen still waited");
  // A page that holds it answers it, though the read began before it arrived.
  assert.equal(ledger.awaitRead("m5", "c", 99), false);
  ledger.settle(4, { messages: [{ id: "m5" }] }, "c");
  assert.equal(ledger.waiting(), 0);
});

test("`#624 voice-page-modularity`: the order timeline reads were sent in, from the module alone", () => {
  const { createReadOrder } = SYNC;
  const order = createReadOrder();
  const first = {};
  const second = {};
  const unasked = {};
  assert.deepStrictEqual([order.asked(), order.now()], [0, 0.5]);
  const firstNumber = order.asked();
  order.noteAsked();
  const secondNumber = order.asked();
  order.noteAsked();
  // The second read's answer lands first: each still stands where its read was sent.
  order.noteSent(second, secondNumber);
  order.noteSent(first, firstNumber);
  assert.deepStrictEqual([order.sentNumberOf(first), order.sentNumberOf(second)], [0, 1]);
  assert.equal(order.sentNumberOf(unasked), 2, "a payload no read sent did not stand after every read so far");
  assert.equal(order.now(), 2.5, "something put in now did not stand between reads");
});

test("`#624 voice-page-modularity`: the quick retries after a stand-in read, from the module alone", () => {
  const { createStandInRetries, STAND_IN_RETRY_MS, DISCORD_POLL_MS } = SYNC;
  const retries = createStandInRetries();
  assert.equal(retries.take(DISCORD_POLL_MS), DISCORD_POLL_MS, "a poll came early with no stand-in read before it");
  // Each stand-in read wants the next quick retry, until they are spent.
  const waits = [];
  for (let read = 0; read <= STAND_IN_RETRY_MS.length; read += 1) {
    const wanted = retries.noteRead(true);
    waits.push([wanted, retries.take(DISCORD_POLL_MS)]);
  }
  assert.deepStrictEqual(waits, [...STAND_IN_RETRY_MS.map((ms) => [true, ms]), [false, DISCORD_POLL_MS]]);
  // A shorter wait already armed is kept; a current read starts the retries again.
  retries.noteRead(false);
  assert.equal(retries.noteRead(true), true, "a current read did not start the quick retries again");
  assert.equal(retries.take(1000), 1000);
  assert.equal(retries.noteRead(false), false);
  assert.equal(retries.take(DISCORD_POLL_MS), DISCORD_POLL_MS, "a retry wanted before a current read was still taken");
});

test("`#624 voice-page-modularity`: which read a refresh makes, and how fresh it is, from the module alone", () => {
  const { deltaCursor, isCursorRefusal, isMutationKey, staleAsOf, STALE_AS_OF_MS, TIMELINE_RECONCILE_MS } = SYNC;
  let now = 10 * TIMELINE_RECONCILE_MS;
  const facts = (over = {}) => ({
    storeChannel: () => "c", selectedChannel: () => "c", complete: () => true, mutationAwaits: () => false, now: () => now,
    cover: () => ({ live: true, newest: "cur", fullAt: now - 1000 }), ...over,
  });
  assert.equal(deltaCursor("poll", facts()), "cur");
  assert.equal(deltaCursor("enter", facts()), null, "a reason no delta serves read a delta");
  assert.equal(deltaCursor(undefined, facts()), null);
  assert.equal(deltaCursor("poll", facts({ storeChannel: () => "other" })), null, "another channel's cursor was used");
  assert.equal(deltaCursor("poll", facts({ cover: () => ({ live: false, newest: "cur", fullAt: now }) })), null);
  assert.equal(deltaCursor("poll", facts({ cover: () => ({ live: true, newest: "cur", fullAt: now - TIMELINE_RECONCILE_MS }) })), null,
    "a view past its reconcile read a delta");
  // Edits and removals need complete deltas; a replay only while one is waiting for a read.
  assert.deepStrictEqual(["mutation", "replay", "poll", "live"].map((reason) => deltaCursor(reason, facts({ complete: () => false }))),
    [null, "cur", null, "cur"]);
  assert.equal(deltaCursor("replay", facts({ complete: () => false, mutationAwaits: () => true })), null);
  // A queued read standing for several reasons is a delta only if every one could be.
  assert.equal(deltaCursor(["poll", "live"], facts()), "cur");
  assert.equal(deltaCursor(["poll", "enter"], facts()), null);
  assert.equal(deltaCursor([], facts()), null);
  assert.deepStrictEqual([isCursorRefusal({ status: 410, code: "cursor_expired" }), isCursorRefusal({ status: 400, code: "cursor_mismatch" }),
    isCursorRefusal({ status: 410, code: "other" }), isCursorRefusal(null)], [true, true, false, false]);
  assert.deepStrictEqual([isMutationKey("message_update:1"), isMutationKey("message_delete:1"), isMutationKey("message:1")], [true, true, false]);
  // Freshness: now, unless the server says its copy is old enough to say so, or it answered from its own copy.
  const at = (ms) => new Date(ms).toISOString();
  now = Date.parse("2026-10-29T12:00:00Z");
  const clock = () => now;
  assert.equal(staleAsOf({ as_of: at(now - STALE_AS_OF_MS) }, false, clock), null);
  assert.equal(staleAsOf({ as_of: at(now - STALE_AS_OF_MS - 1000) }, false, clock), now - STALE_AS_OF_MS - 1000);
  assert.equal(staleAsOf({}, false, clock), null);
  assert.equal(staleAsOf({ as_of: at(now - 5000) }, true, clock), now - 5000, "a stand-in copy was taken for a current one");
  assert.equal(staleAsOf({ as_of: at(now + 5000) }, true, clock), now, "a stand-in copy dated ahead claimed to be newer than now");
  assert.equal(staleAsOf(null, true, clock), now);
});

test("`#624 voice-page-modularity`: following a channel's stream attaches, keeps its cursor, retries, and stops", async () => {
  const { createChannelFollow, LIVE_ATTACH_WAIT_MS, LIVE_RETRY_MS } = SYNC;
  assert.equal(LIVE_ATTACH_WAIT_MS, 3000);
  const timers = new Map();
  let next = 0;
  const clock = {
    setTimeout: (run, ms) => { next += 1; timers.set(next, { run, ms }); return next; },
    clearTimeout: (handle) => timers.delete(handle),
  };
  const fire = (ms) => {
    for (const [handle, timer] of [...timers]) if (timer.ms === ms) { timers.delete(handle); timer.run(); }
  };
  const opened = [];
  const said = [];
  const frames = [];
  let token = "t0k";
  let delivery = "push";
  let chunks = [];
  let respond = null;
  const encode = (text) => text;
  const follow = createChannelFollow({
    clock,
    token: () => token,
    delivery: () => delivery,
    open: (path, headers) => {
      opened.push([path, { ...headers }]);
      return new Promise((resolve) => { respond = resolve; });
    },
    decoder: () => ({ decode: (bytes) => (bytes === undefined ? "" : String(bytes)) }),
    frame: (frame) => { frames.push(frame); },
    attached: () => said.push("attached"),
    changed: () => said.push("changed"),
    dropped: (why) => said.push(`dropped: ${why}`),
    guard: (run) => { run().catch((error) => said.push(`threw: ${error.message}`)); },
  });
  const settle = () => new Promise((resolve) => setImmediate(resolve));
  const body = () => {
    const queue = chunks;
    let release;
    const ended = new Promise((resolve) => { release = resolve; });
    return {
      release,
      response: { ok: true, status: 200, body: { getReader: () => ({
        read: async () => (queue.length ? { done: false, value: encode(queue.shift()) } : (await ended, { done: true })),
      }) } },
    };
  };
  // Nothing followed: a wait runs at once.
  let ran = 0;
  follow.whenAttached(() => { ran += 1; });
  assert.equal(ran, 1);
  // Off on the server: nothing attaches, and Settings is told.
  delivery = "off";
  follow.start("42");
  assert.deepStrictEqual([follow.channel(), opened.length, said.slice(-1)[0]], [null, 0, "changed"]);
  delivery = "push";
  // Attaching: a wait holds until it does.
  follow.start("4 2");
  assert.equal(follow.channel(), "4 2");
  follow.whenAttached(() => { ran += 1; });
  assert.equal(ran, 1, "a wait ran before the stream attached");
  assert.deepStrictEqual(opened[0], ["/api/v1/channels/4%202/stream", { Authorization: "Bearer t0k" }]);
  chunks = ["id: 7\nevent: message\ndata: {}\n\n"];
  const first = body();
  respond(first.response);
  await settle();
  assert.equal(follow.attached(), true);
  assert.equal(follow.resumed(), false);
  assert.equal(ran, 2, "the wait did not run on the attach");
  assert.equal(follow.lastEventId(), "7");
  assert.deepStrictEqual(frames.map((frame) => frame.event), ["message"]);
  // The response ends: not attached, and a retry after LIVE_RETRY_MS resumes from the cursor.
  first.release();
  await settle();
  assert.equal(follow.attached(), false);
  fire(LIVE_RETRY_MS);
  await settle();
  assert.deepStrictEqual(opened[1][1], { Authorization: "Bearer t0k", "Last-Event-ID": "7" });
  chunks = ["event: reset\ndata:\n\n"];
  const second = body();
  respond(second.response);
  await settle();
  assert.equal(follow.resumed(), true);
  assert.equal(follow.lastEventId(), null, "a reset kept the cursor");
  // A wait that is not answered in time runs anyway.
  const generation = follow.generation();
  follow.stop();
  assert.ok(follow.generation() > generation);
  assert.deepStrictEqual([follow.channel(), follow.attached(), follow.lastEventId()], [null, false, null]);
  second.release();
  await settle();
  follow.start("9", "41");
  follow.whenAttached(() => { ran += 1; });
  assert.deepStrictEqual(opened[2][1], { Authorization: "Bearer t0k", "Last-Event-ID": "41" });
  fire(LIVE_ATTACH_WAIT_MS);
  assert.equal(ran, 3, "a wait outlived LIVE_ATTACH_WAIT_MS");
  // A refusal is recorded and retried, and releases nothing twice.
  respond({ ok: false, status: 503, body: null });
  await settle();
  assert.ok(said.includes("dropped: the live stream was refused (HTTP 503)"));
  // Stopped while waiting to retry: the retry does not reopen.
  follow.stop();
  fire(LIVE_RETRY_MS);
  await settle();
  assert.equal(opened.length, 3, "a stopped stream retried");
  // No credential: nothing is followed.
  token = "";
  follow.start("9");
  assert.equal(follow.channel(), null);
});

test("`#624 voice-page-modularity`: the channel's poll is armed, held while the page is hidden, and taken up again", () => {
  const { createChannelPoll, DISCORD_POLL_MS } = SYNC;
  const timers = new Map();
  let next = 0;
  let now = 1000;
  let hidden = false;
  let onChannel = true;
  let polls = 0;
  const poll = createChannelPoll({
    clock: { setTimeout: (run, ms) => { next += 1; timers.set(next, { run, ms }); return next; }, clearTimeout: (h) => timers.delete(h) },
    now: () => now,
    hidden: () => hidden,
    onChannel: () => onChannel,
    take: (delay) => delay,
    poll: () => { polls += 1; },
  });
  const fire = () => { for (const [h, t] of [...timers]) { timers.delete(h); t.run(); } };
  poll.schedule();
  assert.deepStrictEqual([poll.armed(), poll.dueAt(), [...timers.values()][0].ms], [true, 1000 + DISCORD_POLL_MS, DISCORD_POLL_MS]);
  fire();
  assert.equal(polls, 1);
  // Hidden when armed: held, nothing on the clock.
  hidden = true;
  poll.schedule(5000);
  assert.deepStrictEqual([poll.armed(), poll.paused(), timers.size], [false, true, 0]);
  // Shown again: the rest of the wait.
  hidden = false;
  now = 3000;
  poll.resume("rest");
  assert.deepStrictEqual([poll.paused(), [...timers.values()][0].ms], [false, 3000]);
  // Hidden while armed: the poll stops and waits.
  poll.pause();
  assert.deepStrictEqual([poll.armed(), poll.paused()], [false, true]);
  poll.resume("now");
  assert.equal(polls, 2);
  // Come due while hidden: held when it fires.
  poll.schedule(10);
  hidden = true;
  fire();
  assert.deepStrictEqual([polls, poll.paused()], [2, true]);
  hidden = false;
  poll.resume("fresh");
  assert.equal([...timers.values()][0].ms, DISCORD_POLL_MS);
  // Another view: the poll that fires does nothing, and resuming arms nothing.
  onChannel = false;
  fire();
  assert.equal(polls, 2);
  poll.pause();
  poll.stop();
  assert.equal(poll.paused(), false, "stopping kept a held wait");
  poll.resume("now");
  assert.equal(polls, 2);
});

test("`#624 voice-page-modularity`: a hidden page lets its stream go after LIVE_HIDDEN_CLOSE_MS with no call, keeps where it was, and coming back takes what arrived", () => {
  const { createHiddenStream, LIVE_HIDDEN_CLOSE_MS } = SYNC;
  const timers = new Map();
  let next = 0;
  let channel = "C1";
  let generation = 4;
  let hidden = true;
  let call = false;
  let stops = 0;
  const stream = createHiddenStream({
    clock: { setTimeout: (run, ms) => { next += 1; timers.set(next, { run, ms }); return next; }, clearTimeout: (h) => timers.delete(h) },
    follow: { channel: () => channel, lastEventId: () => "e9", generation: () => generation },
    hidden: () => hidden,
    callListening: () => call,
    stop: () => { stops += 1; channel = null; generation += 1; },
  });
  const fire = () => { for (const [h, t] of [...timers]) { timers.delete(h); t.run(); } };
  stream.hide();
  stream.hide();
  assert.deepStrictEqual([...timers.values()].map((t) => t.ms), [LIVE_HIDDEN_CLOSE_MS], "hidden twice arms one timer");
  call = true;
  fire();
  assert.equal(stops, 0, "a call listening keeps the stream");
  assert.equal(stream.takeResume(), null);
  call = false;
  stream.hide();
  fire();
  assert.equal(stops, 1);
  assert.deepStrictEqual(stream.takeResume(), { channel: "C1", from: "e9", generation: 5 });
  assert.equal(stream.takeResume(), null, "a resume is taken once");
  stream.noteMessage();
  stream.noteMutation();
  assert.deepStrictEqual(stream.takeArrivals(() => false), { mutation: true, arrived: true });
  assert.deepStrictEqual(stream.takeArrivals(() => false), { mutation: false, arrived: false }, "what arrived is taken once");
  assert.deepStrictEqual(stream.takeArrivals(() => true), { mutation: false, arrived: true }, "replays waiting are an arrival");
  channel = "C1";
  stream.hide();
  stream.takeArrivals(() => false);
  assert.equal(timers.size, 0, "coming back stops the timer");
  hidden = false;
  stream.hide();
  fire();
  assert.equal(stops, 1, "a page shown again when the timer runs keeps its stream");
});

test("`#624 voice-page-modularity`: a burst of live edits is one re-read, one arriving during it is one more, and a read in flight is waited out", async () => {
  const { createMutationRefresh } = SYNC;
  const timers = [];
  let busy = true;
  let reads = 0;
  let release = null;
  let launched = null;
  const refresh = createMutationRefresh({
    clock: { setTimeout: (run, ms) => { timers.push({ run, ms }); return timers.length; }, clearTimeout: () => {} },
    busy: () => busy,
    read: () => { reads += 1; return new Promise((resolve) => { release = resolve; }); },
    launch: (run) => { launched = run(); },
  });
  const settle = () => new Promise((resolve) => setImmediate(resolve));
  refresh.request();
  refresh.request();
  await settle();
  assert.equal(reads, 0, "an ordinary read in flight is waited for");
  assert.deepStrictEqual(timers.map((t) => t.ms), [50]);
  busy = false;
  timers.shift().run();
  await settle();
  assert.equal(reads, 1, "two edits before the read began are one read");
  refresh.request();
  refresh.request();
  release();
  await settle();
  assert.equal(reads, 2, "edits during the read are one read more");
  release();
  await launched;
  assert.equal(reads, 2);
  refresh.request();
  await settle();
  assert.equal(reads, 3, "the loop ended, a new edit starts it again");
  release();
  await launched;
});

test("`#624 voice-page-modularity`: one channel read on the wire, the view it is for, and one joined read queued per column", () => {
  const { createChannelReads } = SYNC;
  const reads = createChannelReads();
  const [left, right] = [{ name: "main" }, { name: "side" }];
  assert.deepStrictEqual([reads.busy(), reads.readContext()], [false, null]);
  assert.equal(reads.claim("ctx-main"), true);
  assert.deepStrictEqual([reads.busy(), reads.readContext()], [true, "ctx-main"]);
  assert.equal(reads.claim("ctx-all"), false, "one read at a time");
  assert.equal(reads.readContext(), "ctx-main", "and the refused one changes nothing");
  reads.queue(right, { reason: "poll" });
  reads.queue(left, { reason: "live", quiet: true });
  reads.queue(left, { reason: "reset", keepPosition: true });
  assert.deepStrictEqual(reads.pending(left), { reason: ["live", "reset"], keepPosition: true, quiet: true }, "joined");
  assert.deepStrictEqual(reads.release([left, right]), { column: left, options: { reason: ["live", "reset"], keepPosition: true, quiet: true } });
  assert.deepStrictEqual([reads.busy(), reads.readContext(), reads.queued(left), reads.queued(right)], [false, null, false, true]);
  assert.equal(reads.claim(null), true);
  reads.drop(right);
  assert.equal(reads.release([left, right]), null, "nothing queued");
  assert.equal(reads.busy(), false);
});

test("`#624 voice-page-modularity`: a live page with more above and no cursor owes the walk one, until it is paid", () => {
  const { createChannelReads } = SYNC;
  const reads = createChannelReads();
  assert.equal(reads.cursorOwed(), false);
  reads.oweCursor(true);
  assert.equal(reads.cursorOwed(), true);
  reads.payCursor();
  assert.equal(reads.cursorOwed(), false);
  reads.oweCursor(true);
  reads.oweCursor(false);
  assert.equal(reads.cursorOwed(), false, "a later page with a cursor owes none");
});

test("`#624 voice-page-modularity`: one read of a kind on the wire per channel, and the count of arrivals left unplaced", () => {
  const { createChannelInFlight, createArrivalCount } = SYNC;
  const flight = createChannelInFlight();
  assert.deepStrictEqual([flight.busy(), flight.channel(), flight.isFor("c1")], [false, null, false]);
  flight.begin("c1");
  assert.deepStrictEqual([flight.busy(), flight.channel(), flight.isFor("c1"), flight.isFor("c2")], [true, "c1", true, false]);
  flight.end("c2");
  assert.equal(flight.channel(), "c1", "another channel's end leaves this one's read on the wire");
  flight.end("c1");
  assert.equal(flight.busy(), false);
  flight.begin("c2");
  flight.end();
  assert.equal(flight.busy(), false, "an end naming no channel ends whatever is on the wire");
  const arrivals = createArrivalCount();
  assert.deepStrictEqual([arrivals.count(), arrivals.next(), arrivals.next(), arrivals.count()], [0, 1, 2, 2]);
});

test("`#624 voice-page-modularity`: a reopen owes one read, taken once, and kept for later where nothing newer is owed", () => {
  const { createReopenRead } = SYNC;
  const reopen = createReopenRead();
  assert.equal(reopen.take(), null);
  reopen.owe(3);
  const owed = reopen.take();
  assert.deepStrictEqual([owed, reopen.take()], [{ after: 3 }, null]);
  reopen.keep(owed);
  assert.deepStrictEqual(reopen.take(), { after: 3 }, "kept for the next sign-in");
  reopen.owe(5);
  reopen.keep(owed);
  assert.deepStrictEqual(reopen.take(), { after: 5 }, "a newer debt is not replaced by the old one");
});

test("`#624 voice-page-modularity`: the rows on screen belong to the credential they were last claimed for", () => {
  const { createScreenIdentity } = SYNC;
  const identity = createScreenIdentity("aaaa");
  assert.equal(identity.belongsTo("aaaa"), true);
  assert.equal(identity.belongsTo("bbbb"), false, "rows read for one token belong to another");
  identity.claim("bbbb");
  assert.deepStrictEqual([identity.belongsTo("aaaa"), identity.belongsTo("bbbb")], [false, true]);
  // No token at all is a credential of its own: rows drawn signed out are not a token's.
  assert.equal(createScreenIdentity("").belongsTo("aaaa"), false);
});

test("away clock: the first hide counts, coming back says how long and starts over", () => {
  const clock = SYNC.createAwayClock();
  assert.equal(clock.show(500), 0, "a page that was never hidden was not away");
  clock.hide(1000);
  clock.hide(4000);
  assert.equal(clock.show(6000), 5000, "a second hide keeps the first time");
  assert.equal(clock.show(9000), 0, "the clock starts over once visible");
});

test("`#624 voice-page-modularity`: the stream of every channel attaches, resumes from its cursor, starts again at once after a reset, lets its attach go, and asks a server without it once", async () => {
  const { createWatchAll, LIVE_RETRY_MS } = SYNC;
  const timers = new Map();
  let next = 0;
  const clock = {
    setTimeout: (run, ms) => { next += 1; timers.set(next, { run, ms }); return next; },
    clearTimeout: (handle) => timers.delete(handle),
  };
  const fire = (ms) => {
    for (const [handle, timer] of [...timers]) if (timer.ms === ms) { timers.delete(handle); timer.run(); }
  };
  const opened = [];
  const said = [];
  const frames = [];
  const aborted = [];
  let handles = 0;
  let token = "t0k";
  let delivery = "push";
  let chunks = [];
  let respond = null;
  const watch = createWatchAll({
    clock,
    token: () => token,
    delivery: () => delivery,
    abortable: () => {
      handles += 1;
      const id = handles;
      return { id, abort: () => aborted.push(id) };
    },
    open: (path, headers, abort) => {
      opened.push([path, { ...headers }, abort ? abort.id : null]);
      return new Promise((resolve) => { respond = resolve; });
    },
    decoder: () => ({ decode: (bytes) => (bytes === undefined ? "" : String(bytes)) }),
    frame: (frame) => {
      frames.push(frame.event);
      return frame.event === "reset" ? "reset" : undefined;
    },
    dropped: (why) => said.push(`dropped: ${why}`),
    unsupported: () => said.push("unsupported"),
    guard: (run) => { run().catch((error) => said.push(`threw: ${error.message}`)); },
  });
  const settle = () => new Promise((resolve) => setImmediate(resolve));
  const body = () => {
    const queue = chunks;
    let release;
    const ended = new Promise((resolve) => { release = resolve; });
    return {
      release,
      response: { ok: true, status: 200, body: { getReader: () => ({
        read: async () => (queue.length ? { done: false, value: queue.shift() } : (await ended, { done: true })),
      }) } },
    };
  };
  // Off on the server, or no credential: nothing is asked for.
  delivery = "off";
  watch.start();
  token = "";
  delivery = "push";
  watch.start();
  assert.equal(opened.length, 0, "the stream of every channel was asked for with nothing to deliver or no credential");
  token = "t0k";
  // A call's first attach resumes from nothing, and is not attached until the body opens.
  watch.start();
  assert.deepStrictEqual(opened[0], ["/api/v1/live/stream", { Authorization: "Bearer t0k" }, 1]);
  assert.equal(watch.attached(), false);
  chunks = ["id: w.7\nevent: message\ndata: {}\n\n"];
  const first = body();
  respond(first.response);
  await settle();
  assert.deepStrictEqual([watch.attached(), watch.resumed(), frames], [true, false, ["message"]]);
  // It ends: not attached, and the retry after LIVE_RETRY_MS resumes from the cursor.
  first.release();
  await settle();
  assert.equal(watch.attached(), false);
  assert.equal(opened.length, 1, "a dropped stream was asked for again before the retry delay");
  fire(LIVE_RETRY_MS);
  await settle();
  assert.deepStrictEqual(opened[1][1], { Authorization: "Bearer t0k", "Last-Event-ID": "w.7" });
  chunks = ["event: reset\ndata:\n\n"];
  const second = body();
  respond(second.response);
  await settle();
  assert.equal(watch.resumed(), true, "an attach from a cursor was not said to have resumed");
  // A reset: the stream starts again at once, from now, not over the gap.
  second.release();
  await settle();
  assert.equal(opened.length, 3, "a reset waited out the retry delay");
  assert.deepStrictEqual(opened[2][1], { Authorization: "Bearer t0k" }, "a reset kept the cursor");
  // The call ends: the attach now open is let go, and what it answers later reaches nothing.
  watch.stop();
  assert.deepStrictEqual(aborted, [3]);
  assert.equal(watch.attached(), false);
  respond({ ok: false, status: 503, body: null });
  await settle();
  fire(LIVE_RETRY_MS);
  await settle();
  assert.equal(opened.length, 3, "a stopped stream was asked for again");
  assert.deepStrictEqual(said, [], "a stopped stream's answer was reported");
  // A refusal is reported and retried; a new call starts from no cursor.
  watch.start();
  assert.deepStrictEqual(opened[3][1], { Authorization: "Bearer t0k" }, "a new call resumed the last call's stream");
  respond({ ok: false, status: 503, body: null });
  await settle();
  assert.deepStrictEqual(said, ["dropped: it was refused (HTTP 503)"]);
  fire(LIVE_RETRY_MS);
  await settle();
  assert.equal(opened.length, 5);
  // A server without it answers 404 once, and is not asked again, by a retry or by the next call.
  respond({ ok: false, status: 404, body: null });
  await settle();
  assert.deepStrictEqual(said.slice(1), ["unsupported"]);
  fire(LIVE_RETRY_MS);
  watch.start();
  await settle();
  assert.equal(opened.length, 5, "a server that said 404 was asked again");
  assert.deepStrictEqual(aborted, [3, 5], "a start did not let the last attach go first");
  // Where the browser has no way to let an attach go, it is asked for without one, and stopping is quiet.
  const plain = [];
  const bare = createWatchAll({
    clock,
    token: () => "t0k",
    delivery: () => "push",
    abortable: () => null,
    open: (path, headers, abort) => {
      plain.push(abort);
      return new Promise(() => {});
    },
    decoder: () => ({ decode: () => "" }),
    frame: () => undefined,
    dropped: () => {},
    unsupported: () => {},
    guard: (run) => { run().catch(() => {}); },
  });
  bare.start();
  bare.stop();
  assert.deepStrictEqual(plain, [null]);
});

test("`#624 voice-page-modularity`: a timeline read is counted as it is sent and joins its deltas oldest-first, from the module alone", async () => {
  const { createTimelineReader } = SYNC;
  const noted = [];
  let epoch = 5;
  const ledger = { epoch: () => epoch, noteSent: (p, s) => noted.push([p, s]), sentEpochOf: (p) => p.__epoch ?? 0 };
  let askedCount = 0;
  const order = { asked: () => ++askedCount, noteSent: (p, n) => { p.__number = n; } };
  const reader = createTimelineReader({
    decoded: async (path) => ({ path, messages: [], threads: [], dismissed: [] }),
    ledger, order, pageLimit: 50,
    channelId: () => "c/1", view: () => "flat", threadId: () => null,
    withThreadQuery: (path) => path, searchWalkActive: () => false,
    storeChannel: () => "c/1", selectedChannel: () => "c/1",
    cover: () => undefined, complete: () => null, mutationAwaits: () => false,
    refreshFailed: () => false, now: () => 100000,
  });
  const payload = await reader.readTimeline("/x");
  assert.equal(payload.__number, 1, "the read was not counted as it was sent");
  assert.deepStrictEqual(noted, [[payload, 5]]);
  assert.equal(reader.timelinePath("9", null, "poll"), "/api/v1/channels/c%2F1/timeline?view=flat&limit=50&before=9&reason=poll");
  assert.equal(reader.readReasonOf({ reason: ["poll", "live"] }), "poll");
  assert.equal(reader.readReasonOf({ readAs: "side", reason: "poll" }), "side");
  assert.equal(reader.readReasonOf({}), null);
  assert.equal(reader.withReadReason("/a?x=1", { reason: "enter" }), "/a?x=1&reason=enter");
  assert.equal(reader.olderReadReason(), "older");
  const a = { messages: [{ id: "1" }], threads: [], dismissed: [], __epoch: 3 };
  const b = { messages: [{ id: "2" }], threads: [{ id: "t" }], dismissed: ["9"], __epoch: 7 };
  const joined = reader.joinedDeltas([a, b]);
  assert.deepStrictEqual(joined.messages.map((m) => m.id), ["1", "2"]);
  assert.deepStrictEqual(noted[noted.length - 1], [joined, 3], "joined deltas were not as old as their oldest page");
  assert.equal(reader.landedAnswer([]), null);
  assert.equal(reader.landedAnswer([a]).delta, null);
});
test("`#624 voice-page-modularity`: reads given up on hold their view and their late answers stand or not, from the module alone", async () => {
  const { createAbandonedReads, createLiveEvents, createReadHolds } = SYNC;
  let now = 1000;
  const page = { threading: true, hiding: false, context: '["c","main",null]',
    selected: "c", store: { channel: "c" }, asked: 7, generation: 3 };
  const live = createLiveEvents();
  const holds = createReadHolds({ now: () => now });
  const reads = createAbandonedReads({ holds, live,
    threading: () => page.threading, hiding: () => page.hiding,
    contextKey: () => page.context, selectedChannel: () => page.selected,
    currentStore: () => page.store, asked: () => page.asked, generation: () => page.generation });
  const read = (over = {}) => ({ key: reads.holdKey(), channel: "c", sentAt: now,
    live: live.seen("c"), news: live.news("c"), canon: page.store, number: page.asked,
    generation: page.generation, ...over });
  const deferred = () => { let resolve, reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject }; };

  // holdKey: the to-do list is its own view only without threads, while read is hidden.
  assert.equal(reads.holdKey(), page.context);
  page.threading = false; page.hiding = true;
  assert.equal(reads.holdKey(), `${page.context} todo`);
  page.hiding = false;
  assert.equal(reads.holdKey(), page.context);
  page.threading = true;

  // No news since it was sent: the view is held, the hold goes before the answer is used,
  // and settled hears of it after, with nothing unused.
  const a = deferred(); const landed = []; const settledA = [];
  reads.abandon(a.promise, read(), (p) => {
    assert.equal(reads.held(reads.holdKey(), { reason: "poll" }), false,
      "the hold was still up while its answer was being landed");
    landed.push(p);
  }, (ok, unused) => settledA.push([ok, unused]));
  assert.equal(reads.held(reads.holdKey(), { reason: "poll" }), true);
  assert.equal(reads.held(reads.holdKey(), { reason: "reset" }), false, "a non-poll read waited");
  assert.equal(reads.held(reads.holdKey()), false);
  a.resolve("page");
  await a.promise; await Promise.resolve();
  assert.deepStrictEqual([landed, settledA], [["page"], [[true, null]]]);

  // A failure releases the hold too, lands nothing, and settled says so.
  const b = deferred(); const settledB = [];
  reads.abandon(b.promise, read(), () => assert.fail("a failed read landed"),
    (ok, unused) => settledB.push([ok, unused]));
  assert.equal(reads.held(reads.holdKey(), { reason: "poll" }), true);
  b.reject(new Error("slow"));
  await b.promise.catch(() => {}); await Promise.resolve();
  assert.deepStrictEqual(settledB, [[false, null]]);
  assert.equal(reads.held(reads.holdKey(), { reason: "poll" }), false);

  // News after it was sent: no hold at all, and the answer still lands while nothing
  // newer has replaced it.
  const c = read();
  live.noteNews("c");
  const landedC = [];
  reads.abandon(Promise.resolve("late"), c, (p) => landedC.push(p));
  assert.equal(reads.held(c.key, { reason: "poll" }), false, "news did not prevent the hold");
  await Promise.resolve(); await Promise.resolve();
  assert.deepStrictEqual(landedC, ["late"]);
});
