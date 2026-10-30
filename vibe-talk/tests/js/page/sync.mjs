// The app page's tests: keeping the page current: refreshes, the live stream, the cache, and late or failing reads.
//
// One feature's file of the suite tests/js/voice_page.test.mjs runs, against the fixture in
// ./fixture.mjs. Run with the rest of the suite, or alone: node --test tests/js/page/sync.mjs.

import assert from "node:assert/strict";

import { ALICE, BOTTOM_SLACK_PX, CHANNEL, CHARS_PER_LINE, COUNTS_ALL_READ_MS, CSS_CODE,
  DISCORD_POLL_MS, FakeElement, HELP_SLUGS, HTML, HTML_CODE, MESSAGE_CACHE_KEY, OTHER_CHANNEL,
  OWNERS_MESSAGE, PAGE_CODE, PAGE_ELEMENTS, PAGE_IDS, PAGE_READ_REASONS, PULL_ARM_PX,
  RETRY_FIRST_MS, RUN_THREAD, SCRIPT, SLOW_SEND, TIMELINE_RECONCILE_MS, TOKEN_STORAGE_KEY,
  arrowStyle, assertCurrent, assertMarkupContains, atBottomOf, atTheHead, backlog, bodyOf,
  bridgeReply, brokenModule, brokenScript, brokenScriptEach, browsingPage, builtSince, channelReads,
  channelRow, chooseReadMode, clampedAsShownLikeChromium, clickRow, countingChannel, cssBlock,
  delayedReply, deliver, distinctReplies, doneButton, errorResponse, fillTranscript,
  firstCardInView, focusName, focused, forwardPage, forwardReadKinds, forwardTimeline,
  freshMessagePage, freshness, gate, groupsIn, helpEntry, inPlaceRows, inTwoColumns, json,
  jumpSaysArrived, jumpShown, keptAt, keyedChannel, longMessage, manyThreads, mdBoxes, message,
  messageCacheStatus, newPage, nodeWithinFixture, offline, openBrowser, openHelp, openReplyOn,
  openerOf, optionalSourceConstant, outgoingDismiss, outgoingEdit, outgoingRetry, outgoingRows,
  outgoingSpinner, pagedTimeline, pickThread, pinnedPlacePage, pollOnce, posted, postedWhere, press,
  proposal, proposedDuringACall, pullDown, reReadChannel, readViewsSince, readerOn, readsSince,
  relayed, reloadPage, reloadWith, reopenedChannel, replayedFrames, replyArrowOf, replyBoxAt,
  replyButton, replyDismissals, replyQuietly, retryWait, retryingFirst, row, rowOf, rowOnScreen,
  rowShowing, rowWithId, runId, runThread, savedCache, savedOutgoing, savedScopes, scopeKey, search,
  searchHistory, searchMoment, sendChannel, sentOk, sentWithoutKey, setReadNew, settingsGroup,
  settleSend, showDiscord, showPinned, shownIds, sideAtFoot, sideIds, sideRow, signIn,
  sourceConstant, speakRelay, sseDelete, sseMessage, sseUpdate, startTalking, tagOf, tagged,
  tallChannel, tallColumns, test, threadBadge, threadButton, threadData, threadOptions, threadPage,
  timelineAnswer, turnTodoOn, twoColumnServer, twoColumns, unreadChip, viewOf, watchTurns, wideCard,
  withLiveChannel, withReplies, yOf } from "./fixture.mjs";

test("returning to the channel RE-READS it, instead of showing what was true the first time", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, [message({ id: "1", content: "old" })]);

  await page.el("view-switch").click(); // into the channel
  await page.settle();
  assert.equal(page.reads, 1);

  await page.el("view-switch").click(); // back to voice
  await page.settle();

  page.messages = [message({ id: "1", content: "old" }), message({ id: "2", content: "posted while you were away" })];
  await page.el("view-switch").click(); // and back into the channel
  await page.settle();

  assert.equal(page.reads, 2, "the channel was not re-read on re-entry — this is the staleness bug");
  const lines = page.el("discord-log").children;
  assert.equal(lines.length, 2);
  assert.match(lines[1].text(), /posted while you were away/);
});

test("the channel keeps pulling for as long as you are looking at it", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, [message({ id: "1", content: "one" })]);

  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.reads, 1);

  page.messages = [message({ id: "1", content: "one" }), message({ id: "2", content: "two" })];
  assert.equal(page.expireTimers(45000), 1, "no poll was armed after the channel loaded");
  await page.settle();

  assert.equal(page.reads, 2, "the timer did not re-read the channel");
  assert.equal(page.el("discord-log").children.length, 2);

  // And it re-arms, so the second poll is not the last one.
  assert.equal(page.expireTimers(45000), 1, "polling stopped after a single tick");
});

test("polling stops when the channel is no longer the view you are on", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, [message({ id: "1", content: "one" })]);

  await page.el("view-switch").click();
  await page.settle();
  await page.el("view-switch").click(); // back to voice
  await page.settle();

  const before = page.reads;
  assert.equal(page.expireTimers(45000), 0, "a poll was left armed after leaving the channel view");
  await page.settle();
  assert.equal(page.reads, before, "the channel was read while nobody was looking at it");
});

test("a background refresh does NOT drag a reader who has scrolled up", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, tallChannel());
  await page.el("view-switch").click();
  await page.settle();

  const area = page.el("scroll-area");
  // Reading older messages: parked well above the bottom of a channel that really overflows.
  assert.ok(area.scrollHeight > area.clientHeight * 2, "the channel does not overflow");
  area.scrollTop = 120;

  page.messages = [...tallChannel(), message({ id: "13", content: longMessage("brand new") })];
  page.expireTimers(45000);
  await page.settle();

  assert.equal(page.reads, 2, "the poll did not fire");
  assert.equal(area.scrollTop, 120, "a refresh nobody asked for yanked the reader to the bottom");
});

test("a background refresh DOES follow the newest line for a reader already at the bottom", async () => {
  const page = newPage();
  await signIn(page);
  countingChannel(page, tallChannel());
  await page.el("view-switch").click();
  await page.settle();

  const area = page.el("scroll-area");
  area.scrollTop = area.scrollHeight - area.clientHeight; // pinned to the bottom
  const before = area.scrollTop;

  page.messages = [...tallChannel(), message({ id: "13", content: longMessage("brand new") })];
  page.expireTimers(45000);
  await page.settle();

  assert.ok(area.scrollTop > before, "the reader was at the bottom and did not follow");
  assert.ok(atBottomOf(area), "it followed, but not to the newest message");
});

test("REFRESH keeps your place too — asking for fresh messages is not asking to be moved", async () => {
  // The button is a re-read of the channel already in front of you. Being thrown to the bottom of
  // it is the same defect as a background poll doing so.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, tallChannel());
  const area = page.el("scroll-area");
  area.scrollTop = 120;

  page.messages = [...tallChannel(), message({ id: "13", content: longMessage("brand new") })];
  await reReadChannel(page);

  assert.equal(page.el("discord-log").children.length, 13, "Refresh did not re-read the channel");
  assert.equal(area.scrollTop, 120, "Refresh threw the reader to the bottom");
});

test("a Discord read that fails reports itself and does NOT hang up on you", async () => {
  const page = newPage();
  await startTalking(page);
  page.channelPage = async () => json(502, { error: "discord_error", detail: "rate limited" });

  await page.el("view-switch").click();
  await page.settle();

  assert.match(page.el("error").textContent, /rate limited/);
  assert.equal(page.tracks[0].stops, 0, "a failed channel read tore down the call");
  assert.notEqual(page.sockets[0].readyState, 3, "a failed channel read hung up");
});

test("a message arriving on the stream is rendered with its id, without a refresh", async () => {
  const page = newPage();
  const stream = await withLiveChannel(page, [message({ id: "100", content: "already here" })]);
  const before = page.el("discord-log").children.length;

  await deliver(
    page,
    stream,
    sseMessage(message({ id: "200", author: "claude-integ", content: "the runner came back" }))
  );

  const rows = page.el("discord-log").children;
  assert.equal(rows.length, before + 1, "the arriving message never reached the list");
  const arrived = rows[rows.length - 1];
  assert.equal(arrived.getAttribute("data-id"), "200");
  assert.match(arrived.text(), /the runner came back/);
  assert.match(arrived.text(), /claude-integ/, "a live row carries its author like a fetched one");
  // The id identifies the row without being printed on it, exactly as on a fetched row.
  assert.doesNotMatch(arrived.text(), /id 200/, "a live row still prints the id the fetch dropped");
});

test("a replayed message the page already shows renders once, not twice", async () => {
  // The reconnection case: the server's replay tail legitimately re-sends what this page may
  // already hold, and de-duplication is what makes "must not duplicate" true rather than hoped.
  const page = newPage();
  const stream = await withLiveChannel(page, []);
  const arriving = message({ id: "300", content: "landed" });

  await deliver(page, stream, sseMessage(arriving));
  await deliver(page, stream, sseMessage(arriving));

  const shown = [...page.el("discord-log").children].filter(
    (li) => li.getAttribute("data-id") === "300"
  );
  assert.equal(shown.length, 1, "the same message was rendered twice");
});

test("live updates and deletes re-read the authoritative channel without losing scroll intent", async () => {
  const page = newPage();
  const original = Array.from({ length: 12 }, (_unused, index) =>
    message({
      id: `350-${index}`,
      content: index === 0 ? "short message 0" : longMessage(`message ${index}`),
    })
  );
  const stream = await withLiveChannel(page, original);
  const area = page.el("scroll-area");
  const anchorId = "350-8";
  let anchor = [...page.el("discord-log").children].find(
    (row) => row.getAttribute("data-id") === anchorId
  );
  area.scrollTop = anchor.offsetTop() - 40;
  const topBeforeEdit = anchor.getBoundingClientRect().top;
  assert.ok(!atBottomOf(area), "the reader is not parked in older messages");
  const readsBeforeUpdate = page.pageReads;

  const edited = { ...original[0], content: longMessage("after edit, much taller") };
  page.messages = [edited, ...original.slice(1)];
  await deliver(page, stream, sseUpdate("change-350", edited));
  assert.equal(page.pageReads, readsBeforeUpdate + 1, "an update left the fetched row stale");
  assert.match(page.el("discord-log").text(), /after edit/);
  anchor = [...page.el("discord-log").children].find(
    (row) => row.getAttribute("data-id") === anchorId
  );
  assert.equal(
    anchor.getBoundingClientRect().top,
    topBeforeEdit,
    "an edit above the viewport changed which row the reader was looking at"
  );

  const readsBeforeDelete = page.pageReads;
  const topBeforeDelete = anchor.getBoundingClientRect().top;
  page.messages = original.slice(1);
  await deliver(page, stream, sseDelete("delete-350", CHANNEL.id, edited.id));
  assert.equal(page.pageReads, readsBeforeDelete + 1, "a delete left the fetched row stale");
  assert.equal(page.el("discord-log").children.length, 11);
  anchor = [...page.el("discord-log").children].find(
    (row) => row.getAttribute("data-id") === anchorId
  );
  assert.equal(
    anchor.getBoundingClientRect().top,
    topBeforeDelete,
    "a deletion above the viewport changed which row the reader was looking at"
  );
});

// --- `#333 page-live-reads-remainder`: a view shown again shows the words another view's read brought --
//
// The one part of the shelved page-live-reads work still worth having on its own (`queue-prep` survey):
// a held view compared its rows with the store by id alone (`catchUpHeldView`), so when a read of one
// view brought an edited copy of a message another held view shows — an agent's placeholder edited into
// its answer, read in its thread — that view went on drawing the old words when shown again, and the
// other column did too, until its own next read, up to a poll later. #306 done-stale-across-views closed
// the same gap for Done. Now the rows are compared word for word, and a copy that says the same draws
// nothing.

/** The line of `catchUpHeldView` that compares the rows' words, put back to ids alone for a negative control. */
const HELD_COMPARES_IDS = ["(!words || threads || projected.every((item, at) => item === held[at] || sameWords(held[at], item)))",
  "true"];
/** ...and compared as JSON, in the order the fields came in (`sameMessage`), for another. */
const HELD_COMPARES_JSON = ["item === held[at] || sameWords(held[at], item)", "item === held[at] || sameMessage(held[at], item)"];

/**
 * The channel of `threadData` on a server with a change record, read in All, then Main and the thread of
 * root 201 each picked once, so each holds rows of its own; then `change` made on the server and the thread
 * read again by its poll, which brings it. Answers the page, the server, Main's rows as they were drawn
 * before, and the reads made from the change on.
 */
async function threadReadBrings(change, script = SCRIPT) {
  const page = newPage(new Map(), script);
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  const server = forwardTimeline(page);
  await signIn(page);
  await showDiscord(page, data.messages);
  await pickThread(page, "main");
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the premise is Main on screen");
  const drawn = [...page.el("discord-log").children];
  await pickThread(page, "thread:spaces/A/threads/one & two");
  assert.deepStrictEqual(shownIds(page), ["201", "202"], "the premise is the thread on screen");
  const reads = page.timelineCalls.length;
  change(server, page);
  await reReadChannel(page);
  for (let i = 0; i < 2; i += 1) await page.settle();
  assert.deepStrictEqual(readViewsSince(page, reads), ["thread"], "the premise is one read, of the thread");
  return { page, server, drawn, reads };
}

test("`#333 page-live-reads-remainder`: Main shown again draws the words a read of a thread brought for its root, with no read of its own", async () => {
  const edited = "first thread root, edited into its answer";
  const run = async (script = SCRIPT) => {
    const { page, reads } = await threadReadBrings((server) => server.edit("201", { content: edited }), script);
    assert.equal(rowText(page, "201"), edited, "the premise is the thread drawing the edit its read brought");
    await pickThread(page, "main");
    assert.deepStrictEqual(readViewsSince(page, reads), ["thread"], "Main was read to draw what the store held already");
    return page;
  };
  const page = await run();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.equal(rowText(page, "201"), edited, "Main shown again drew the root's old words");
  // THE CONTROL: rows compared by id alone, Main draws the old words until its own next read.
  const control = await run(brokenScript(...HELD_COMPARES_IDS));
  assert.equal(rowText(control, "201"), "first thread root", "compared by id alone Main drew the edit, so this test cannot see it");
});

test("`#333 page-live-reads-remainder`: Main shown again keeps its very rows when a read of a thread brought its root's words again in a new object, its fields in another order", async () => {
  const run = async (script = SCRIPT) => {
    const { page, drawn, reads } = await threadReadBrings((server, p) => {
      // The same words, served afresh with their fields the other way round, and changed on the server,
      // so the thread's read carries them.
      const at = p.messages.findIndex((m) => String(m.id) === "201");
      p.messages[at] = Object.fromEntries(Object.entries(p.messages[at]).reverse());
      server.edit("201", {});
    }, script);
    await pickThread(page, "main");
    assert.deepStrictEqual(readViewsSince(page, reads), ["thread"]);
    return { page, drawn };
  };
  const { page, drawn } = await run();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  assert.equal(rowText(page, "201"), "first thread root");
  assert.ok(page.el("discord-log").children.every((li, at) => li === drawn[at]),
    "Main shown again was drawn afresh for a root that says what it said");
  // THE CONTROL: compared as JSON, the fields' order alone draws Main again.
  const control = await run(brokenScript(...HELD_COMPARES_JSON));
  assert.ok(!control.page.el("discord-log").children.every((li, at) => li === control.drawn[at]),
    "compared as JSON Main kept its rows, so this test cannot see a redraw");
});

test("`#333 page-live-reads-remainder`: in two columns, a read of the thread on the right that brings its root's edit draws it in Main on the left, with no read of Main", async () => {
  const edited = "first thread root, edited into its answer";
  const run = async (script = SCRIPT) => {
    const { page, server } = await twoColumnServer(null, script);
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the premise is Main on the left");
    await threadBadge(channelRow(page, "201")).click();
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.deepStrictEqual(sideIds(page), ["201", "202"], "the premise is the thread opened on the right");
    // The poll reads Main, then the thread on the right; the root is edited between the two, so only the
    // thread's read brings the edit.
    const serve = page.timeline;
    let reads = null;
    page.timeline = async (path, options) => {
      const response = await serve(path, options);
      if (viewOf(path) !== "main" || reads !== null) return response;
      const body = await response.text();
      server.edit("201", { content: edited });
      reads = page.timelineCalls.length;
      return { ...response, text: async () => body };
    };
    page.expireTimers(DISCORD_POLL_MS);
    for (let i = 0; i < 4; i += 1) await page.settle();
    assert.ok(reads !== null, "the premise is the poll's read of Main");
    assert.deepStrictEqual(readViewsSince(page, reads), ["thread"], "the premise is one read since the edit, of the thread on the right");
    assert.equal(bodyOf(sideRow(page, "201")).text(), edited, "the premise is the right column drawing the edit its read brought");
    return page;
  };
  const page = await run();
  assert.equal(rowText(page, "201"), edited, "Main on the left drew the root's old words beside its edit on the right");
  // THE CONTROL: rows compared by id alone, the left column keeps the old words.
  const control = await run(brokenScript(...HELD_COMPARES_IDS));
  assert.equal(rowText(control, "201"), "first thread root", "compared by id alone Main drew the edit, so this test cannot see it");
});

test("a reload draws the saved timeline before any request answers, then merges the refresh", async () => {
  const first = newPage();
  first.threadingSupported = true;
  await signIn(first);
  const saved = [
    message({ id: "501", content: "first saved" }),
    message({ id: "502", content: "a typo in the saved copy" }),
    message({ id: "503", content: "third saved" }),
  ];
  await showDiscord(first, saved);
  assert.deepStrictEqual(
    savedScopes(first)[scopeKey(CHANNEL.id)].messages.map((m) => m.id),
    ["501", "502", "503"],
    "a successful read was not saved for the next start"
  );

  let config = null;
  let timeline = null;
  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    config = gate(p.clientConfig);
    timeline = gate(p.timeline);
    p.clientConfig = config.respond;
    p.timeline = timeline.respond;
  });
  // Synchronously: nothing has answered, nothing has even been awaited.
  assert.equal(page.screen(), "main", "a reload with a saved channel waited for the network");
  assert.deepStrictEqual(shownIds(page), ["501", "502", "503"], "the saved rows were not drawn first");
  assert.equal(page.el("discord-channel").value, CHANNEL.id, "the picker waited for /client-config");
  // `#189 restore-ui-state`: back on the channel it was left on, in the view it opened in — All.
  // Its one read waits for /client-config to prove the token.
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  assert.deepStrictEqual(page.timelineCalls, [], "the reopen read the channel with a token nobody had proved");

  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["501", "502", "503"], "the reopen dropped the saved rows");
  assert.equal(freshness(page).hidden, false, "saved rows were presented as current");
  // Nothing is being read yet, and the pill does not say otherwise.
  assert.match(freshness(page).textContent, /^Showing messages saved \d{2}:\d{2}$/);
  assert.deepStrictEqual(page.timelineCalls, [], "the reopen read before /client-config answered");

  page.messages = [
    saved[0],
    { ...saved[1], content: "the corrected text" },
    saved[2],
    message({ id: "504", content: "arrived while away" }),
  ];
  config.open();
  await page.settle();
  assert.deepStrictEqual(page.timelineCalls.map((call) => new URL(call, "http://fixture.test").searchParams.get("view")),
    ["flat"], "the reopen did not read the view on screen, once");
  assert.deepStrictEqual(shownIds(page), ["501", "502", "503"], "sign-in dropped the saved rows");
  assert.match(freshness(page).textContent, /^Saved \d{2}:\d{2} · refreshing…$/);
  timeline.open();
  await page.settle();
  await page.settle();

  assert.deepStrictEqual(shownIds(page), ["501", "502", "503", "504"], "the refresh duplicated or lost rows");
  assert.match(page.el("discord-log").children[1].text(), /the corrected text/);
  assert.doesNotMatch(page.el("discord-log").text(), /typo/, "a corrected row kept its stale text");
  assertCurrent(page, "a successful refresh still says the rows are saved");
  const entry = savedScopes(page)[scopeKey(CHANNEL.id)];
  assert.deepStrictEqual(entry.messages.map((m) => m.id), ["501", "502", "503", "504"]);
  assert.equal(entry.messages[1].content, "the corrected text", "the correction was not saved");
});

test("a legacy page-mode channel is saved, drawn first on reload, and merged the same way", async () => {
  const first = newPage();
  await signIn(first);
  await showDiscord(first, [message({ id: "601", content: "one" }), message({ id: "602", content: "two" })]);

  let reads = null;
  const page = reloadPage(first.storage, (p) => {
    reads = gate(p.channelPage);
    p.channelPage = reads.respond;
  });
  // Drawn before anything answered — from the device, not by the request — and still up while the
  // reopen's one read, sent once sign-in has answered, is out.
  assert.deepStrictEqual(shownIds(page), ["601", "602"]);
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  assert.equal(page.pageReads, 0, "the reopen read the channel before /client-config answered");
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["601", "602"]);
  assert.equal(page.pageReads, 1, "the reopen did not make its one read");

  page.messages = [message({ id: "602", content: "two, edited" }), message({ id: "603", content: "three" })];
  reads.open();
  await page.settle();
  await page.settle();
  // No `has_more`: this page IS the whole channel, so 601 was deleted upstream and goes; the saved
  // copy of 602 gives way to the edit, and nothing appears twice.
  assert.deepStrictEqual(shownIds(page), ["602", "603"]);
  assert.match(page.el("discord-log").children[0].text(), /two, edited/);
  assertCurrent(page, "a legacy-mode refresh still says the rows are saved");
});

test("a saved rendered body is drawn again from the device, and one saved before the field as text", async () => {
  // `#217 markdown-blocks`. The device keeps each message as the server sent it, so a reload draws
  // the rendered list before any request answers. A message saved by a page from before the field
  // existed has only its text, and is drawn as text: never handed to the HTML sink.
  const first = newPage();
  first.threadingSupported = true;
  await signIn(first);
  await showDiscord(first, [
    message({ id: "701", ...OWNERS_MESSAGE }),
    message({ id: "702", content: "**old** saved copy", content_html: "<p><strong>old</strong> saved copy</p>\n" }),
  ]);
  const cache = savedCache(first);
  const entry = cache.scopes[scopeKey(CHANNEL.id)];
  assert.equal(entry.messages[0].content_html, OWNERS_MESSAGE.content_html, "the rendered body was not saved");
  delete entry.messages[1].content_html;
  first.storage.set(MESSAGE_CACHE_KEY, JSON.stringify(cache));

  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    p.timeline = offline;
  });
  const parsed = [];
  page.onParse = (html) => parsed.push(html);
  const [rendered, plain] = page.el("discord-log").children;
  assert.equal(tagged(mdBoxes(rendered)[0], "li").length, 6, "the saved list was not drawn as a list");
  assert.equal(tagged(mdBoxes(plain)[0], "strong").length, 0, "a saved body without HTML was rendered");
  assert.equal(mdBoxes(plain)[0].text(), "**old** saved copy");
  await page.settle();
  assert.ok(!parsed.includes("**old** saved copy"), "the old saved text reached the HTML sink");
});

test("offline, the saved rows stay up and say how old they are, and the next poll recovers", async () => {
  const first = newPage();
  await signIn(first);
  await showDiscord(first, [message({ id: "701", content: "kept offline" })]);

  const page = reloadPage(first.storage, (p) => {
    p.onlineConfig = p.clientConfig;
    p.clientConfig = offline;
    p.channelPage = offline;
  });
  await page.settle();
  assert.equal(page.screen(), "main");
  // Reopened on the channel, whose read failed like everything else. `#189 restore-ui-state`.
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  await page.settle();

  assert.deepStrictEqual(shownIds(page), ["701"], "a failed refresh took the saved rows away");
  assert.equal(freshness(page).hidden, false);
  assert.match(freshness(page).textContent, /^Offline · showing messages saved \d{2}:\d{2}$/);
  assert.match(page.el("error").textContent, /Could not reach vibe-talk/);
  assert.equal(page.streamOpens.length, 0);

  // The network comes back. The poll is still armed although every read so far failed, and the
  // page that never heard /client-config asks for it before reading.
  page.clientConfig = page.onlineConfig;
  page.channelPage = async () =>
    json(200, { channel: CHANNEL, messages: [message({ id: "701", content: "kept offline" }),
      message({ id: "702", content: "after reconnecting" })], has_more: false });
  assert.ok(page.expireTimers(DISCORD_POLL_MS) > 0, "a failed read stopped the channel polling");
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["701", "702"]);
  assertCurrent(page, "a recovered read still says it is offline");
  assert.equal(page.streamOpens.length, 1, "the late /client-config never attached the live stream");
});

test("a refresh the SERVER fails is reported as a failure, not as being offline", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "711", content: "on screen" })]);
  page.channelPage = errorResponse(502, "discord_error", "the provider is down");
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), ["711"]);
  assert.match(freshness(page).textContent, /^Refresh failed · showing messages from \d{2}:\d{2}$/);
  assert.equal(freshness(page).getAttribute("data-state"), "failed");
  // ...and it gives way to the pull-to-refresh pill, which sits in exactly the same place.
  page.el("pull-refresh").hidden = false;
  await page.el("view-switch").click();
  await page.el("view-switch").click();
  assert.equal(freshness(page).hidden, true, "two pills stacked in one place");
});

// `#32 freshness-pill-overlap`. The pill floats over the head of the list, so the pane makes room
// for it there for exactly as long as it has something to say — and keeps that room while the pill
// gives way to a pull, because the list jumping mid-gesture is worse than the pill's absence.
test("the channel pane reserves the freshness pill's room for as long as it has something to say", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "721", content: "on screen" })]);
  const reserved = () => page.el("pane-discord").hasAttribute("data-freshness");
  const failing = errorResponse(502, "discord_error", "the provider is down");
  // The pill has something to say from the first read on, so its room is held from then: the
  // list never shifts as the pill changes between current, failed, and current again.
  assertCurrent(page, "a freshly read channel did not say it is current");
  assert.equal(reserved(), true, "the current-state pill sits over the list's header");
  page.channelPage = failing;
  await reReadChannel(page);
  assert.equal(freshness(page).hidden, false);
  assert.equal(reserved(), true, "the failed-refresh pill sits over the list's header");
  page.channelPage = async () =>
    json(200, { channel: CHANNEL, messages: [message({ id: "721", content: "on screen" })], has_more: false });
  await reReadChannel(page);
  assertCurrent(page, "a recovered channel did not say it is current again");
  assert.equal(reserved(), true, "a recovered channel moved the list by dropping the pill's room");
  page.channelPage = failing;
  await reReadChannel(page);
  assert.equal(reserved(), true);
  page.el("pull-refresh").hidden = false;
  await page.el("view-switch").click();
  await page.el("view-switch").click();
  assert.equal(freshness(page).hidden, true);
  assert.equal(reserved(), true, "giving way to the pull moved the list under the finger");
});

test("each scope keeps at most MESSAGE_CACHE_ROWS rows, and a reload walks back from the oldest kept", async () => {
  const rows = sourceConstant("MESSAGE_CACHE_ROWS");
  const page = newPage();
  await signIn(page);
  const all = [];
  for (let i = 0; i < rows + 80; i += 1) all.push(message({ id: String(10000 + i), content: `m${i}` }));
  page.channelPage = async () =>
    json(200, { channel: CHANNEL, messages: all, has_more: true, next_before: all[0].id });
  await showDiscord(page, all);

  const entry = savedScopes(page)[scopeKey(CHANNEL.id)];
  assert.equal(entry.messages.length, rows, "the per-scope bound was not applied");
  assert.equal(entry.messages[0].id, all[80].id, "the bound kept the oldest rows rather than the newest");
  assert.equal(entry.more, true, "a trimmed snapshot claims to be the whole channel");
  assert.equal(entry.cursor, all[80].id);

  const paths = [];
  const again = reloadPage(page.storage, (p) => {
    p.channelPage = async (path) => {
      paths.push(String(path));
      // The reopen's newest page is the rows kept (`#189 restore-ui-state`); a step back from the
      // oldest of them is the twenty before.
      const older = /[?&]before=/.test(String(path));
      return json(200, { channel: CHANNEL, messages: older ? all.slice(60, 80) : all.slice(80), has_more: true,
        next_before: older ? all[60].id : all[80].id });
    };
  });
  assert.equal(again.el("discord-log").children.length, rows);
  assert.equal(again.el("load-older").hidden, false, "the trimmed snapshot offered no way further back");
  await again.settle();
  assert.equal(again.el("discord-log").children.length, rows, "the reopen's newest page changed the rows kept");
  await again.el("load-older").click();
  await again.settle();
  assert.match(paths.at(-1), new RegExp(`[?&]before=${all[80].id}(&|$)`));
  assert.equal(again.el("discord-log").children.length, rows + 20);
});

test("at most MESSAGE_CACHE_SCOPES scopes are kept, and the least recently opened go first", async () => {
  const limit = sourceConstant("MESSAGE_CACHE_SCOPES");
  const page = newPage();
  const channels = [];
  for (let i = 0; i < limit + 2; i += 1) {
    channels.push({ id: String(1120000000000000000n + BigInt(i)), label: `c${i}`, writable: true });
  }
  page.channels = channels;
  await signIn(page);
  page.channelPage = async (path) => {
    const id = /channels\/([^/]+)\/page/.exec(String(path))[1];
    return json(200, { channel: knownOf(id), messages: [message({ id: `${id}9`, channel_id: id })], has_more: false });
  };
  const knownOf = (id) => channels.find((c) => c.id === id);
  await showDiscord(page, []);
  for (let i = 1; i < channels.length; i += 1) {
    page.setClock(page.clock() + 1000);
    page.el("discord-channel").value = channels[i].id;
    await page.el("discord-channel").dispatch("change");
    await page.settle();
  }
  const kept = Object.keys(savedScopes(page));
  assert.equal(kept.length, limit, "the scope bound was not applied");
  assert.ok(!kept.includes(channels[0].id) && !kept.includes(channels[1].id),
    "the least recently opened scopes survived");
  assert.ok(kept.includes(channels.at(-1).id), "the scope on screen was evicted");
});

test("the whole snapshot stays inside MESSAGE_CACHE_CHARS, halving the current scope if it must", async () => {
  const budget = sourceConstant("MESSAGE_CACHE_CHARS");
  const page = newPage();
  await signIn(page);
  const big = [];
  for (let i = 0; i < 100; i += 1) big.push(message({ id: String(20000 + i), content: `${i} ${"x".repeat(9000)}` }));
  await showDiscord(page, big);
  const stored = page.storage.get(MESSAGE_CACHE_KEY);
  assert.ok(stored.length <= budget, `the snapshot is ${stored.length} units, over ${budget}`);
  const entry = savedScopes(page)[scopeKey(CHANNEL.id)];
  assert.ok(entry.messages.length > 0 && entry.messages.length < 100);
  assert.equal(entry.messages.at(-1).id, big.at(-1).id, "halving kept the old end of the channel");
  assert.equal(entry.more, true);
  assert.equal(entry.cursor, entry.messages[0].id);
});

test("storage refusing the snapshot costs nothing but the snapshot", async () => {
  const page = newPage();
  const set = page.storage.set.bind(page.storage);
  page.storage.set = (key, value) => {
    if (key === MESSAGE_CACHE_KEY) throw new Error("QuotaExceededError");
    return set(key, value);
  };
  await signIn(page);
  await showDiscord(page, [message({ id: "801", content: "still readable" })]);
  assert.deepStrictEqual(shownIds(page), ["801"]);
  assert.equal(page.storage.has(MESSAGE_CACHE_KEY), false);
  assert.equal(page.el("error").hidden, true, "a refused convenience was reported as an error");
});

test("a full quota gives the snapshot's space to an unsent message", async () => {
  const page = newPage();
  await signIn(page);
  const big = [];
  for (let i = 0; i < 20; i += 1) big.push(message({ id: String(30000 + i), content: "y".repeat(5000) }));
  await showDiscord(page, big);
  assert.ok(page.storage.has(MESSAGE_CACHE_KEY));
  const total = () => [...page.storage].reduce((n, [k, v]) => n + k.length + String(v).length, 0);
  const budget = total() + 40;
  page.storage.set = (key, value) => {
    const previous = page.storage.has(key) ? key.length + String(page.storage.get(key)).length : 0;
    if (total() - previous + key.length + String(value).length > budget) throw new Error("QuotaExceededError");
    return Map.prototype.set.call(page.storage, key, value);
  };
  delayedReply(page);
  await page.el("channel-compose-text").setValue("this must survive a reload");
  page.el("channel-send").click();
  assert.equal(savedOutgoing(page).length, 1, "the unsent message lost to the saved channel");
  assert.equal(page.storage.has(MESSAGE_CACHE_KEY), false);
  assert.doesNotMatch(outgoingRows(page)[0].text(), /could not save/);
});

test("a damaged snapshot is discarded without breaking the start, and a damaged scope alone", async () => {
  const store = new Map([[TOKEN_STORAGE_KEY, "write-token-aaaaaaaaaaaaaaaa"], [MESSAGE_CACHE_KEY, "{\"v\":1,\"ident"]]);
  const page = reloadPage(store);
  assert.equal(store.has(MESSAGE_CACHE_KEY), false, "an interrupted write was kept");
  await page.settle();
  assert.equal(page.screen(), "main");

  await showDiscord(page, [message({ id: "901", content: "good scope" })]);
  const cache = savedCache(page);
  cache.scopes["1110000000000000077"] = { mode: "page", messages: "not a list" };
  page.storage.set(MESSAGE_CACHE_KEY, JSON.stringify(cache));
  const again = reloadPage(page.storage);
  assert.deepStrictEqual(shownIds(again), ["901"], "one damaged entry took the good one with it");
  assert.equal("1110000000000000077" in savedScopes(again), false, "the damaged entry was kept");
});

test("changing the token clears every saved and shown message before the new one is used", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1001", content: "readable by the first token" })]);
  assert.ok(page.storage.has(MESSAGE_CACHE_KEY));

  page.el("api-token").value = "write-token-bbbbbbbbbbbbbbbb";
  page.el("save-token").click();
  assert.equal(page.storage.has(MESSAGE_CACHE_KEY), false, "another credential inherited the saved rows");
  assert.deepStrictEqual(shownIds(page), [], "another credential inherited the rows on screen");
  await page.settle();
  assert.doesNotMatch(page.renderedText(), /readable by the first token/);
});

test("signing out removes the saved messages and the rows on screen", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1101", content: "private" })]);
  await page.el("forget-token").click();
  assert.equal(page.storage.has(MESSAGE_CACHE_KEY), false);
  assert.deepStrictEqual(shownIds(page), []);
  assert.doesNotMatch(page.renderedText(), /private/);
});

test("a snapshot saved for one token is never drawn for another", async () => {
  // The main app shares this origin and this token key, so the token can change underneath the
  // page without its sign-out control ever being pressed.
  const first = newPage();
  await signIn(first);
  await showDiscord(first, [message({ id: "1201", content: "first identity only" })]);
  first.storage.set(TOKEN_STORAGE_KEY, "write-token-cccccccccccccccc");
  const page = reloadPage(first.storage, (p) => {
    p.clientConfig = () => new Promise(() => {});
  });
  assert.deepStrictEqual(shownIds(page), []);
  assert.equal(page.storage.has(MESSAGE_CACHE_KEY), false, "the other identity's snapshot survived");
  assert.equal(page.el("screen-main").hidden, true, "the app was shown from a snapshot this token did not make");
  // `#625 signin-flash`: nor is this token, not yet refused, asked for again while it is checked.
  assert.equal(page.el("screen-signin").hidden, true, "the sign-in form came up for a token nobody had refused");
});

// `#625 signin-flash`. The owner: "whenever I refresh the page on desktop I would see the sign in form
// for just a second even though I was already signed in." The form was served visible, so the browser
// drew it while /voice.js loaded, and with no saved shell to draw the page left it up until
// /client-config answered. Now it is served hidden and shown only with no token or after a refusal.

/** Arrange a page to note every time the sign-in screen is shown, before its script runs. */
const noteSigninShown = (shown) => (p) => {
  const screen = p.el("screen-signin");
  let hidden = screen.hidden;
  Object.defineProperty(screen, "hidden", {
    configurable: true,
    get: () => hidden,
    set: (value) => {
      if (hidden && !value) shown.push(p.el("status").textContent);
      hidden = value;
    },
  });
};

test("with a token saved, a reload never shows the sign-in form while the token is checked", async () => {
  // Served hidden, as every screen is: which one comes up is the script's to say.
  assert.match(HTML_CODE, /<section id="screen-signin" class="screen"[^>]*\shidden>/, "the sign-in form is served on screen");

  // No saved shell to draw: no screen at all while the token is checked, and a status saying so.
  const first = newPage();
  await signIn(first);
  first.storage.delete(MESSAGE_CACHE_KEY);
  const shown = [];
  let config = null;
  const page = reloadPage(first.storage, (p) => {
    noteSigninShown(shown)(p);
    config = gate(p.clientConfig);
    p.clientConfig = config.respond;
  });
  await page.settle();
  assert.equal(page.el("screen-signin").hidden, true, "the sign-in form is up while a saved token is checked");
  assert.equal(page.el("screen-main").hidden, true, "the app is up on a token nobody has proved, with nothing saved to show");
  assert.equal(page.el("status").textContent, "Checking your token…");
  config.open();
  await page.settle();
  await page.settle();
  assert.equal(page.screen(), "main", "the proved token did not open the app");
  assert.deepStrictEqual(shown, [], "the sign-in form came up during the load");

  // A saved shell: the app at once, and the form never.
  const saved = newPage();
  await signIn(saved);
  await showDiscord(saved, [message({ id: "6251", content: "saved for the next load" })]);
  const quiet = [];
  const held = { gate: null };
  const hydrated = reloadPage(saved.storage, (p) => {
    noteSigninShown(quiet)(p);
    held.gate = gate(p.clientConfig);
    p.clientConfig = held.gate.respond;
  });
  assert.equal(hydrated.screen(), "main", "a reload with a saved shell waited for the network");
  held.gate.open();
  await hydrated.settle();
  await hydrated.settle();
  assert.equal(hydrated.screen(), "main");
  assert.deepStrictEqual(quiet, [], "the sign-in form came up under a saved shell");

  // Refused: the form, once, after the answer and not before it.
  const refusedShown = [];
  const refused = reloadPage(new Map([[TOKEN_STORAGE_KEY, "write-token-eeeeeeeeeeeeeeee"]]), (p) => {
    noteSigninShown(refusedShown)(p);
    p.clientConfig = errorResponse(401, "unauthorized", "token revoked");
  });
  assert.deepStrictEqual(refusedShown, [], "the form came up before the server refused the token");
  await refused.settle();
  assert.equal(refused.screen(), "signin", "a refused token did not bring the form back");
  assert.equal(refusedShown.length, 1);

  // No token at all: the form, straight away.
  const fresh = newPage();
  assert.equal(fresh.screen(), "signin", "with no token the sign-in form is not shown");

  // THE CONTROL: with the startup's no-token branch showing the form whatever is saved, this sees it.
  const control = [];
  const flashing = newPage(first.storage, brokenScript('} else if (token()) {\n  if (hydrateFromCache()) {',
    '} else if (token()) {\n  showScreen("signin");\n  if (hydrateFromCache()) {'), (p) => {
    noteSigninShown(control)(p);
    p.clientConfig = () => new Promise(() => {});
  });
  await flashing.settle();
  assert.equal(control.length, 1, "the test cannot see the form come up during a load");
});

test("a token replaced in another tab never inherits the rows this page is showing", async () => {
  // The main app and other tabs share the token. This page is not told; it has to notice before it
  // merges into, or saves, rows the previous token read.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1251", content: "read by the old token" })]);
  page.storage.set(TOKEN_STORAGE_KEY, "write-token-dddddddddddddddd");
  page.stream().push(sseMessage(message({ id: "1252", content: "arrives on the old stream" })));
  await page.settle();
  assert.doesNotMatch(JSON.stringify(savedCache(page) || {}), /old token|old stream/,
    "the previous token's rows were saved under the new one");
  assert.deepStrictEqual(shownIds(page), [], "the previous token's rows stayed on screen");

  page.channelPage = async () =>
    json(200, { channel: CHANNEL, messages: [message({ id: "1253", content: "read by the new token" })], has_more: true });
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), ["1253"]);
  assert.deepStrictEqual(savedScopes(page)[scopeKey(CHANNEL.id)].messages.map((m) => m.id), ["1253"]);
});

test("a refusal at start removes the snapshot it had already drawn", async () => {
  const first = newPage();
  await signIn(first);
  await showDiscord(first, [message({ id: "1301", content: "revoked" })]);
  const page = reloadPage(first.storage, (p) => {
    p.clientConfig = errorResponse(401, "unauthorized", "token revoked");
  });
  assert.deepStrictEqual(shownIds(page), ["1301"]);
  await page.settle();
  assert.equal(page.screen(), "signin");
  assert.deepStrictEqual(shownIds(page), [], "a revoked token's rows stayed on screen");
  assert.equal(page.storage.has(MESSAGE_CACHE_KEY), false, "a revoked token's rows stayed on the device");
});

test("a refused channel read removes that credential's messages too", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "1401", content: "no longer allowed" })]);
  page.channelPage = errorResponse(401, "unauthorized", "token revoked");
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), []);
  assert.equal(page.storage.has(MESSAGE_CACHE_KEY), false);
});

test("a channel the server no longer lists takes its saved scopes with it", async () => {
  const page = newPage();
  const other = { id: "1110000000000000009", label: "other", writable: true };
  page.channels = [{ ...CHANNEL }, other];
  await signIn(page);
  await showDiscord(page, [message({ id: "1501", content: "in lead team" })]);
  page.channels = [other];
  const again = reloadPage(page.storage, (p) => {
    p.channels = [other];
  });
  await again.settle();
  assert.equal(scopeKey(CHANNEL.id) in savedScopes(again), false, "an unlisted channel's rows stayed");
  assert.deepStrictEqual(shownIds(again), [], "an unlisted channel's rows stayed on screen");
});

test("one store per channel: every view it covered is drawn after a reload with no request", async () => {
  // Opened in All, the default (`#189 restore-ui-state`); Main and the Threads list read on top. A
  // thread is a subset of All, drawn from All's cover, so it costs no read of its own.
  const first = await threadPage("main");
  const data = threadData();
  await first.el("channel-view-threads").click();
  await first.settle();
  await openerOf(first.el("thread-list").children[0], "thread-card-open").click();
  await first.settle();
  await first.el("thread-back").click();
  await first.el("channel-view-flat").click();
  await first.settle();
  const entry = savedScopes(first)[scopeKey(CHANNEL.id)];
  assert.deepStrictEqual(entry.messages.map((m) => m.id), ["200", "201", "202", "203", "204"],
    "the channel's rows were not held once, in time order");
  assert.deepStrictEqual(entry.threads.map((t) => t.id), data.threads.map((t) => t.id));
  assert.deepStrictEqual(Object.keys(entry.views).sort(), ["flat", "main", "threads"]);

  let reads = null;
  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    reads = gate(p.timeline);
    p.timeline = reads.respond;
  });
  // Reopened on All, where it was left, with that view's read on the wire and its rows up first.
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  await page.settle();
  // Exactly ONE bounded newest page, for the channel and view on screen — not every saved view,
  // not every saved channel, not a walk back through history.
  assert.equal(channelReads(page).length, 1, `a cold start made ${channelReads(page).length} history reads`);
  const refresh = new URL(channelReads(page)[0].slice(4), "http://fixture.test");
  assert.equal(refresh.searchParams.get("view"), "flat");
  assert.equal(refresh.searchParams.get("before"), null, "the cold-start refresh walked back into history");
  assert.equal(refresh.searchParams.get("limit"), String(sourceConstant("DISCORD_PAGE_LIMIT")));

  page.el("channel-view-threads").click();
  await page.settle();
  assert.deepStrictEqual(page.el("thread-list").children.map((card) => card.getAttribute("data-context-id")),
    data.threads.map((t) => t.id));
  openerOf(page.el("thread-list").children[0], "thread-card-open").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["201", "202"]);
  assert.equal(wideCard(page).text, "first thread root");
  page.el("thread-back").click();
  page.el("channel-view-main").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  page.el("channel-view-flat").click();
  await page.settle();
  assert.equal(channelReads(page).length, 1, "switching among covered views went to the network");

  page.messages = data.messages;
  reads.open();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204"]);
  assert.equal(channelReads(page).length, 1);
  assertCurrent(page, `the refreshed view still says: ${freshness(page).textContent}`);

  // Main was not refreshed: it says when it was saved, and does not claim to be refreshing.
  await page.el("channel-view-main").click();
  assert.equal(channelReads(page).length, 1);
  assert.match(freshness(page).textContent, /^Showing messages saved \d{2}:\d{2}$/);
  // The poll refreshes the view on screen, and only that one.
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  await page.settle();
  assert.equal(channelReads(page).length, 2);
  assert.match(channelReads(page)[1], /view=main/);
  assertCurrent(page, "the polled Main view still says it is saved");
});

test("a refresh that lands after the reader switched away still makes its view current", async () => {
  const first = await threadPage();
  const data = threadData();
  await first.el("channel-view-flat").click();
  await first.settle();
  await first.el("channel-view-main").click();
  await first.settle();

  let reads = null;
  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    reads = gate(p.timeline);
    p.timeline = reads.respond;
  });
  // Reopened on Main, the view the reader chose last. `#189 restore-ui-state`.
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  await page.settle();
  assert.equal(channelReads(page).length, 1);
  assert.match(channelReads(page)[0], /view=main/);
  // Away before the cold-start page for Main arrives.
  page.el("channel-view-flat").click();
  await page.settle();
  page.messages = [...data.messages, message({ id: "206", content: "posted elsewhere meanwhile" })];
  reads.open();
  await page.settle();
  await page.settle();
  assert.equal(channelReads(page).length, 1, "the page for Main was read again for All");

  page.el("channel-view-main").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "206"], "Main dropped the page read for it");
  assertCurrent(page, `Main still says: ${freshness(page).textContent}`);
  assert.equal(channelReads(page).length, 1, "returning to Main went to the network");
  assert.ok(savedScopes(page)[scopeKey(CHANNEL.id)].messages.some((m) => m.id === "206"),
    "the late page was not saved");
});

test("switching Main, Threads, All and a read thread in one page is local, and each sees live rows", async () => {
  const page = await threadPage();
  const data = threadData();
  await page.el("channel-view-threads").click();
  await openerOf(page.el("thread-list").children[0], "thread-card-open").click();
  await page.el("thread-back").click();
  await page.el("channel-view-flat").click();
  await page.el("channel-view-main").click();
  await page.settle();

  // A reply arrives on the stream while Main — where replies do not show — is on screen. The page
  // re-reads the view on screen for the server's counts, as it always has; nothing else.
  const first = data.threads[0];
  const reads = channelReads(page).length;
  page.stream().push(sseMessage(message({ id: "205", content: "a live reply", thread: { ...data.messages[2].thread } })));
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(channelReads(page).slice(reads).map((r) => new URL(r.slice(4), "http://x").searchParams.get("view")),
    ["main"], "a live arrival re-read more than the view on screen");
  const before = channelReads(page).length;
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "a reply was drawn in Main");

  await page.el("channel-view-flat").click();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204", "205"], "All missed the live reply");
  await page.el("channel-view-threads").click();
  assert.equal(page.el("thread-list").children.at(-1).getAttribute("data-context-id"), first.id,
    "the thread with the newest activity did not move to the bottom");
  await openerOf(page.el("thread-list").children.at(-1), "thread-card-open").click();
  assert.deepStrictEqual(shownIds(page), ["201", "202", "205"], "the thread missed its own live reply");
  await page.el("thread-back").click();
  await page.el("channel-view-main").click();
  await page.settle();
  assert.equal(channelReads(page).length, before, "switching among read views went to the network");
});

test("a view no page has covered is read, once", async () => {
  // All is covered by the read that opened the channel in it (`#189 restore-ui-state`), and so Main
  // is, from the same point up — every row of Main is a row of All (`#220 view-switch-instant`).
  // The Threads list is not: its summaries are no projection of All's messages.
  const page = await threadPage();
  const before = channelReads(page).length;
  await page.el("channel-view-main").click();
  await page.settle();
  assert.equal(channelReads(page).length, before, "Main was read though All covers it");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"]);
  await page.el("channel-view-threads").click();
  await page.settle();
  assert.equal(channelReads(page).length, before + 1);
  assert.match(channelReads(page).at(-1), /view=threads/);
  await page.el("channel-view-flat").click();
  await page.el("channel-view-main").click();
  await page.el("channel-view-threads").click();
  await page.settle();
  assert.equal(channelReads(page).length, before + 1, "a covered view was read again");
});

test("a cold start with several saved channels reads only the one on screen", async () => {
  const other = { id: "1110000000000000004", label: "other", writable: true };
  const first = newPage();
  first.channels = [{ ...CHANNEL }, other];
  await signIn(first);
  await showDiscord(first, [message({ id: "1801", content: "lead" })]);
  first.el("discord-channel").value = other.id;
  first.channelPage = async () => json(200, { channel: other,
    messages: [message({ id: "1802", channel_id: other.id, content: "other" })], has_more: false });
  await first.el("discord-channel").dispatch("change");
  await first.settle();
  first.el("discord-channel").value = CHANNEL.id;
  first.channelPage = async () => json(200, { channel: CHANNEL, messages: [message({ id: "1801", content: "lead" })], has_more: false });
  await first.el("discord-channel").dispatch("change");
  await first.settle();
  assert.deepStrictEqual(Object.keys(savedScopes(first)).sort(), [CHANNEL.id, other.id].sort());

  const page = reloadPage(first.storage, (p) => {
    p.channels = [{ ...CHANNEL }, other];
  });
  await reopenedChannel(page);
  assert.equal(channelReads(page).length, 1, `a cold start read ${channelReads(page).length} channels`);
  assert.match(channelReads(page)[0], new RegExp(`/channels/${CHANNEL.id}/page\\?`));
  assert.doesNotMatch(channelReads(page)[0], /before=/, "the cold-start refresh walked back into history");
  assert.equal(page.streamOpens.length, 1, "the page did not fall back on the live stream");
});

test("a saved timeline waits for a live cursor, and a step back over the overlap duplicates nothing", async () => {
  const limit = 3;
  const all = [];
  for (let i = 0; i < 9; i += 1) all.push(message({ id: String(40000 + i), content: `t${i}` }));
  const serve = (p) => async (path) => {
    const url = new URL(path, "http://fixture.test");
    const before = url.searchParams.get("before");
    const end = before ? Number(before.split(":")[1]) : all.length;
    const start = Math.max(0, end - limit);
    return json(200, timelineAnswer({ messages: all.slice(start, end), limit,
      has_more: start > 0, next_before: start > 0 ? `cursor:${start}` : null }));
  };
  const first = newPage();
  first.threadingSupported = true;
  first.timeline = serve(first);
  await signIn(first);
  await showDiscord(first, []);
  await first.el("load-older").click();
  await first.settle();
  assert.deepStrictEqual(shownIds(first), all.slice(3).map((m) => m.id));

  let reads = null;
  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    reads = gate(serve(p));
    p.timeline = reads.respond;
  });
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  assert.deepStrictEqual(shownIds(page), all.slice(3).map((m) => m.id));
  assert.equal(page.el("load-older").hidden, true, "a saved snapshot offered an expired cursor");
  assert.match(page.el("channel-summary").text(), /older ones are not loaded/);
  reads.open();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), all.slice(3).map((m) => m.id), "the refresh dropped the saved rows above it");
  assert.equal(page.el("load-older").hidden, false, "the refresh's live cursor was not adopted");
  await page.el("load-older").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), all.slice(3).map((m) => m.id), "the overlapping step duplicated rows");
  await page.el("load-older").click();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), all.map((m) => m.id));
  assert.equal(new Set(shownIds(page)).size, all.length);
});

test("a delivered send is shown once from the snapshot, and only the server retires its receipt", async () => {
  const first = newPage();
  await signIn(first);
  await showDiscord(first, []);
  const finish = delayedReply(first);
  await first.el("channel-compose-text").setValue("sent before the reload");
  const submitted = first.el("channel-send").click();
  await first.settle();
  finish(json(200, { posted: message({ id: "1601", content: "sent before the reload" }) }));
  await submitted;
  await first.settle();
  assert.deepStrictEqual(savedScopes(first)[scopeKey(CHANNEL.id)].messages.map((m) => m.id), ["1601"],
    "the acknowledged send was not saved");
  assert.equal(savedOutgoing(first).length, 1);

  let reads = null;
  const page = reloadPage(first.storage, (p) => {
    reads = gate(p.channelPage);
    p.channelPage = reads.respond;
  });
  assert.equal(page.tab(), "discord", "a reload of a page left on the channel did not reopen there");
  assert.deepStrictEqual(shownIds(page), ["1601"]);
  assert.equal(outgoingRows(page).length, 0, "the delivered send was drawn twice");
  assert.equal(savedOutgoing(page).length, 1, "the device's own copy retired the receipt");
  page.messages = [message({ id: "1601", content: "sent before the reload" })];
  reads.open();
  await page.settle();
  await page.settle();
  assert.equal(savedOutgoing(page).length, 0, "the server's copy did not retire the receipt");
});

test("read state and live arrivals are saved as they happen", async () => {
  const page = newPage();
  page.dealtWith.add("1702");
  page.messages = [message({ id: "1701", content: "open" }), message({ id: "1702", content: "done already" })];
  await signIn(page);
  await page.el("view-switch").click();
  await page.settle();
  assert.deepStrictEqual(savedScopes(page)[scopeKey(CHANNEL.id)].dismissed, ["1702"]);

  await doneButton(row(page, 0)).click();
  await page.settle();
  assert.deepStrictEqual(savedScopes(page)[scopeKey(CHANNEL.id)].dismissed.sort(), ["1701", "1702"],
    "a local Done was not saved");

  page.stream().push(sseMessage(message({ id: "1703", content: "live" })));
  await page.settle();
  assert.deepStrictEqual(savedScopes(page)[scopeKey(CHANNEL.id)].messages.map((m) => m.id), ["1701", "1702", "1703"],
    "a live arrival was not saved");

  const again = reloadPage(page.storage, (p) => {
    p.channelPage = () => new Promise(() => {});
  });
  assert.deepStrictEqual(shownIds(again), ["1701", "1702", "1703"]);
  assert.equal(again.el("discord-log").children[0].getAttribute("data-archived"), "true");
  assert.equal(again.el("discord-log").children[2].getAttribute("data-archived"), "false");
});

test("no private response is ever handed to a service worker's CacheStorage", () => {
  // The snapshot lives in this origin's localStorage, scoped to the token and removed with it.
  // CacheStorage would outlive sign-out and be served by whatever a worker decides.
  assert.doesNotMatch(PAGE_CODE, /\bcaches\b|serviceWorker|CacheStorage/);
  assert.doesNotMatch(HTML_CODE, /serviceWorker|caches\./);
});

test("a channel added or removed here is in the saved picker the next cold start draws", async () => {
  // `#19 channel-browser` and the add form change the list between two `/client-config` answers;
  // the saved shell follows them, or a cold start draws the picker as it was before the change.
  const page = browsingPage();
  await openBrowser(page);
  const row = page.el("channel-directory-list").children[1];
  await row.children.find((kid) => kid.tagName === "button").click();
  await page.settle();
  const added = String(page.registeredChannelId);
  assert.ok(
    savedCache(page).shell.channels.some((channel) => String(channel.id) === added),
    "the browsed channel was added, but the saved picker does not have it"
  );

  let config = null;
  const reloaded = reloadPage(page.storage, (p) => {
    p.channelRegistrationSupported = true;
    p.channelDiscoverySupported = true;
    p.channels = page.channels;
    config = gate(p.clientConfig);
    p.clientConfig = config.respond;
  });
  assert.ok(
    reloaded.el("discord-channel").children.some((option) => option.value === added),
    "the cold start drew the picker from before the add"
  );
  config.open();
  await reloaded.settle();

  await reloaded.el("open-settings").click();
  reloaded.el("settings-channel").value = added;
  reloaded.el("settings-channel").dispatch("change");
  await reloaded.el("remove-channel").click();
  await reloaded.settle();
  assert.deepStrictEqual(reloaded.removeChannelCalls, [added]);
  assert.ok(
    !savedCache(reloaded).shell.channels.some((channel) => String(channel.id) === added),
    "the channel was removed, but the saved picker still offers it"
  );
});

// --- a reload's replay burst ----------------------------------------------------------------------
//
// `#43 replay-burst-double-read`. A stream that attaches without a Last-Event-ID is replayed the
// server's whole tail, every frame marked `replayed`. On a threaded channel each message frame
// used to re-read the newest page, so a warm reload made two identical reads of it — one of them
// on the voice pane. These count the reads.

/**
 * A warm threaded page, saved to the device and reloaded over the same storage.
 *
 * Left in Main and on the call view, so the reload opens there (`#189 restore-ui-state`) and its
 * stream attaches before anything enters the channel — the order every count below is about.
 */
async function warmThreadReload(arrange) {
  const first = newPage();
  const data = threadData();
  first.threadingSupported = true;
  first.threads = data.threads;
  await signIn(first);
  await showDiscord(first, data.messages);
  await pickThread(first, "main");
  await first.el("view-switch").click();
  const page = reloadPage(first.storage, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    p.messages = [...data.messages];
    if (arrange) arrange(p);
  });
  await page.settle();
  assert.equal(page.streamOpens.length, 1, "the reload never attached its live stream");
  return { page, data };
}

test("a replay of messages the page already holds adds no read, on either pane", async () => {
  // On the voice pane: no read at all, because nothing refreshes a channel nobody is looking at.
  const { page: voice, data } = await warmThreadReload();
  const held = [data.messages[0], data.messages[3]];
  assert.equal(voice.timelineCalls.length, 0, "the voice pane read the channel before any frame");
  await deliver(voice, voice.stream(), replayedFrames(held));
  await voice.settle();
  assert.equal(voice.timelineCalls.length, 0, "a replay of held messages read the hidden channel");

  // On the channel pane: the entry read, and nothing queued behind it, even when the whole burst
  // lands while that read is still on the wire.
  let timeline = null;
  const { page } = await warmThreadReload((p) => {
    timeline = gate(p.timeline);
    p.timeline = timeline.respond;
  });
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "entering the channel did not read it");
  await deliver(page, page.stream(), replayedFrames(held));
  timeline.open();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "a replay burst queued a second read of the same page");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the replay changed Main");
});

test("a replayed message the page does not hold is merged by ONE read, in its own thread", async () => {
  // Said while the tab was shut: a new Main message, and a reply in the first thread. On the voice
  // pane that costs nothing: entering the channel reads it, and that one read merges both.
  const { page, data } = await warmThreadReload();
  const root = message({ id: "210", content: "posted while you were away" });
  const reply = message({ id: "211", content: "answered while you were away",
    thread: { ...data.messages[1].thread, is_root: false } });
  page.messages.push(root, reply);
  page.messages[1] = withReplies(page.messages[1], 2);
  await deliver(page, page.stream(), replayedFrames([data.messages[0], root, data.messages[3], reply]));
  await page.settle();
  assert.equal(page.timelineCalls.length, 0, "a replay read the channel on the voice pane");

  await page.el("view-switch").click();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "entering the channel did not read once for the burst");
  assert.ok(shownIds(page).includes("210"), "the new Main message never reached the list");
  assert.ok(!shownIds(page).includes("211"), "a thread reply was dropped into Main");
  // Only a read can say the thread grew: the frame carries no count the page could add up.
  assert.equal(threadButton(rowShowing(page, "first thread root")).textContent, "2 replies",
    "the read did not merge the thread the replayed reply joined");
});

test("a live frame during a read still gets the follow-up read that read cannot answer for", async () => {
  let timeline = null;
  const { page } = await warmThreadReload((p) => {
    timeline = gate(p.timeline);
    p.timeline = timeline.respond;
  });
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "entering the channel did not read it");
  const arriving = message({ id: "220", content: "said just now" });
  page.messages.push(arriving);
  await deliver(page, page.stream(), sseMessage(arriving));
  timeline.open();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 2, "a live frame during a read lost its follow-up read");
  assert.ok(shownIds(page).includes("220"), "the live message never reached the list");
});

test("a catch-up message the adapter sends after the entry read still gets its read", async () => {
  // The server marks the adapter's catch-up `replayed` too, so it may arrive after every read the
  // page has made — a replay is not proof that a read already saw it.
  const { page, data } = await warmThreadReload();
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "entering the channel did not read it");
  const reply = message({ id: "230", content: "caught up after the adapter resumed",
    thread: { ...data.messages[1].thread, is_root: false } });
  page.messages.push(reply);
  page.messages[1] = withReplies(page.messages[1], 2);
  await deliver(page, page.stream(), sseMessage(reply, { replayed: true }));
  await page.settle();
  assert.equal(page.timelineCalls.length, 2, "a catch-up message was taken as already read");
  assert.equal(threadButton(rowShowing(page, "first thread root")).textContent, "2 replies",
    "the thread the catch-up reply joined kept its old count");
});

test("a replayed copy never replaces the newer copy a read gave the store", async () => {
  // The tail captured 201 when it was posted, before its thread had a reply.
  const { page, data } = await warmThreadReload();
  await deliver(page, page.stream(), replayedFrames([withReplies(data.messages[1], 0)]));
  await page.settle();
  assert.equal(page.timelineCalls.length, 0, "a held message was read again");
  // The device keeps what the store kept: reload with a read that never answers, and look.
  const again = reloadPage(page.storage, (p) => {
    p.threadingSupported = true;
    p.threads = data.threads;
    p.messages = [...data.messages];
    p.timeline = gate(p.timeline).respond;
  });
  await again.settle();
  await again.el("view-switch").click();
  await again.settle();
  assert.equal(threadButton(rowShowing(again, "first thread root")).textContent, "1 reply",
    "the replayed copy's stale count was saved over the read's");
});

/** A threaded page on CHANNEL, switching to OTHER_CHANNEL with its entry read held on the wire. */
async function coldChannelChange(entryPage) {
  const page = newPage();
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.channels = [{ ...CHANNEL }, { ...OTHER_CHANNEL }];
  await signIn(page);
  await showDiscord(page, data.messages);
  // Each read answers with the page as it stood when the read went out, however late it lands.
  let served = entryPage;
  const timeline = gate(async (answer) => json(200, timelineAnswer({ channel: { ...CHANNEL, ...OTHER_CHANNEL }, messages: answer })));
  page.timeline = () => timeline.respond(served);
  const before = page.timelineCalls.length;
  page.el("discord-channel").value = OTHER_CHANNEL.id;
  const changed = page.el("discord-channel").dispatch("change");
  await page.settle();
  return {
    page,
    reads: () => page.timelineCalls.length - before,
    serve: (messages) => { served = messages; },
    land: async () => {
      timeline.open();
      await changed;
      await page.settle();
      await page.settle();
      // A read that failed leaves the rows the frames delivered, and would count as one read.
      assert.equal(page.el("error").hidden, true, `a read failed: ${page.el("error").textContent}`);
    },
  };
}

test("a channel change reads once for its own replay burst", async () => {
  // Its read goes out before its stream attaches, so the burst lands while that read is out.
  const other = [
    message({ id: "900", channel_id: OTHER_CHANNEL.id, content: "other one" }),
    message({ id: "901", channel_id: OTHER_CHANNEL.id, content: "other two" }),
  ];
  const change = await coldChannelChange(other);
  assert.equal(change.reads(), 1, "the channel change did not read the channel");
  await deliver(change.page, change.page.stream(), replayedFrames(other));
  await change.land();
  assert.equal(change.reads(), 1, "the burst queued a second read of the page the first returned");
  assert.deepStrictEqual(shownIds(change.page), ["900", "901"]);
});

test("a replayed message the landed read lacks gets exactly one more read", async () => {
  // Stored between the entry read and the attach: in the burst, but not on the page read before it.
  const first = message({ id: "900", channel_id: OTHER_CHANNEL.id, content: "other one" });
  const late = message({ id: "902", channel_id: OTHER_CHANNEL.id, content: "stored after the read" });
  const change = await coldChannelChange([first]);
  await deliver(change.page, change.page.stream(), replayedFrames([first, late]));
  change.serve([first, late]);
  await change.land();
  assert.equal(change.reads(), 2, "the message the entry read lacked was not read, or read twice");
  assert.deepStrictEqual(shownIds(change.page), ["900", "902"]);
});

test("a reconnect's replay reads nothing for what the page holds, and once for what it lacks", async () => {
  const { page, data } = await warmThreadReload();
  await page.el("view-switch").click();
  await page.settle();
  await deliver(page, page.stream(), sseMessage(data.messages[3]));
  await page.settle();
  const before = page.timelineCalls.length;
  page.stream().drop();
  await page.settle();
  page.expireTimers(sourceConstant("LIVE_RETRY_MS"));
  await page.settle();
  assert.equal(page.streamOpens.length, 2, "the dropped stream did not reconnect");
  assert.equal(page.streamOpens[1].lastEventId, "203", "the reconnect did not resume");

  await deliver(page, page.stream(), replayedFrames([data.messages[3]]));
  await page.settle();
  assert.equal(page.timelineCalls.length, before, "a reconnect re-read for a message the page holds");

  const missed = message({ id: "240", content: "said during the drop" });
  page.messages.push(missed);
  await deliver(page, page.stream(), replayedFrames([missed]));
  await page.settle();
  assert.equal(page.timelineCalls.length, before + 1, "the message missed in the drop was not read once");
  assert.ok(shownIds(page).includes("240"), "the message missed in the drop never reached the list");
});

test("a replay arriving after a read that began after the attach reads nothing, even for a reply", async () => {
  // The stream attached before the channel was entered, so the entry read saw the whole tail —
  // including a reply in a thread Main does not show, which no page of Main could prove it saw.
  const { page, data } = await warmThreadReload();
  const reply = message({ id: "250", content: "answered before the reload",
    thread: { ...data.messages[1].thread, is_root: false } });
  page.messages.push(reply);
  page.messages[1] = withReplies(page.messages[1], 2);
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "entering the channel did not read it");
  await deliver(page, page.stream(), replayedFrames([data.messages[0], reply]));
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "a replay the entry read had already seen was read again");
  assert.equal(threadButton(rowShowing(page, "first thread root")).textContent, "2 replies");
});

test("a replayed reply no page can show costs one more read, never a loop", async () => {
  // A channel change reads before its stream attaches, and a reply is never on Main's page, so
  // only a read begun after it can answer for it — one read, however many pages lack it.
  const root = message({ id: "900", channel_id: OTHER_CHANNEL.id, content: "other root",
    thread: { id: "spaces/B/threads/t", root_message_id: "900", is_root: true, reply_count: 1,
      reply_count_exact: true } });
  const reply = message({ id: "903", channel_id: OTHER_CHANNEL.id, content: "other reply",
    thread: { ...root.thread, is_root: false } });
  const change = await coldChannelChange([root]);
  await deliver(change.page, change.page.stream(), replayedFrames([root, reply]));
  await change.land();
  await change.page.settle();
  assert.equal(change.reads(), 2, "a reply in the tail was not read once more, or was read forever");
  assert.deepStrictEqual(shownIds(change.page), ["900"], "the reply was dropped into Main");
});

test("an edit or removal out of the tail costs no read once a read has begun after the attach", async () => {
  // Until one has, the device's copy may predate the edit. The voice pane still reads nothing for
  // it: the read entering the channel makes began after the attach, so it has the edit.
  const { page: voice, data: saved } = await warmThreadReload();
  voice.messages[0] = { ...saved.messages[0], content: "edited while away" };
  await deliver(voice, voice.stream(),
    sseUpdate("u0", voice.messages[0], { replayed: true, from_tail: true }));
  await voice.settle();
  await voice.settle();
  assert.equal(voice.timelineCalls.length, 0, "a tail edit read the channel on the voice pane");
  await voice.el("view-switch").click();
  await voice.settle();
  await voice.settle();
  assert.equal(voice.timelineCalls.length, 1, "entering the channel did not read it once");
  assert.ok(rowShowing(voice, "edited while away"), "the entry read did not bring the edit");

  const { page, data } = await warmThreadReload();
  await page.el("view-switch").click();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "entering the channel did not read it");
  const edited = { ...data.messages[0], content: "edited before the reload" };
  await deliver(page, page.stream(),
    sseUpdate("u1", edited, { replayed: true, from_tail: true }) +
    sseDelete("d1", CHANNEL.id, "203", { replayed: true, from_tail: true }));
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "an edit the entry read had already seen was read again");

  // The adapter's catch-up edit may be newer than that read, so it still gets one.
  await deliver(page, page.stream(), sseUpdate("u2", edited, { replayed: true }));
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 2, "a catch-up edit lost its read");
});

test("a held replay the to-do filter hides stays hidden", async () => {
  // A replayed frame for a message the store holds appends nothing: the view drawn from the store
  // left it out on purpose, and no read follows to take a stray row away again.
  const { page } = await warmThreadReload((p) => {
    p.dealtWith = new Set(["200"]);
  });
  await page.el("view-switch").click();
  await page.settle();
  await turnTodoOn(page);
  const shown = shownIds(page);
  assert.ok(!shown.includes("200"), "the to-do filter showed a dealt-with message");
  const reads = page.timelineCalls.length;
  await deliver(page, page.stream(), replayedFrames(page.messages.filter((m) => !m.thread || m.thread.is_root)));
  await page.settle();
  assert.deepStrictEqual(shownIds(page), shown, "a held replay put a hidden message back on the list");
  assert.equal(page.timelineCalls.length, reads, "a held replay read the channel");
});

test("a read that failed answers for no replay, and the next read does", async () => {
  let timeline = null;
  let healthy = null;
  const { page, data } = await warmThreadReload((p) => {
    healthy = p.timeline;
    timeline = gate(async () => errorResponse(502, "chat_error", "the provider returned HTTP 502"));
    p.timeline = timeline.respond;
  });
  await page.el("view-switch").click();
  await page.settle();
  // The entry read began after the attach, but it failed, so it saw nothing of the tail.
  timeline.open();
  await page.settle();
  await page.settle();
  assert.equal(page.el("error").hidden, false, "the failed read was not reported");
  assert.equal(page.timelineCalls.length, 1, "entering the channel did not read it once");

  page.timeline = healthy;
  page.messages[0] = { ...data.messages[0], content: "edited while away" };
  const reply = message({ id: "260", content: "answered while away",
    thread: { ...data.messages[1].thread, is_root: false } });
  page.messages.push(reply);
  page.messages[1] = withReplies(page.messages[1], 2);
  await deliver(page, page.stream(), sseUpdate("u0", page.messages[0], { replayed: true, from_tail: true }) +
    replayedFrames([reply]));
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 2, "the tail was taken as seen by a read that failed");
  assert.ok(rowShowing(page, "edited while away"), "the tail edit the failed read lacked never showed");
  assert.equal(threadButton(rowShowing(page, "first thread root")).textContent, "2 replies",
    "the thread the tail reply joined kept its old count");
});

test("a read that fails while replays wait on it is not retried for them", async () => {
  // A server that keeps failing is not asked again for every frame: the poll's read answers.
  let timeline = null;
  let healthy = null;
  const { page, data } = await warmThreadReload((p) => {
    healthy = p.timeline;
    timeline = gate(async () => errorResponse(502, "chat_error", "the provider returned HTTP 502"));
    p.timeline = timeline.respond;
  });
  await page.el("view-switch").click();
  await page.settle();
  page.messages[0] = { ...data.messages[0], content: "edited while away" };
  await deliver(page, page.stream(), sseUpdate("u0", page.messages[0], { replayed: true, from_tail: true }));
  timeline.open();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "a failed read was retried for the replay waiting on it");
  page.timeline = healthy;
  page.expireTimers(DISCORD_POLL_MS);
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 2, "the poll did not read the channel");
  assert.ok(rowShowing(page, "edited while away"), "the poll's read did not bring the edit");
});

test("the voice pane queues no read for a replay a read in flight may predate", async () => {
  // A live message reads even on the voice pane; a catch-up arriving during that read waits for
  // the read entering the channel makes, rather than queueing another hidden one.
  let timeline = null;
  const { page, data } = await warmThreadReload((p) => {
    timeline = gate(p.timeline);
    p.timeline = timeline.respond;
  });
  const live = message({ id: "270", content: "said just now" });
  const caughtUp = message({ id: "271", content: "caught up after the adapter resumed",
    thread: { ...data.messages[1].thread, is_root: false } });
  page.messages.push(live, caughtUp);
  page.messages[1] = withReplies(page.messages[1], 2);
  await deliver(page, page.stream(), sseMessage(live) + sseMessage(caughtUp, { replayed: true }));
  assert.equal(page.timelineCalls.length, 1, "a live message on the voice pane was not read");
  timeline.open();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 1, "a catch-up queued a hidden read behind the live one");
  await page.el("view-switch").click();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length, 2, "entering the channel did not read it");
  assert.equal(threadButton(rowShowing(page, "first thread root")).textContent, "2 replies");
});

test("a channel change's burst with a new message and a tail edit reads once more, not twice", async () => {
  const first = message({ id: "900", channel_id: OTHER_CHANNEL.id, content: "other one" });
  const late = message({ id: "902", channel_id: OTHER_CHANNEL.id, content: "stored after the read" });
  const change = await coldChannelChange([first]);
  await deliver(change.page, change.page.stream(), replayedFrames([first, late]) +
    sseUpdate("u0", { ...first, content: "other one, edited" }, { replayed: true, from_tail: true }));
  change.serve([{ ...first, content: "other one, edited" }, late]);
  await change.land();
  await change.page.settle();
  assert.equal(change.reads(), 2, "the burst's new message and edit did not share one more read");
  assert.deepStrictEqual(shownIds(change.page), ["900", "902"]);
  assert.ok(rowShowing(change.page, "other one, edited"), "the tail edit never showed");
});

test("a read out before the attach does not answer for a tail edit to a message its page holds", async () => {
  // The page holds the message, but a page read before the attach may predate the edit: holding
  // the message is not having seen the edit, so the edit still gets the one more read.
  const first = message({ id: "900", channel_id: OTHER_CHANNEL.id, content: "other one" });
  const edited = { ...first, content: "other one, edited" };
  const change = await coldChannelChange([first]);
  await deliver(change.page, change.page.stream(), replayedFrames([first]) +
    sseUpdate("u0", edited, { replayed: true, from_tail: true }));
  change.serve([edited]);
  await change.land();
  await change.page.settle();
  assert.equal(change.reads(), 2, "the page holding the message answered for its edit");
  assert.deepStrictEqual(shownIds(change.page), ["900"]);
  assert.ok(rowShowing(change.page, "other one, edited"), "the tail edit was lost");
});

test("a read out before the attach does not answer for a tail removal of a message its page holds", async () => {
  const other = [
    message({ id: "900", channel_id: OTHER_CHANNEL.id, content: "other one" }),
    message({ id: "901", channel_id: OTHER_CHANNEL.id, content: "other two" }),
  ];
  const change = await coldChannelChange(other);
  await deliver(change.page, change.page.stream(), replayedFrames(other) +
    sseDelete("d0", OTHER_CHANNEL.id, "901", { replayed: true, from_tail: true }));
  change.serve([other[0]]);
  await change.land();
  await change.page.settle();
  assert.equal(change.reads(), 2, "the page holding the message answered for its removal");
  assert.deepStrictEqual(shownIds(change.page), ["900"], "the tail removal was lost");
});

const RETRY_ATTEMPTS = sourceConstant("OUTGOING_RETRY_ATTEMPTS");

const CHANNEL_READ_TIMEOUT_MS = sourceConstant("CHANNEL_READ_TIMEOUT_MS");

test("a background refresh the chat service is too slow for says so in the pill, never in a banner", async () => {
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, [message({ id: "1941", content: "already on screen" })]);
  page.channelPage = errorResponse(502, "chat_error",
    'Example Chat returned HTTP 502: {"message":"history read timed out (command timed out after ' +
      '18.6s); no partial timeline was returned"}');
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), ["1941"], "a failed refresh took the rows away");
  assert.equal(page.el("error").hidden, true,
    `a refresh nobody asked for raised a banner: ${page.el("error").textContent}`);
  assert.match(freshness(page).textContent,
    /^Example Chat is slow to answer · showing messages from \d{2}:\d{2}$/);
  assert.equal(freshness(page).getAttribute("data-state"), "slow");

  // A failure that is not a timeout is still the pill's to say.
  page.channelPage = errorResponse(502, "chat_error",
    'Example Chat returned HTTP 502: {"message":"upstream command exited 1"}');
  await reReadChannel(page);
  assert.match(freshness(page).textContent, /^Refresh failed · showing messages from \d{2}:\d{2}$/);
  assert.equal(page.el("error").hidden, true, "the second failed refresh raised a banner");

  // A refresh the READER asked for keeps its banner: in words, not the server's diagnosis.
  page.el("scroll-area").scrollTop = 0;
  await pullDown(page, PULL_ARM_PX);
  assert.equal(page.el("error").hidden, false, "a pull that failed said nothing");
  assert.match(page.el("error").textContent, /^Example Chat could not answer: upstream command exited 1$/);
  assert.doesNotMatch(page.el("error").textContent, /HTTP 502|chat_error|\{/);
});

test("a failed background refresh with nothing on screen is still reported, in words", async () => {
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  page.channelPage = errorResponse(504, "error", "(no detail)");
  await showDiscord(page, []);
  assert.equal(page.el("error").hidden, false, "entering a channel that could not be read said nothing");
  await page.el("dismiss-error").click();
  await reReadChannel(page);
  assert.equal(page.el("error").hidden, false, "with no rows there is no pill to carry the failure");
  assert.equal(page.el("error").textContent, "Example Chat is slow to answer right now.");
});

test("a read of the newest messages gives up after a bound, says so, and the next poll asks again", async () => {
  // The server bounds its own read at fifty seconds; a reader watching "refreshing…" should not.
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, [message({ id: "1942", content: "already on screen" })]);
  const healthy = page.channelPage;
  const reads = page.pageReads;
  page.channelPage = () => new Promise(() => {});
  await reReadChannel(page);
  assert.equal(page.pageReads, reads + 1);
  assert.equal(page.expireTimers(CHANNEL_READ_TIMEOUT_MS), 1, "nothing bounded the read");
  await page.settle();
  assert.match(freshness(page).textContent, /^Example Chat is slow to answer · showing messages from/);
  assert.equal(page.el("error").hidden, true);
  page.channelPage = healthy;
  // The next poll is armed when the read is given up on, so it comes past the server's own bound.
  // `remote-ops-audit` W7: inside that bound it would wait (see that section, at the end).
  page.setClock(page.clock() + CHANNEL_READ_TIMEOUT_MS + DISCORD_POLL_MS);
  await reReadChannel(page);
  assert.equal(page.pageReads, reads + 2, "the abandoned read still held the next one back");
  assertCurrent(page, "a read after the abandoned one did not bring the channel back to current");

  const threaded = newPage();
  threaded.threadingSupported = true;
  await signIn(threaded);
  await showDiscord(threaded, [message({ id: "1943", content: "a threaded channel" })]);
  threaded.timeline = () => new Promise(() => {});
  await reReadChannel(threaded);
  assert.equal(threaded.expireTimers(CHANNEL_READ_TIMEOUT_MS), 1, "a timeline read was not bounded");
  await threaded.settle();
  assert.match(freshness(threaded).textContent, /is slow to answer · showing messages from/);
});

test("a send the chat service was too slow to confirm goes again on its own, under the same key", async () => {
  const page = await keyedChannel();
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 0, unsent: "did this go out", detail: SLOW_SEND, retryable: true,
    resumable: true,
  });
  await sendChannel(page, "did this go out");
  const row = outgoingRows(page)[0];
  const id = row.getAttribute("data-outgoing-id");
  assert.deepEqual(posted(page), [{ text: "did this go out", idempotency_key: id }],
    "the send carried no key a retry could be recognised by");
  assert.equal(row.getAttribute("data-send-state"), "retrying");
  assert.match(row.text(), retryingFirst("Sending… Example Chat is slow to answer right now\\. "));
  assert.doesNotMatch(row.text(), /HTTP 502|\{/, "the row shows the server's diagnosis");
  assert.equal(outgoingRetry(row).textContent, "Retry now");
  assert.equal(outgoingSpinner(row), undefined, "a row that is waiting claims to be sending");
  assert.equal(savedOutgoing(page)[0].state, "unconfirmed", "the saved state would let a reload resend");

  page.replyResponse = sentOk;
  assert.equal(page.expireTimers(RETRY_FIRST_MS), 1, "no automatic retry was armed");
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2);
  assert.deepEqual(posted(page)[1], posted(page)[0], "the retry was not the same request");
  assert.equal(outgoingRows(page).length, 0, "the confirmed retry left its receipt behind");
  assert.equal(shownIds(page).filter((shown) => shown === "9194000000000000001").length, 1);
});

test("automatic retries wait 3, 6 and 12 seconds, then 20 at most, then stop and leave Retry to the reader", async () => {
  // `#277 send-order`: 5, 15 and 45 seconds had the owner reading "Retrying in 45 s" after a
  // failure that took a moment, with the messages he wrote after it waiting just as long.
  assert.deepEqual([0, 1, 2, 3, 4, 5].map(retryWait), [3000, 6000, 12000, 20000, 20000, 20000],
    "the schedule is not the one this test is named for");
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", SLOW_SEND);
  await sendChannel(page, "keep trying");
  for (let attempt = 0; attempt < RETRY_ATTEMPTS; attempt += 1) {
    const wait = retryWait(attempt);
    assert.match(outgoingRows(page)[0].text(), new RegExp(`Retrying in ${wait / 1000} s\\.`));
    assert.ok(page.expireTimers(wait) >= 1, `no retry was armed for ${wait} ms`);
    await settleSend(page);
    assert.equal(page.repliesPosted.length, attempt + 2, `the retry after ${wait} ms did not go out`);
  }
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed", "the retries never stopped");
  assert.doesNotMatch(row.text(), /Retrying in/);
  assert.match(row.text(), /Delivery unconfirmed\. Retrying cannot post it twice\./);
  assert.equal(outgoingRetry(row).textContent, "Retry");
  page.expireTimers(retryWait(RETRY_ATTEMPTS));
  await settleSend(page);
  assert.equal(page.repliesPosted.length, RETRY_ATTEMPTS + 1, "a retry went out after the last one");
  assert.ok(posted(page).every((body) => body.idempotency_key === posted(page)[0].idempotency_key),
    "an attempt went out under another key");

  // A person tapping Retry starts the schedule again, should that attempt fail too.
  await outgoingRetry(row).click();
  await settleSend(page);
  assert.equal(page.repliesPosted.length, RETRY_ATTEMPTS + 2);
  assert.match(outgoingRows(page)[0].text(), retryingFirst());
});

test("a refusal is never sent again on its own, keyed or not", async () => {
  const page = await keyedChannel();
  // The provider said no: the server marks the 207 as not worth retrying.
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 0, unsent: "refused words", retryable: false, resumable: true,
    detail: 'Example Chat returned HTTP 400: {"message":"not allowed in this space"}',
  });
  await sendChannel(page, "refused words");
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed");
  assert.match(row.text(), /not allowed in this space/);
  assert.doesNotMatch(row.text(), /Retrying in|\{/);
  // ...and the server itself refusing, with a 4xx. Dismissed first, as the next message to the same
  // place would otherwise wait behind it (`#277 send-order`).
  await outgoingDismiss(row).click();
  page.replyResponse = errorResponse(400, "refused", "message content is empty");
  await sendChannel(page, "also refused");
  assert.equal(outgoingRows(page)[0].getAttribute("data-send-state"), "failed");
  for (const wait of [0, 1, 2, 3, 4, 5].map(retryWait)) page.expireTimers(wait);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2, "a refusal was asked again");
});

// `#444 send-refusal-reason`. The owner, replying in a thread: "the message won't send after multiple
// retries." Each was the chat service's 400, which the server answered as a 207 that said nothing
// was posted, certainly (`uncertain` false), and why; the row called it "Delivery unconfirmed.
// Retrying cannot post it twice." and sent him back to Retry.
test("a refusal the server is sure of is Not sent, says why in plain words, and is never sent again on its own", async () => {
  const page = await keyedChannel();
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 0, unsent: "@Ada can you look", retryable: false, resumable: true,
    uncertain: false,
    detail: 'Example Chat returned HTTP 400: {"message":"content contains mention syntax that Google Chat would expand"}',
    refusal: {
      status: 400, reason: "content contains mention syntax that Google Chat would expand",
      explanation: "it contains @Ada, which would notify someone",
    },
  });
  await sendChannel(page, "@Ada can you look");
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "failed");
  assert.match(row.text(),
    /Not sent\. Example Chat refused this message: it contains @Ada, which would notify someone\./);
  assert.doesNotMatch(row.text(), /unconfirmed|cannot post it twice|Retrying in|\{/);
  // Nothing of it can be in the channel, so Edit takes it back to the box to be changed.
  assert.ok(outgoingEdit(row), "a refused send offered no Edit");
  for (const wait of [0, 1, 2, 3, 4, 5].map(retryWait)) page.expireTimers(wait);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 1, "a refusal was sent again on its own");

  // Refused after a part went: that part is confirmed, the rest is not sent, and Edit stays away from
  // a send part of which is in the channel.
  await outgoingDismiss(row).click();
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 1, unsent: "the second part", retryable: false, resumable: true,
    uncertain: false, detail: 'Example Chat returned HTTP 400: {"message":"no"}',
    refusal: { status: 400, reason: "no", explanation: "no" },
  });
  await sendChannel(page, "the first part");
  const partial = outgoingRows(page)[0];
  assert.equal(partial.getAttribute("data-send-state"), "failed");
  assert.match(partial.text(), /1 part confirmed\. Not sent\. Example Chat refused this message: no\./);
  assert.equal(outgoingEdit(partial), undefined, "Edit was offered for a send partly in the channel");
});

test("where no provider promises one post per key, a lost send waits for the reader, carrying its key for the server", async () => {
  // `#448 send-dedupe`. The key goes to every provider now: the server itself answers a repeat of a
  // post that landed, or is landing, with that post. It cannot vouch for a part whose own answer was
  // lost, which only the provider could recognise, so nothing here goes again on its own.
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, []);
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 0, unsent: "maybe delivered", retryable: true, resumable: true,
    detail: "Example Chat request failed: connection reset by peer",
  });
  await sendChannel(page, "maybe delivered");
  assert.deepEqual(posted(page).map(sentWithoutKey), [{ text: "maybe delivered" }]);
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed");
  assert.match(row.text(), /Check history before retrying/);
  assert.match(row.text(), /Example Chat could not be reached right now\./);
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 1, "an ambiguous send was retried without a guarantee");

  // Several providers: only the channel whose own provider promises it is retried on its own.
  const multi = newPage();
  const space = { id: "1110000000000000301", label: "chat space", writable: true, provider: "gchat" };
  const team = { id: "C0194ABCDE", label: "team", writable: true, provider: "slack" };
  multi.channels = [team, space];
  multi.idempotentPostsSupported = true;
  multi.providers = [
    { key: "gchat", name: "Example Chat", threading_supported: false, live_delivery: "poll",
      live_poll_seconds: 30, channel_registration_supported: true, channel_discovery_supported: false,
      upstream_read_mark_supported: false, idempotent_posts_supported: true,
      self_author_id: null, owner_author_id: null },
    { key: "slack", name: "Other Chat", threading_supported: false, live_delivery: "poll",
      live_poll_seconds: 30, channel_registration_supported: true, channel_discovery_supported: false,
      upstream_read_mark_supported: false, self_author_id: null, owner_author_id: null },
  ];
  multi.channelPage = async (path) =>
    json(200, { channel: String(path).includes(space.id) ? space : team, messages: [] });
  multi.replyResponse = errorResponse(502, "chat_error", SLOW_SEND);
  await signIn(multi);
  await showDiscord(multi, []);
  // `#355 provider-grouped-channels`: the picker opens on the first channel it shows, which is Example
  // Chat's, the service configured first; the team is chosen as a reader would.
  multi.el("discord-channel").value = team.id;
  await multi.el("discord-channel").dispatch("change");
  await multi.settle();
  await sendChannel(multi, "to the team");
  multi.el("discord-channel").value = space.id;
  await multi.el("discord-channel").dispatch("change");
  await multi.settle();
  await sendChannel(multi, "to the space");
  // Both carry their keys (`#448 send-dedupe`); only the promising provider's goes again on its own.
  assert.ok(posted(multi)[0].idempotency_key, "the team's send carried no key");
  assert.ok(posted(multi)[1].idempotency_key, "the promising provider's channel sent no key");
  assert.match(outgoingRows(multi)[0].text(), retryingFirst());
  multi.expireTimers(RETRY_FIRST_MS);
  await settleSend(multi);
  assert.deepEqual(posted(multi).map((body) => body.text), ["to the team", "to the space", "to the space"],
    "the other provider's guarantee was borrowed");
});

test("a message written while the device is offline waits for the connection, then goes", async () => {
  // Nothing left the device, so this is safe through any provider: no key is needed.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, []);
  await page.setOnline(false);
  await sendChannel(page, "written in a tunnel");
  assert.equal(page.repliesPosted.length, 0, "an offline device tried to send");
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "retrying");
  assert.match(row.text(), /Offline\. Will send when online\./);
  assert.equal(savedOutgoing(page)[0].state, "failed", "a reload would claim it might have been sent");
  await page.setOnline(true);
  await settleSend(page);
  assert.deepEqual(posted(page).map(sentWithoutKey), [{ text: "written in a tunnel" }]);
  assert.equal(outgoingRows(page).length, 0);
});

test("a retry that comes due while offline waits for the connection instead", async () => {
  const page = await keyedChannel();
  page.replyResponse = async () => { throw new TypeError("Failed to fetch"); };
  await sendChannel(page, "sent as the signal dropped");
  assert.match(outgoingRows(page)[0].text(), retryingFirst());
  await page.setOnline(false);
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 1, "a retry was attempted with no connection");
  assert.match(outgoingRows(page)[0].text(), /Sending… Will retry when online\./);
  page.replyResponse = sentOk;
  await page.setOnline(true);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2, "the connection came back and nothing was sent");
  assert.deepEqual(posted(page)[1], posted(page)[0]);
  assert.equal(outgoingRows(page).length, 0);
});

test("a retry whose wait ran out while the page was hidden goes as soon as the page is back", async () => {
  // A phone holds a hidden page's timers, so the wait cannot be trusted to have fired.
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", SLOW_SEND);
  await sendChannel(page, "sent before the screen went off");
  await page.setVisibility("hidden");
  page.setClock(page.clock() + RETRY_FIRST_MS + 1000);
  page.replyResponse = sentOk;
  await page.setVisibility("visible");
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2, "coming back did not send what was due");
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2, "the held timer sent it a second time");
});

test("a reload never resumes an automatic retry, and a tapped Retry still goes under the same key", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", SLOW_SEND);
  await sendChannel(page, "before the app was closed");
  const key = posted(page)[0].idempotency_key;
  const again = reloadWith(new Map(page.storage), [], (p) => {
    p.idempotentPostsSupported = true;
  });
  await signIn(again);
  await reopenedChannel(again);
  for (const wait of [retryWait(0), retryWait(1)]) again.expireTimers(wait);
  await settleSend(again);
  assert.equal(again.repliesPosted.length, 0, "reopening the app sent a message on its own");
  const row = outgoingRows(again)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed");
  assert.match(row.text(), /Retrying cannot post it twice\./);
  again.replyResponse = sentOk;
  await outgoingRetry(row).click();
  await settleSend(again);
  assert.deepEqual(posted(again), [{ text: "before the app was closed", idempotency_key: key }]);
});

test("after part of a message posted, the rest goes again on its own only if it does not start a reply", async () => {
  const page = await keyedChannel();
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 1, unsent: "the second part", detail: SLOW_SEND, retryable: true,
    resumable: true,
  });
  await sendChannel(page, "the first part. the second part");
  assert.match(outgoingRows(page)[0].text(), retryingFirst("1 part confirmed\\. Sending… .*"));
  page.replyResponse = sentOk;
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.deepEqual(posted(page)[1], { text: "the second part", idempotency_key: posted(page)[0].idempotency_key },
    "the retry repeated the posted part or changed its key");

  // A reply's remainder goes again WITH the reply target, so its first part would be a different
  // request from the one that failed: that is left to the reader. The page decides this itself;
  // the fixture claims otherwise to show it does not lean on the server's `resumable`, which the
  // server never sets for a reply's remainder.
  const replied = await keyedChannel();
  replied.messages = [message({ id: "1944", content: "a question" })];
  await reReadChannel(replied);
  await openReplyOn(replied, replied.messages);
  replied.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 1, unsent: "the rest of the answer", detail: SLOW_SEND, retryable: true,
    resumable: true,
  });
  await replied.el("reply-text").setValue("an answer. the rest of the answer");
  await replied.el("reply-send").click();
  await replied.settle();
  const row = outgoingRows(replied)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed");
  assert.match(row.text(), /Check history before retrying/);
  replied.expireTimers(RETRY_FIRST_MS);
  await settleSend(replied);
  assert.equal(replied.repliesPosted.length, 1, "a reply's remainder was retried on its own");
});

test("the rest of a post cut in parts goes into the thread the server names, without the reply target", async () => {
  // `#263 post-split-per-provider`. Where a provider threads a long message under its first part,
  // the 207 names that thread, and the unsent text goes there: the parts after the first never
  // carried the reply target, so the remainder goes without it, as the part that failed did.
  const page = await keyedChannel();
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 1, unsent: "the second part", detail: SLOW_SEND, retryable: true,
    resumable: true, thread_id: "t_first",
  });
  await sendChannel(page, "the first part. the second part");
  assert.match(outgoingRows(page)[0].text(), retryingFirst("1 part confirmed\\. Sending… .*"));
  assert.equal(savedOutgoing(page)[0].continueIn, "t_first", "a reload would send the rest elsewhere");
  page.replyResponse = sentOk;
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.deepEqual(posted(page)[1],
    { text: "the second part", thread_id: "t_first", idempotency_key: posted(page)[0].idempotency_key },
    "the rest did not go to the first part's thread under the same key");
  assert.equal(outgoingRows(page).length, 0);

  // A reply's remainder continuing in that thread is not a second answer, so it goes on its own.
  const replied = await keyedChannel();
  replied.messages = [message({ id: "1945", content: "a question" })];
  await reReadChannel(replied);
  await openReplyOn(replied, replied.messages);
  replied.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 1, unsent: "the rest of the answer", detail: SLOW_SEND, retryable: true,
    resumable: true, thread_id: "t_answer",
  });
  await replied.el("reply-text").setValue("an answer. the rest of the answer");
  await replied.el("reply-send").click();
  await replied.settle();
  assert.equal(posted(replied)[0].reply_to, "1945");
  assert.match(outgoingRows(replied)[0].text(), retryingFirst(), "a continued reply was left to the reader");
  replied.replyResponse = sentOk;
  replied.expireTimers(RETRY_FIRST_MS);
  await settleSend(replied);
  assert.equal(replied.repliesPosted.length, 2, "the rest of the answer did not go again");
  assert.deepEqual(posted(replied)[1], {
    text: "the rest of the answer", thread_id: "t_answer", idempotency_key: posted(replied)[0].idempotency_key,
  }, "the rest went with the reply target, or outside the thread");

  // Nothing posted yet means nothing to continue: the whole text goes where it was going.
  const unposted = await keyedChannel();
  unposted.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 0, unsent: "not yet anywhere", detail: SLOW_SEND, retryable: true,
    resumable: true, thread_id: "t_elsewhere",
  });
  await sendChannel(unposted, "not yet anywhere");
  assert.equal(savedOutgoing(unposted)[0].continueIn, null, "nothing posted, yet the destination moved");
});

test("a long post continued in its first part's thread leaves a receipt that names the thread", async () => {
  // `#263 post-split-per-provider`. The first part is drawn here; the rest is drawn in its thread,
  // not here, so its receipt carries the Thread tag that opens it, as a reply placed in a thread does.
  const page = newPage();
  page.threadingSupported = true;
  page.threads = [];
  await signIn(page);
  await showDiscord(page, []);
  await pickThread(page, "main");
  const thread = (isRoot) => ({
    id: "t_first", root_message_id: isRoot ? "9194000000000000001" : null, is_root: isRoot,
    reply_count: null, reply_count_exact: false,
  });
  const first = message({ id: "9194000000000000001", content: "the first part. ", thread: thread(true) });
  const rest = message({ id: "9194000000000000002", content: "the second part", thread: thread(false) });
  page.replyResponse = async () => json(200, { posted: first, parts: [first, rest] });
  await sendChannel(page, "the first part. the second part");
  assert.ok(shownIds(page).includes("9194000000000000001"), "the first part is not drawn in the channel");
  assert.ok(!shownIds(page).includes("9194000000000000002"), "a part in the thread was drawn in the channel");
  const receipt = outgoingRows(page)[0];
  assert.ok(receipt, "nothing says the rest was sent");
  assert.match(receipt.text(), /the second part/);
  assert.match(receipt.text(), /Sent\./);
  assert.equal(threadBadge(receipt)?.textContent, "Thread", "the receipt does not say which thread the rest is in");

  // All draws every part where it is, so there is nothing left for a receipt to stand in for.
  const all = newPage();
  all.threadingSupported = true;
  all.threads = [];
  await signIn(all);
  await showDiscord(all, []);
  assert.equal(all.el("thread-select").value, "flat", "All is not what this test assumes");
  all.replyResponse = async () => json(200, { posted: first, parts: [first, rest] });
  await sendChannel(all, "the first part. the second part");
  assert.deepStrictEqual(shownIds(all).slice(-2), ["9194000000000000001", "9194000000000000002"]);
  assert.equal(outgoingRows(all).length, 0, "a receipt stayed for parts drawn in All");
});

test("a remainder the server cannot resend as the same request waits for the reader and promises nothing", async () => {
  // The server says `resumable: false` where sending the unsent text again would not repeat the
  // part that failed under its own words and key — a code block cut across it, say. The provider
  // could not recognise that retry, so neither an automatic one nor the row's promise is safe.
  const page = await keyedChannel();
  page.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 1, unsent: "the rest of the block", detail: SLOW_SEND,
    retryable: true, resumable: false,
  });
  await sendChannel(page, "a long block. the rest of the block");
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "unconfirmed", "a retry was armed");
  assert.match(row.text(), /1 part confirmed\. Delivery unconfirmed\. Check history before retrying/);
  assert.doesNotMatch(row.text(), /cannot post it twice|Retrying in/);
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 1, "a remainder that is not the same request went on its own");
  // Nor does a later failure win it back: the part whose arrival nobody knows stays unknown.
  page.replyResponse = errorResponse(502, "chat_error", SLOW_SEND);
  await outgoingRetry(row).click();
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2);
  assert.match(outgoingRows(page)[0].text(), /Check history before retrying/);
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2, "a later 502 re-armed the automatic retry");
  assert.equal(savedOutgoing(page)[0].keyedOnly, false, "a reload would forget it");

  // A 207 that does not say, from a server older than the field, proves nothing either.
  const older = await keyedChannel();
  older.replyResponse = async () => json(207, {
    error: "partially_posted", posted: 0, unsent: "older server", detail: SLOW_SEND, retryable: true,
  });
  await sendChannel(older, "older server");
  assert.match(outgoingRows(older)[0].text(), /Check history before retrying/);
  older.expireTimers(RETRY_FIRST_MS);
  await settleSend(older);
  assert.equal(older.repliesPosted.length, 1);
});

test("a gateway's own error page in front of the server is retried where the key makes it safe", async () => {
  // Not this server's JSON, so it names no chat service; but nothing answered, like any 5xx.
  const page = await keyedChannel();
  page.replyResponse = async () => ({ ok: false, status: 502, text: async () => "<html>Bad Gateway</html>" });
  await sendChannel(page, "through a proxy");
  const row = outgoingRows(page)[0];
  assert.equal(row.getAttribute("data-send-state"), "retrying", "a gateway 502 was left to the reader");
  assert.match(row.text(), retryingFirst("Sending… vibe-talk did not answer \\(HTTP 502\\)\\. "));
  page.replyResponse = sentOk;
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 2);
  assert.deepEqual(posted(page)[1], posted(page)[0]);
  assert.equal(outgoingRows(page).length, 0);
});

test("a waiting row's cause reads as a sentence before the countdown", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error",
    'Example Chat returned HTTP 502: {"message":"upstream command exited 1"}');
  await sendChannel(page, "one more try");
  assert.match(outgoingRows(page)[0].text(),
    retryingFirst("Sending… Example Chat could not answer: upstream command exited 1\\. "));
});

test("signing out cancels every automatic retry", async () => {
  const page = await keyedChannel();
  page.replyResponse = errorResponse(502, "chat_error", SLOW_SEND);
  await sendChannel(page, "not for the next person");
  await page.el("forget-token").click();
  await page.settle();
  assert.equal(page.screen(), "signin");
  page.expireTimers(RETRY_FIRST_MS);
  await settleSend(page);
  assert.equal(page.repliesPosted.length, 1, "a send went out after signing out");
});

const VISIBLE_REFRESH_MS = sourceConstant("VISIBLE_REFRESH_MS");

test("a poll after a newest read asks only for what changed since, and draws it", async () => {
  const { page, server } = await forwardPage();
  assert.deepStrictEqual(forwardReadKinds(server), [false], "the first read of a view is not a full one");
  const seen = server.revision;
  server.post(message({ id: "300", content: "posted after the first read" }));
  await pollOnce(page);
  assert.equal(server.reads.at(-1).after, `main@${seen}`, "the poll did not continue from the read before it");
  assert.equal(server.reads.at(-1).before, null);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "300"]);
  assert.match(freshness(page).textContent, /^(Live · u|U)pdated \d{2}:\d{2}$/);
  assert.equal(freshness(page).getAttribute("data-state"), "fresh");
  await pollOnce(page);
  assert.equal(server.reads.at(-1).after, `main@${server.revision}`, "the delta's own cursor was not kept");
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "300"], "an empty delta changed the list");
});

test("a delta replaces what changed, removes what was deleted, and keeps every row it does not mention", async () => {
  const { page, server } = await forwardPage({ olderCursor: "older:held" }, (p) => p.dealtWith.add("200"));
  assert.ok(savedScopes(page)[scopeKey(CHANNEL.id)].dismissed.includes("200"));
  assert.equal(page.el("load-older").hidden, false, "the fixture's older history was not offered");
  const root = page.messages.find((m) => m.id === "201");
  server.edit("201", { content: "first thread root, edited", thread: { ...root.thread, reply_count: 7 } });
  server.remove("200");
  await pollOnce(page);
  assert.equal(server.reads.at(-1).after !== null, true);
  assert.deepStrictEqual(shownIds(page), ["201", "203"],
    "a deleted row stayed, or a row the delta did not mention was taken as deleted");
  assert.ok(rowShowing(page, "first thread root, edited"), "the edit did not replace the held row");
  assert.equal(threadButton(rowShowing(page, "first thread root, edited")).textContent, "7 replies",
    "the delta's thread record did not replace the held one");
  const saved = savedScopes(page)[scopeKey(CHANNEL.id)];
  assert.ok(!saved.messages.some((m) => m.id === "200"), "the deleted message was saved for the next start");
  assert.ok(!saved.dismissed.includes("200"), "the deleted message kept its archive mark");
  assert.ok(!("newest" in saved.views.main), "a forward cursor was saved to the device");
  // A delta says nothing about older history: the walk back continues from where it was.
  assert.equal(page.el("load-older").hidden, false, "a delta took the walk back away");
  await page.el("load-older").click();
  await page.settle();
  assert.equal(server.reads.at(-1).before, "older:held", "a delta replaced the walk-back cursor");
});

test("a thread list delta moves the threads that changed and takes away the ones that left", async () => {
  const { page, server } = await forwardPage();
  await page.el("channel-view-threads").click();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server), [false, false], "the thread list was not read in full first");
  const [first, second] = page.threads;
  assert.match(page.el("thread-list").text(), /Second discussion/);
  server.dropThread(second.id);
  page.threads[0] = { ...first, title: "First discussion, renamed", updated_at: "2026-08-19T06:00:00.000Z" };
  server.threadChanged.set(first.id, (server.revision += 1));
  await pollOnce(page);
  const read = server.reads.at(-1);
  assert.equal(read.view, "threads");
  assert.equal(read.after !== null, true, "the thread list was read in full again");
  assert.doesNotMatch(page.el("thread-list").text(), /Second discussion/, "a thread the delta removed is still listed");
  assert.match(page.el("thread-list").text(), /First discussion, renamed/, "the moved thread was not replaced");
});

for (const [status, error] of [[410, "cursor_expired"], [400, "cursor_mismatch"]]) {
  test(`a forward read answered ${status} ${error} becomes one full read in the same refresh, never a failure`, async () => {
    const { page, server } = await forwardPage();
    server.post(message({ id: "301", content: "posted while the cursor went stale" }));
    server.refuse = { status, error };
    const from = server.reads.length;
    await pollOnce(page);
    assert.deepStrictEqual(forwardReadKinds(server, from), [true, false], "the refusal was not followed by a full read");
    assert.equal(freshness(page).getAttribute("data-state"), "fresh", "a refused cursor was reported as a failure");
    assert.doesNotMatch(freshness(page).textContent, /failed/i);
    assert.equal(page.el("error").hidden, true, "a refused cursor raised the error banner");
    assert.ok(shownIds(page).includes("301"));
    await pollOnce(page);
    assert.equal(server.reads.at(-1).after, `main@${server.revision}`, "the full read's cursor was not taken up");
  });
}

test("a delta still saying more after four pages is replaced by a full read that claims no continuity", async () => {
  const { page, server } = await forwardPage();
  server.deltaLimit = 1;
  server.newestLimit = 2;
  for (const id of ["310", "311", "312", "313", "314", "315"]) {
    server.post(message({ id, content: `burst ${id}` }));
  }
  const from = server.reads.length;
  await pollOnce(page);
  assert.deepStrictEqual(forwardReadKinds(server, from), [true, true, true, true, false]);
  assert.deepStrictEqual(shownIds(page), ["314", "315"],
    "the full read claimed coverage across rows it never read");
  assert.equal(page.el("load-older").hidden, false, "the history it did not read is not offered");
});

test("a delta overtaken by a newer read of its view is dropped, not folded over it", async () => {
  const { page, server } = await forwardPage();
  await page.el("thread-select").dispatch("pointerdown");
  await page.settle();
  const stale = {
    id: "from-a-stale-delta", root: null, title: "a card only the stale delta had", reply_count: 1,
    reply_count_exact: true, updated_at: "2026-08-19T06:30:00.000Z",
  };
  const held = gate(async (path) => (new URL(path, "http://fixture.test").searchParams.get("after")
    ? json(200, timelineAnswer({ view: "threads", has_threads: true, threads: [stale], next_after: "threads@999",
      delta: { more: false, complete: true, deleted: [], removed_threads: [] } }))
    : server.answer(path)));
  server.hold = held;
  page.setClock(page.clock() + 120000);
  await page.el("thread-select").dispatch("pointerdown");
  await page.settle();
  // While that delta is on the wire, something changes and the thread list's Older threads, with no
  // cursor to step back from yet, reads the newest page of the same list, which issues a cursor of its
  // own. (`#281 view-keys`: it was the Threads screen's first read.)
  server.hold = null;
  server.post(message({ id: "390", content: "moves the change record on" }));
  await page.el("side-more-threads").click();
  await page.settle();
  const replaced = server.revision;
  held.open();
  await page.settle();
  await page.settle();
  assert.ok(!threadOptions(page).some(([value]) => value === `thread:${stale.id}`),
    "a delta older than the page it raced was folded over it");
  page.setClock(page.clock() + 120000);
  await page.el("thread-select").dispatch("pointerdown");
  await page.settle();
  assert.equal(server.reads.at(-1).after, `threads@${replaced}`, "the dropped delta's cursor was taken up");
});

test("a live message reads a delta; an edit or deletion does only where deltas carry them", async () => {
  for (const complete of [true, false]) {
    const { page, server } = await forwardPage({ complete });
    const stream = page.stream();
    const arriving = server.post(message({ id: "320", content: "arrived live" }));
    await deliver(page, stream, sseMessage(arriving));
    await page.settle();
    assert.equal(server.reads.at(-1).after !== null, true, `complete=${complete}: a live arrival read in full`);
    server.edit("320", { content: "arrived live, edited" });
    await deliver(page, stream, sseUpdate("e1", { ...arriving, content: "arrived live, edited" }));
    await page.settle();
    assert.equal(server.reads.at(-1).after !== null, complete,
      `complete=${complete}: an edit was read with a delta that cannot carry it, or in full when one can`);
    assert.ok(rowShowing(page, "arrived live, edited"), `complete=${complete}: the edit never showed`);
    server.remove("320");
    await deliver(page, stream, sseDelete("d1", CHANNEL.id, "320"));
    await page.settle();
    assert.equal(server.reads.at(-1).after !== null, complete);
    assert.ok(!shownIds(page).includes("320"), `complete=${complete}: the deletion never showed`);
  }
});

test("an edit replayed from the tail is answered by a read that carries edits, and by one read", async () => {
  for (const complete of [true, false]) {
    const { page, server } = await forwardPage({ complete });
    // Learn what this channel's deltas carry, then reattach the stream so its tail is replayed.
    const arriving = server.post(message({ id: "330", content: "before the reconnect" }));
    await deliver(page, page.stream(), sseMessage(arriving));
    page.stream().drop();
    await page.settle();
    page.expireTimers(sourceConstant("LIVE_RETRY_MS"));
    await page.settle();
    server.edit("330", { content: "edited while the stream was away" });
    const from = server.reads.length;
    await deliver(page, page.stream(),
      sseUpdate("u9", page.messages.find((m) => m.id === "330"), { replayed: true, from_tail: true }));
    await page.settle();
    await page.settle();
    assert.deepStrictEqual(forwardReadKinds(server, from), [complete],
      `complete=${complete}: the tail edit was not answered by exactly one read that carries edits`);
    assert.ok(rowShowing(page, "edited while the stream was away"));
  }
});

test("a delta of additions does not answer an edit replayed from the tail; the full read after it does", async () => {
  const { page, server } = await forwardPage({ complete: false });
  const first = server.post(message({ id: "395", content: "learns what the deltas carry" }));
  await deliver(page, page.stream(), sseMessage(first));
  page.stream().drop();
  await page.settle();
  page.expireTimers(sourceConstant("LIVE_RETRY_MS"));
  await page.settle();
  // A read begun after the reattach is on the wire when the tail's edit is replayed.
  const held = gate(server.answer);
  server.hold = held;
  const live = server.post(message({ id: "396", content: "arrived after the reconnect" }));
  await deliver(page, page.stream(), sseMessage(live));
  server.edit("395", { content: "edited while the stream was away" });
  await deliver(page, page.stream(),
    sseUpdate("u5", page.messages.find((m) => m.id === "395"), { replayed: true, from_tail: true }));
  server.hold = null;
  const from = server.reads.length;
  held.open();
  await page.settle();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, from), [true, false],
    "a delta that cannot carry edits was taken as having seen the replayed one");
  assert.ok(rowShowing(page, "edited while the stream was away"), "the replayed edit never showed");
});

test("coming back after fifteen seconds away reads what changed; a glance away reads nothing", async () => {
  const { page, server } = await forwardPage();
  const reads = server.reads.length;
  await page.setVisibility("hidden");
  page.setClock(page.clock() + 10000);
  await page.setVisibility("visible");
  await page.settle();
  assert.equal(server.reads.length, reads, "a ten-second glance at another app read the channel");
  server.post(message({ id: "340", content: "posted while the phone was in a pocket" }));
  await page.setVisibility("hidden");
  page.setClock(page.clock() + VISIBLE_REFRESH_MS);
  await page.setVisibility("visible");
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true], "coming back did not read one delta");
  assert.ok(shownIds(page).includes("340"));
});

// --- `#226 page-energy-profile`: what the page costs while nothing is happening -----------------
//
// The owner keeps the page open as an installed app, on a desk and on a phone, and asked that it not
// drain either. Measured with CPU time as the proxy: every poll rebuilt every row when nothing had
// changed, a hidden page polled and re-read every live message as if it were on screen and kept its
// stream's keep-alive waking the phone's radio, the reply arrows asked for a layout per arrow, the
// lookup of a row's parts walked every word of every message, and every row made its own clock
// formatter. These pin each of those away, by what the page does rather than by how long it takes.

const LIVE_HIDDEN_CLOSE_MS = sourceConstant("LIVE_HIDDEN_CLOSE_MS");
const LOADING_LINE_DELAY_MS = sourceConstant("LOADING_LINE_DELAY_MS");

test("A REFRESH THAT BRINGS NOTHING BUILDS NOTHING: the same rows stay, and what did change still redraws", async () => {
  const { page, server } = await forwardPage();
  const rows = [...page.el("discord-log").children];
  const made = page.createdTags.length;
  const reads = server.reads.length;
  await pollOnce(page);
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true], "the poll did not read what changed");
  assert.equal(builtSince(page, made, "li"), 0, "a poll that brought nothing built rows");
  assert.equal(builtSince(page, made, "button"), 0, "a poll that brought nothing built controls");
  assert.ok(rows.length > 0 && page.el("discord-log").children.length === rows.length &&
    [...page.el("discord-log").children].every((li, at) => li === rows[at]), "a poll that brought nothing replaced the rows");
  // What a delta does carry is drawn: a message, a Done from another device, a deletion.
  server.post(message({ id: "380", content: "posted on another device" }));
  await pollOnce(page);
  assert.ok(shownIds(page).includes("380"), "a new message was not drawn");
  page.dealtWith.add("200");
  server.edit("200", {});
  await pollOnce(page);
  assert.equal(rowWithId(page, "200").getAttribute("data-archived"), "true", "a Done from another device was not drawn");
  server.remove("380");
  await pollOnce(page);
  assert.ok(!shownIds(page).includes("380"), "a deletion was not drawn");
});

test("A LIVE MESSAGE IS BUILT ONCE: the read behind it, bringing the same message, rebuilds nothing", async () => {
  const { page, server } = await forwardPage();
  const arriving = server.post(message({ id: "390", content: "the runner came back" }));
  const reads = server.reads.length;
  const held = gate(server.answer);
  server.hold = held;
  await deliver(page, page.stream(), sseMessage(arriving));
  assert.ok(rowWithId(page, "390"), "the live message was not drawn at once");
  const rows = [...page.el("discord-log").children];
  const made = page.createdTags.length;
  server.hold = null;
  held.open();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true], "the live message was not read behind");
  assert.equal(builtSince(page, made, "li"), 0, "the read behind a live message built the rows again");
  assert.ok([...page.el("discord-log").children].every((li, at) => li === rows[at]),
    "the read behind a live message replaced the rows");
});

test("...BUT A ROW THE STREAM CHANGED UNDER is drawn again by the read behind it, though that read changes nothing held", async () => {
  // The stream's copy of a message already on screen goes into the store and leaves its row as it
  // was; the read behind it brings the same copy. Nothing in the store changes, but the row on
  // screen was drawn from an older one.
  const { page, server } = await forwardPage();
  server.edit("200", { content: "main channel announcement, corrected" });
  await deliver(page, page.stream(), sseMessage(page.messages.find((m) => m.id === "200")));
  await page.settle();
  await page.settle();
  assert.ok(rowShowing(page, "main channel announcement, corrected"), "the row kept the copy it was drawn from");
});

test("...NOR ONE THAT MARKS A REPLY DONE ON ANOTHER DEVICE: Main's count of it goes", async () => {
  const { page, server } = await forwardPage();
  await pickThread(page, "flat");
  await pickThread(page, "main");
  await page.settle();
  assert.equal(unreadChip(page, "201") && unreadChip(page, "201").textContent, "1 unread");
  // Main's deltas say what is Done among every message from a position on, the replies among them.
  const answer = server.answer;
  server.answer = async (path) => {
    const response = await answer(path);
    const url = new URL(path, "http://fixture.test");
    if (!url.searchParams.get("after") || url.searchParams.get("view") !== "main") return response;
    return json(response.status, { ...JSON.parse(await response.text()),
      reply_dismissals: replyDismissals(page.messages, page.dealtWith) });
  };
  await pollOnce(page);
  assert.equal(unreadChip(page, "201") && unreadChip(page, "201").textContent, "1 unread", "nothing was marked yet");
  page.dealtWith.add("202");
  await pollOnce(page);
  assert.equal(unreadChip(page, "201"), null, "a reply marked Done on another device still counted as unread on Main");
});

test("A REFRESH NOBODY ASKED FOR, over rows on screen, shows its loading line only once it is slow", async () => {
  const { page, server } = await forwardPage();
  const line = page.el("channel-loading");
  const held = gate(server.answer);
  server.hold = held;
  assert.equal(page.expireTimers(DISCORD_POLL_MS), 1);
  await page.settle();
  assert.equal(line.hidden, true, "a poll over rows on screen put its loading line up at once");
  assert.match(page.el("channel-freshness").textContent, /refreshing…$/, "the pill did not say the poll was out");
  assert.equal(page.expireTimers(LOADING_LINE_DELAY_MS), 1, "nothing waited to show the line of a slow poll");
  assert.equal(line.hidden, false, "a slow poll never said it was loading");
  server.hold = null;
  held.open();
  await page.settle();
  await page.settle();
  assert.equal(line.hidden, true, "the line outlived its poll");
  // A quick one never shows it, and leaves nothing waiting to.
  await pollOnce(page);
  assert.equal(line.hidden, true);
  assert.equal(page.expireTimers(LOADING_LINE_DELAY_MS), 0, "a finished poll left its line waiting to show");
});

test("THE SCROLLER IS SIZED BY ITS CELL ALONE, so a change beside it does not lay its history out again", () => {
  // `#226 page-energy-profile`. The freshness pill shares #scroll-area's grid cell, and its words
  // change at every refresh: without containment that laid the whole history out, twice.
  assert.match(cssBlock("#scroll-area"), /contain:\s*size layout/, "the scroller is sized by its content again");
  // Layout containment makes the scroller what everything positioned inside it is placed against, so
  // nothing in the list may hang off anything outside it: what is positioned there hangs off its row.
  assert.match(cssBlock("#discord-log li.discord-message"), /position:\s*relative/);
  assert.doesNotMatch(CSS_CODE, /position:\s*(fixed|sticky)/, "something now escapes the scroller's containment");
});

test("A HIDDEN PAGE POLLS NOTHING: the poll waits for it to come back, and one that came due meanwhile runs then", async () => {
  const { page, server } = await forwardPage();
  const reads = server.reads.length;
  await page.setVisibility("hidden");
  assert.equal(page.expireTimers(DISCORD_POLL_MS), 0, "a hidden page kept its poll armed");
  page.setClock(page.clock() + 10 * 60 * 1000);
  await page.settle();
  assert.equal(server.reads.length, reads, "a hidden page read the channel");
  // Back after ten minutes: one read of what changed, and the poll armed again.
  await page.setVisibility("visible");
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true], "coming back did not read what changed, once");
  await pollOnce(page);
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true, true], "the poll was not armed again");
  // A glance away that the poll came due in: it runs on the way back.
  page.setClock(page.clock() + DISCORD_POLL_MS - 5000);
  await page.setVisibility("hidden");
  page.setClock(page.clock() + 10000);
  await page.setVisibility("visible");
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true, true, true], "a poll that came due while hidden never ran");
  // A glance away that it did not: nothing is read, and the poll goes on with the rest of its wait,
  // so glancing away again and again cannot put it off for good.
  await page.setVisibility("hidden");
  page.setClock(page.clock() + 5000);
  await page.setVisibility("visible");
  await page.settle();
  assert.equal(server.reads.length, reads + 3, "a five-second glance read the channel");
  assert.equal(page.expireTimers(DISCORD_POLL_MS), 0, "a glance away started the poll's wait again");
  // A poll on the wire when the page is hidden, landing after: it arms no next one until the return.
  const held = gate(server.answer);
  server.hold = held;
  assert.equal(page.expireTimers(DISCORD_POLL_MS - 5000), 1, "the poll was not armed for the rest of its wait");
  await page.settle();
  await page.setVisibility("hidden");
  server.hold = null;
  held.open();
  await page.settle();
  await page.settle();
  assert.equal(server.reads.length, reads + 4, "the poll on the wire did not land");
  assert.equal(page.expireTimers(DISCORD_POLL_MS), 0, "a poll landing on a hidden page armed the next");
  await page.setVisibility("visible");
  await page.settle();
  assert.equal(page.expireTimers(DISCORD_POLL_MS), 1, "the poll did not come back with the page");
});

test("...and a channel read whole polls at once on the way back when its poll came due while hidden", async () => {
  // No deltas here, so no read for coming back: the poll that came due is that read.
  const page = newPage();
  await signIn(page);
  await showDiscord(page, [message({ id: "100", content: "the only message" })]);
  const reads = page.pageReads;
  page.setClock(page.clock() + DISCORD_POLL_MS - 5000);
  await page.setVisibility("hidden");
  assert.equal(page.expireTimers(DISCORD_POLL_MS), 0, "a hidden page kept its poll armed");
  page.setClock(page.clock() + 10000);
  await page.setVisibility("visible");
  await page.settle();
  assert.equal(page.pageReads, reads + 1, "a poll that came due while hidden did not run on the way back");
});

test("A LIVE MESSAGE ON A HIDDEN PAGE is kept, not drawn or read; coming back draws it at once and reads once", async () => {
  const { page, server } = await forwardPage();
  const reads = server.reads.length;
  await page.setVisibility("hidden");
  const arriving = server.post(message({ id: "361", content: "posted while the phone was in a pocket" }));
  await deliver(page, page.stream(), sseMessage(arriving));
  await page.settle();
  assert.equal(server.reads.length, reads, "a hidden page re-read the channel for a live message");
  assert.ok(!shownIds(page).includes("361"), "a hidden page drew a row for a live message");
  // Back within a glance: drawn before any read answers, then read once behind it.
  const held = gate(server.answer);
  server.hold = held;
  page.setClock(page.clock() + 5000);
  await page.setVisibility("visible");
  await page.settle();
  assert.ok(shownIds(page).includes("361"), "coming back did not draw what arrived while hidden");
  server.hold = null;
  held.open();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true], "coming back did not read what changed, once");
  assert.ok(shownIds(page).includes("361"));
  // An edit waits for that read the same way.
  const before = server.reads.length;
  await page.setVisibility("hidden");
  server.edit("361", { content: "edited in the pocket" });
  await deliver(page, page.stream(), sseUpdate("u361", page.messages.find((m) => m.id === "361")));
  await page.settle();
  assert.equal(server.reads.length, before, "a hidden page re-read the channel for an edit");
  page.setClock(page.clock() + 5000);
  await page.setVisibility("visible");
  await page.settle();
  await page.settle();
  await page.settle();
  assert.equal(server.reads.length, before + 1, "coming back did not read the edit, once");
  assert.ok(rowShowing(page, "edited in the pocket"), "the edit made while hidden never showed");
});

test("A HIDDEN PAGE LETS ITS STREAM GO after a while with no call, and takes it up again from the last event it saw", async () => {
  const { page, server } = await forwardPage();
  const first = server.post(message({ id: "370", content: "seen before the pocket" }));
  await deliver(page, page.stream(), sseMessage(first));
  await page.settle();
  const stream = page.stream();
  const opens = page.streamOpens.length;
  await page.setVisibility("hidden");
  assert.equal(page.expireTimers(LIVE_HIDDEN_CLOSE_MS), 1, "a hidden page kept its stream with nothing listening");
  // The reader lets go at its next chunk, the keep-alive being the one that comes.
  stream.push(": keep-alive\n\n");
  await page.settle();
  assert.equal(stream.cancels, 1, "the stream was not closed");
  const missed = server.post(message({ id: "371", content: "posted while the stream was closed" }));
  const reads = server.reads.length;
  page.setClock(page.clock() + LIVE_HIDDEN_CLOSE_MS);
  await page.setVisibility("visible");
  await page.settle();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(forwardReadKinds(server, reads), [true], "coming back did not read what changed, once");
  assert.ok(shownIds(page).includes(missed.id), "what was posted while the stream was closed never showed");
  assert.equal(page.streamOpens.length, opens + 1, "the stream did not come back");
  assert.equal(page.streamOpens[opens].lastEventId, "370", "the stream came back from its tail, not from the last event it saw");
});

test("...BUT A CALL KEEPS ITS STREAM, and hears what arrives while the page is hidden", async () => {
  const page = newPage();
  await startTalking(page);
  page.messages = [];
  await page.el("view-switch").click();
  await page.settle();
  const stream = page.stream();
  await setReadNew(page, "gist");
  await page.setVisibility("hidden");
  page.expireTimers(LIVE_HIDDEN_CLOSE_MS);
  await deliver(page, stream, sseMessage(message({ id: "502", content: "and the tag is cut" })));
  await speakRelay(page);
  assert.equal(page.stream(), stream, "a call's stream was replaced while the page was hidden");
  assert.equal(stream.cancels, 0, "a call's stream was closed while the page was hidden");
  assert.equal(relayed(page).length, 1, "a call did not hear what arrived while the page was hidden");
});

/** A forward-capable Main with two answers drawn directly under what they answer, so both arrows meet. */
const twoAdjacentReplies = (p) => {
  p.messages = [...p.messages,
    message({ id: "205", content: "is the runner wedged?" }),
    message({ id: "206", content: "restarted it", reply_to: "205", ...ALICE }),
    message({ id: "207", content: "and the cache?" }),
    message({ id: "208", content: "cleared too", reply_to: "207", ...ALICE })];
  p.enableResizeObserver();
};

test("THE ARROWS ARE MEASURED ONCE A REDRAW, every rectangle before any write; never hidden, never with none to measure", async () => {
  const { page, server } = await forwardPage({}, twoAdjacentReplies);
  const list = page.el("discord-log");
  list.clientWidth = 380;
  assert.equal(arrowStyle(rowWithId(page, "206")), "adjacent");
  // Every rectangle read and every arrow written, in order: "list" for the list's own box.
  const log = [];
  const read = FakeElement.prototype.getBoundingClientRect;
  const write = FakeElement.prototype.setAttribute;
  FakeElement.prototype.getBoundingClientRect = function rect() {
    log.push(this === list ? "list" : "read");
    return read.call(this);
  };
  FakeElement.prototype.setAttribute = function set(name, value) {
    if (name === "data-reply-reach") log.push("write");
    return write.call(this, name, value);
  };
  try {
    server.post(message({ id: "209", content: "one more thing" }));
    await pollOnce(page);
    assert.ok(shownIds(page).includes("209"));
    assert.equal(log.filter((entry) => entry === "list").length, 1, "a redraw measured the arrows more than once");
    const measuring = log.slice(log.indexOf("list"));
    assert.deepStrictEqual(measuring.filter((entry) => entry === "write"), ["write", "write"], "the two arrows were not measured");
    assert.ok(measuring.lastIndexOf("read") < measuring.indexOf("write"), "a rectangle was read after an arrow was written");
    // Measured again with nothing changed, nothing is written.
    log.length = 0;
    page.resized(list);
    page.expireTimers(0);
    assert.deepStrictEqual(log.filter((entry) => entry !== "read"), ["list"], "an unchanged list had its arrows written again");
    // Hidden: nothing is laid out for nobody. Shown again: measured, once.
    await page.setVisibility("hidden");
    log.length = 0;
    page.resized(list);
    page.expireTimers(0);
    assert.deepStrictEqual(log, [], "a hidden page measured the arrows");
    await page.setVisibility("visible");
    await page.settle();
    assert.equal(log.filter((entry) => entry === "list").length, 1, "the arrows a hidden page left were not measured on its return");
  } finally {
    FakeElement.prototype.getBoundingClientRect = read;
    FakeElement.prototype.setAttribute = write;
  }
  // A list with no arrow meeting the row above asks for no layout at all.
  const plain = await forwardPage();
  const quiet = plain.page.el("discord-log");
  quiet.clientWidth = 380;
  let asked = 0;
  quiet.getBoundingClientRect = () => {
    asked += 1;
    return read.call(quiet);
  };
  plain.server.post(message({ id: "210", content: "nothing answers anything" }));
  await pollOnce(plain.page);
  assert.ok(shownIds(plain.page).includes("210"));
  assert.equal(asked, 0, "a list with no adjacent arrow was laid out to measure them");
});

test("A ROW'S PARTS ARE FOUND WITHOUT WALKING THE MESSAGE'S OWN TEXT", async () => {
  const { page } = await forwardPage({}, twoAdjacentReplies);
  // A row with no arrow, holding in its text something that would pass for one. The sanitizer lets
  // no such class through, so this is the fixture's doing: what it shows is whether the lookup of a
  // row's parts — six to eight of them a row, on every pass — reads every word of the message.
  const plain = rowWithId(page, "205");
  assert.equal(replyArrowOf(plain), undefined, "the question has an arrow of its own");
  const text = plain.descendants().find((node) => node.className === "md");
  const decoy = page.document.createElement("span");
  decoy.className = "reply-jump";
  text.append(decoy);
  // Every pass over the search decides every row's arrow (`renderReplyLinks`): opening every fold is one.
  await page.el("expand-all").click();
  await page.settle();
  assert.equal(decoy.getAttribute("data-reply-style"), null, "looking for a row's arrow walked into the text of its message");
  assert.equal(arrowStyle(rowWithId(page, "206")), "adjacent", "the arrows themselves were not found");
});

test("ONE CLOCK FORMATTER FOR EVERY ROW, made again when the device's zone moves", async () => {
  const { page, server } = await forwardPage();
  const made = page.dateFormatsMade;
  const at = server.post(message({ id: "395", content: "what time is it" })).timestamp;
  await pollOnce(page);
  const stamp = () => rowWithId(page, "395").descendants().find((node) => node.className === "msg-time").textContent;
  assert.ok(page.dateFormatsMade - made <= 1, `a redraw of ${shownIds(page).length} rows made ${page.dateFormatsMade - made} formatters`);
  assert.equal(stamp(), new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
    "the row's time is not what the browser's own short time says");
  const zone = process.env.TZ;
  try {
    process.env.TZ = new Date(at).getTimezoneOffset() === -540 ? "America/New_York" : "Asia/Tokyo";
    server.post(message({ id: "396", content: "and now" }));
    await pollOnce(page);
    assert.equal(stamp(), new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }),
      "a row drawn after the device changed zone kept the old zone's time");
  } finally {
    if (zone === undefined) delete process.env.TZ;
    else process.env.TZ = zone;
  }
});

// --- remote-ops-audit: every read of the channel says why it was made ----------------------------
//
// The server writes a read's `reason=` in its access line and counts the provider reads it makes under
// it (src/read_reason.rs), so a figure like "forty timeline reads a minute" can be taken apart by
// cause. Attribution only: these tests check what the page SAYS; nothing it sends changes because of
// the word, which every other test in this file holds it to.

/** The `reason=` a request path carries, or null. */
const readReasonIn = (path) => new URL(String(path).replace(/^[A-Z]+ /, ""), "http://fixture.test").searchParams.get("reason");

/** Every read of a channel's messages the page sent: timeline, page and to-do reads. */
const channelReadRequests = (page) => page.requests.filter((request) => /^GET \/api\/v1\/channels\/[^/]+\/(timeline|page|todo)\?/.test(request));

/** Every channel read carries one of the server's words. */
function assertEveryReadSaysWhy(page) {
  const reads = channelReadRequests(page);
  assert.ok(reads.length > 0, "no channel read was sent, so this proves nothing");
  const silent = reads.filter((request) => !PAGE_READ_REASONS.includes(readReasonIn(request)));
  assert.deepStrictEqual(silent, [], "a channel read said nothing the server takes about why it was made");
}

test("`remote-ops-audit`: every timeline read says what it was made for, in a word the server takes", async () => {
  const page = await threadPage();
  assert.equal(readReasonIn(page.timelineCalls[0]), "enter", "opening the channel");
  const reads = page.timelineCalls.length;
  await pollOnce(page);
  const polled = page.timelineCalls.slice(reads);
  assert.ok(polled.length > 0, "the poll read nothing");
  assert.deepStrictEqual(polled.map(readReasonIn), polled.map(() => "poll"), polled.join("\n"));
  assertEveryReadSaysWhy(page);
});

test("`remote-ops-audit`: in two columns the right column's read beside Main says side", async () => {
  const { page, server } = await twoColumnServer();
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  const reads = page.timelineCalls.length;
  await pollOnce(page);
  const polled = page.timelineCalls.slice(reads).map((path) => `${viewOf(path)} ${readReasonIn(path)}`);
  assert.ok(polled.includes("thread side"), `the right column's thread was not read as side: ${polled}`);
  assert.ok(polled.some((read) => read.endsWith(" poll") && !read.startsWith("thread")),
    `Main was not read for the poll: ${polled}`);
  assert.ok(server.reads.length > 0);
  assertEveryReadSaysWhy(page);
});

test("`remote-ops-audit`: a channel without threads says why it reads its page and its to-do list", async () => {
  const page = newPage();
  await signIn(page);
  await showDiscord(page, backlog(5));
  const reads = () => channelReadRequests(page).map(readReasonIn);
  assert.equal(reads()[0], "enter", "entering the channel");
  await reReadChannel(page);
  assert.equal(reads().at(-1), "poll", "the poll");
  await turnTodoOn(page);
  assert.ok(page.todoReadPaths.length > 0, "turning Hide read on did not read the to-do list");
  assert.equal(readReasonIn(page.todoReadPaths.at(-1)), "enter", "the to-do list is a view entered");
  assertEveryReadSaysWhy(page);
});

// --- remote-ops-audit W7: no poll sent again while the server still reads one given up on ---------
//
// The page gives up on a read at CHANNEL_READ_TIMEOUT_MS; the server's own read runs on to its
// fifty-second bound, and behind a bridge it is a scan of the whole space. The page used to poll again
// regardless. These tests stand a server in front of it that answers in a minute.

/**
 * Derived, or the server's fifty seconds on a page that has no such rule, so that each test below
 * fails on its own against the page before it rather than stopping this module loading.
 */
const SERVER_READ_BOUND_MS = optionalSourceConstant("SERVER_READ_BOUND_MS") ?? 50000;

/**
 * Hold the forward server's next answers: each is the channel as it stood when the read was ASKED,
 * frozen then, and handed over only when `release` is called — a minute later, in these tests.
 */
function answerInAMinute(server) {
  /** @type {Array<() => void>} */
  const waiting = [];
  server.hold = {
    respond: async (path) => {
      const asked = await server.answer(path);
      const frozen = await asked.text();
      return new Promise((resolve) => {
        waiting.push((failWith) => resolve(failWith
          ? json(failWith, { error: "upstream_timeout", detail: "the chat service did not answer in time" })
          : { ok: asked.ok, status: asked.status, text: async () => frozen }));
      });
    },
  };
  return {
    /** Hand the held answers over; with `failWith`, as that refusal instead, as a server's bound ends a read. */
    release: (failWith = null) => {
      server.hold = null;
      for (const answer of waiting.splice(0)) answer(failWith);
    },
  };
}

/** Poll once with the server answering in a minute, and give up on the read at twenty seconds. */
async function pollAndGiveUp(page) {
  await pollOnce(page);
  page.setClock(page.clock() + CHANNEL_READ_TIMEOUT_MS);
  assert.ok(page.expireTimers(CHANNEL_READ_TIMEOUT_MS) >= 1, "nothing bounded the read");
  await page.settle();
  await page.settle();
}

test("`remote-ops-audit` W7: with a server that answers in a minute, the polls send one read in that minute, and its late answer still lands", async () => {
  const { page, server } = await forwardPage();
  const sent = page.clock();
  const calls = page.timelineCalls.length;
  server.post(message({ id: "450", content: "posted before the poll" }));
  const minute = answerInAMinute(server);
  await pollAndGiveUp(page);
  assert.equal(page.timelineCalls.length - calls, 1, "the poll did not read");
  assert.ok(!shownIds(page).includes("450"), "fixture: the read was answered before it was given up on");
  // The one thing put off inside the server's bound: the poll come round, here twice. (Coming back
  // to the page, a stream reset and the replay tail's edits are not: see the tests below.)
  page.setClock(sent + 30000);
  await pollOnce(page);
  page.setClock(sent + SERVER_READ_BOUND_MS - 1);
  await pollOnce(page);
  assert.equal(page.timelineCalls.length - calls, 1,
    `a view whose read was given up on was read again inside the server's bound: ${page.timelineCalls.slice(calls)}`);
  // At a minute the server answers the read given up on. Nothing has replaced it, so it is drawn.
  page.setClock(sent + 60000);
  minute.release();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 1, "more than one read went out in the minute");
  assert.ok(shownIds(page).includes("450"), "the late answer of the read given up on was thrown away");
  // Past the bound, the poll reads again.
  await pollOnce(page);
  assert.equal(page.timelineCalls.length - calls, 2, "the view was never read again");
});

test("`remote-ops-audit` W7: inside that wait a pull to refresh reads at once, and the late answer does not take back what it brought", async () => {
  const { page, server } = await forwardPage();
  // A poll this long after the last full read reads in full, so its late answer carries every row.
  page.setClock(page.clock() + TIMELINE_RECONCILE_MS);
  const sent = page.clock();
  const calls = page.timelineCalls.length;
  const minute = answerInAMinute(server);
  await pollAndGiveUp(page);
  assert.equal(page.timelineCalls.length - calls, 1, "the poll did not read");
  assert.equal(new URL(page.timelineCalls.at(-1), "http://fixture.test").searchParams.get("after"), null,
    "fixture: the poll's read was not a full one");
  // Edited upstream after the poll's read was asked, and the server answers at once again.
  server.hold = null;
  server.edit("200", { content: "main channel announcement, corrected" });
  page.setClock(sent + 30000);
  page.el("scroll-area").scrollTop = 0;
  await pullDown(page, PULL_ARM_PX);
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 2, "a pull to refresh inside the wait did not read");
  assert.ok(rowShowing(page, "main channel announcement, corrected"), "the pull's read was not drawn");
  // The poll's read lands now, with the words from before the edit: the pull's answer is newer.
  minute.release();
  await page.settle();
  await page.settle();
  assert.ok(rowShowing(page, "main channel announcement, corrected"),
    "the late answer of a read given up on put back the words a newer read had replaced");
});

test("`remote-ops-audit` W7: a live message for the channel ends the wait, and the read it calls for goes", async () => {
  const { page, server } = await forwardPage();
  const calls = page.timelineCalls.length;
  answerInAMinute(server);
  await pollAndGiveUp(page);
  server.hold = null;
  const arriving = server.post(message({ id: "440", content: "the service is answering again" }));
  await deliver(page, page.stream(), sseMessage(arriving));
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 2, "a live message did not end the wait for its read");
});

test("`remote-ops-audit` W7: a channel without threads waits out the server's bound too, and a pull still reads", async () => {
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, [message({ id: "1944", content: "already on screen" })]);
  const healthy = page.channelPage;
  const reads = page.pageReads;
  const sent = page.clock();
  /** @type {Array<() => void>} */
  const waiting = [];
  page.channelPage = (path, options) => new Promise((resolve) => {
    waiting.push(() => resolve(healthy(path, options)));
  });
  await reReadChannel(page);
  page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
  assert.equal(page.expireTimers(CHANNEL_READ_TIMEOUT_MS), 1, "nothing bounded the read");
  await page.settle();
  page.setClock(sent + 30000);
  await reReadChannel(page);
  assert.equal(page.pageReads, reads + 1, "the poll read again inside the server's bound");
  page.channelPage = healthy;
  page.el("scroll-area").scrollTop = 0;
  await pullDown(page, PULL_ARM_PX);
  await page.settle();
  assert.equal(page.pageReads, reads + 2, "a pull to refresh inside the wait did not read");
  for (const answer of waiting.splice(0)) answer();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["1944"]);
});

test("`remote-ops-audit` W7: coming back to the channel view inside the wait reads at once: entering a view is the reader's own act", async () => {
  for (const scrolledUp of [false, true]) {
    // A channel long enough to scroll, so a reader can leave it off its newest line: coming back is
    // then a return to their place, a read the page names `visible`, and still the reader's own act.
    const { page, server } = await forwardPage({}, (p) => {
      p.messages = [...p.messages, ...Array.from({ length: 30 }, (_, n) =>
        message({ id: String(600 + n), content: `a line of the channel, number ${n}, long enough to take room` }))];
    });
    const calls = page.timelineCalls.length;
    answerInAMinute(server);
    await pollAndGiveUp(page);
    server.hold = null;
    const area = page.el("scroll-area");
    if (scrolledUp) {
      area.scrollTop = 0;
      assert.ok(!atBottomOf(area), "fixture: the channel does not scroll, so the reader cannot leave it off its newest line");
    }
    await page.el("view-switch").click();
    await page.settle();
    assert.notEqual(page.tab(), "discord", "fixture: the switch did not leave the channel view");
    await page.el("view-switch").click();
    await page.settle();
    await page.settle();
    assert.equal(page.tab(), "discord");
    assert.equal(page.timelineCalls.length - calls, 2,
      `coming back to the channel view inside the wait did not read it (${scrolledUp ? "to the reader's place" : "at the newest"})`);
  }
});

test("`remote-ops-audit` W7: on a channel without threads the late answer of a read given up on is drawn when nothing replaced it", async () => {
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, [message({ id: "1945", content: "already on screen" })]);
  const healthy = page.channelPage;
  const sent = page.clock();
  /** @type {Array<() => void>} */
  const waiting = [];
  page.channelPage = (path, options) => new Promise((resolve) => {
    waiting.push(() => resolve(healthy(path, options)));
  });
  await reReadChannel(page);
  page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
  assert.equal(page.expireTimers(CHANNEL_READ_TIMEOUT_MS), 1, "nothing bounded the read");
  await page.settle();
  assert.match(freshness(page).textContent, /is slow to answer/);
  page.messages.push(message({ id: "1946", content: "arrived while the server was slow" }));
  page.setClock(sent + 60000);
  for (const answer of waiting.splice(0)) answer();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["1945", "1946"], "the late answer was thrown away");
  assertCurrent(page, "the late answer did not bring the channel back to current");
});

test("`remote-ops-audit` W7: the late answer of a read given up on is not drawn over a live message that came after it", async () => {
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, [message({ id: "1948", content: "already on screen" })]);
  const healthy = page.channelPage;
  const sent = page.clock();
  /** @type {Array<() => void>} */
  const waiting = [];
  page.channelPage = (path, options) => new Promise((resolve) => {
    waiting.push(() => resolve(healthy(path, options)));
  });
  await reReadChannel(page);
  page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
  assert.equal(page.expireTimers(CHANNEL_READ_TIMEOUT_MS), 1, "nothing bounded the read");
  await page.settle();
  // A live message the late answer, read before it, does not carry.
  await deliver(page, page.stream(), sseMessage(message({ id: "1949", content: "arrived live after the read was sent" })));
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["1948", "1949"], "fixture: the live message was not drawn");
  page.setClock(sent + 60000);
  for (const answer of waiting.splice(0)) answer();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["1948", "1949"], "the late answer took away a live message newer than it");
});

// Review round 1. Only the poll is put off: every read that fills a gap the stream reported, or that
// coming back to the page makes, goes at once, as it did before W7; a live message that arrives while
// the page still waits ends the wait before it can begin; and the wait ends when the read given up on
// answers, however it answers, because the server is then reading nothing.

test("`remote-ops-audit` W7: a stream reset inside the wait re-reads the channel at once, and the late answer does not take back what it brought", async () => {
  const { page, server } = await forwardPage();
  const sent = page.clock();
  const calls = page.timelineCalls.length;
  const minute = answerInAMinute(server);
  await pollAndGiveUp(page);
  // The service is answering again, and something changed upstream that the stream then lost.
  server.hold = null;
  server.edit("200", { content: "edited inside the gap" });
  server.post(message({ id: "470", content: "posted inside the gap" }));
  page.setClock(sent + 30000);
  await deliver(page, page.stream(), "event: reset\ndata: {}\n\n");
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 2,
    `a stream reset inside the wait did not re-read the channel: ${page.timelineCalls.slice(calls)}`);
  assert.ok(rowShowing(page, "edited inside the gap"), "the edit the stream lost was not drawn");
  assert.ok(shownIds(page).includes("470"), "the message the stream lost was not drawn");
  // The read given up on answers now, with the channel from before the gap.
  page.setClock(sent + 40000);
  minute.release();
  await page.settle();
  await page.settle();
  assert.ok(rowShowing(page, "edited inside the gap"), "the late answer, read before the gap, took back the edit");
  assert.ok(shownIds(page).includes("470"), "the late answer, read before the gap, took back the message");
});

test("`remote-ops-audit` W7: an edit out of the replay tail inside the wait is read at once", async () => {
  const { page, server } = await forwardPage();
  const sent = page.clock();
  const calls = page.timelineCalls.length;
  answerInAMinute(server);
  await pollAndGiveUp(page);
  server.hold = null;
  // The stream drops, the message is edited, and the stream comes back with the edit in its tail.
  page.stream().drop();
  await page.settle();
  server.edit("200", { content: "edited while the stream was away" });
  page.setClock(sent + 25000);
  page.expireTimers(sourceConstant("LIVE_RETRY_MS"));
  await page.settle();
  const edited = page.messages.find((m) => String(m.id) === "200");
  await deliver(page, page.stream(), sseUpdate("u77", edited, { replayed: true, from_tail: true }));
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 2,
    `an edit out of the replay tail inside the wait was not read: ${page.timelineCalls.slice(calls)}`);
  assert.ok(rowShowing(page, "edited while the stream was away"), "the edit out of the replay tail was not drawn");
});

test("`remote-ops-audit` W7: on a channel without threads a stream reset inside the wait re-reads the page at once", async () => {
  const page = newPage();
  page.chatProviderName = "Example Chat";
  await signIn(page);
  await showDiscord(page, [message({ id: "1950", content: "already on screen" })]);
  const healthy = page.channelPage;
  const reads = page.pageReads;
  const sent = page.clock();
  /** @type {Array<() => void>} */
  const waiting = [];
  // The channel as it stood when the read was asked, handed over a minute later.
  page.channelPage = (path, options) => {
    const asked = healthy(path, options);
    return new Promise((resolve) => waiting.push(() => resolve(asked)));
  };
  await reReadChannel(page);
  page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
  assert.equal(page.expireTimers(CHANNEL_READ_TIMEOUT_MS), 1, "nothing bounded the read");
  await page.settle();
  page.channelPage = healthy;
  page.messages.push(message({ id: "1951", content: "lost in the gap" }));
  page.setClock(sent + 30000);
  await deliver(page, page.stream(), "event: reset\ndata: {}\n\n");
  await page.settle();
  await page.settle();
  assert.equal(page.pageReads, reads + 2, "a stream reset inside the wait did not re-read the page");
  assert.deepStrictEqual(shownIds(page), ["1950", "1951"], "the message the stream lost was not drawn");
  page.setClock(sent + 40000);
  for (const answer of waiting.splice(0)) answer();
  await page.settle();
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["1950", "1951"], "the late answer, read before the gap, took back the message");
});

test("`remote-ops-audit` W7: coming back to a hidden page inside the wait reads at once", async () => {
  const { page, server } = await forwardPage();
  const sent = page.clock();
  const calls = page.timelineCalls.length;
  answerInAMinute(server);
  await pollAndGiveUp(page);
  server.hold = null;
  server.edit("200", { content: "edited while hidden" });
  await page.setVisibility("hidden");
  page.setClock(sent + CHANNEL_READ_TIMEOUT_MS + VISIBLE_REFRESH_MS + 1000);
  await page.setVisibility("visible");
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 2,
    `coming back to the page inside the wait did not read: ${page.timelineCalls.slice(calls)}`);
  assert.ok(rowShowing(page, "edited while hidden"), "the edit made while the page was hidden was not drawn");
});

test("`remote-ops-audit` W7: a poll queued behind a read given up on, and then put off, leaves no loading line up", async () => {
  const { page, server } = await forwardPage();
  const sent = page.clock();
  const calls = page.timelineCalls.length;
  answerInAMinute(server);
  // The reader pulls to refresh, and the poll comes round while that read is out: it queues behind it.
  page.el("scroll-area").scrollTop = 0;
  await pullDown(page, PULL_ARM_PX, { lift: false });
  // Not awaited: the pull's read is out until the server answers, a minute from now.
  void page.el("scroll-area").dispatch("touchend");
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 1, "fixture: the pull did not read");
  await pollOnce(page);
  assert.equal(page.timelineCalls.length - calls, 1, "fixture: the poll did not queue behind the pull's read");
  assert.equal(page.el("channel-loading").hidden, false, "fixture: no loading line while the pull's read was out");
  // Given up on at twenty seconds: the queued poll is put off, so no read is out.
  page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
  assert.ok(page.expireTimers(CHANNEL_READ_TIMEOUT_MS) >= 1, "nothing bounded the read");
  await page.settle();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 1, "fixture: the queued poll was not put off");
  page.setClock(sent + 25000);
  page.expireTimers(LOADING_LINE_DELAY_MS);
  await page.settle();
  const line = page.el("channel-loading");
  assert.equal(line.hidden, true, `the loading line stayed up with no read out: "${line.textContent}"`);
  assert.doesNotMatch(freshness(page).textContent, /refreshing/, `pill: "${freshness(page).textContent}"`);
});

test("`remote-ops-audit` W7: a live message that arrives while the page still waits ends the wait before it begins", async () => {
  const { page, server } = await forwardPage();
  const sent = page.clock();
  const calls = page.timelineCalls.length;
  answerInAMinute(server);
  await pollOnce(page);
  page.setClock(sent + 5000);
  const arriving = server.post(message({ id: "440", content: "live during the wait" }));
  await deliver(page, page.stream(), sseMessage(arriving));
  await page.settle();
  server.hold = null;
  page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
  assert.ok(page.expireTimers(CHANNEL_READ_TIMEOUT_MS) >= 1, "nothing bounded the read");
  await page.settle();
  await page.settle();
  await page.settle();
  assert.equal(page.timelineCalls.length - calls, 2,
    `the read the live message queued was dropped when the wait was given up on: ${page.timelineCalls.slice(calls)}`);
  assert.equal(page.el("channel-loading").hidden, true, "the loading line stayed up");
  // The service answered for the channel after the read was sent: the view is not held, and the poll
  // come round inside the server's bound reads.
  page.setClock(sent + 30000);
  await pollOnce(page);
  assert.equal(page.timelineCalls.length - calls, 3,
    `the view was held after a live message said the service was answering: ${page.timelineCalls.slice(calls)}`);
});

test("`remote-ops-audit` W7: once the read given up on has answered, or failed, the wait is over and the poll reads", async () => {
  for (const failWith of [null, 504]) {
    const { page, server } = await forwardPage();
    const sent = page.clock();
    const calls = page.timelineCalls.length;
    server.post(message({ id: "450", content: "posted before the poll" }));
    const minute = answerInAMinute(server);
    await pollAndGiveUp(page);
    // The server answers at twenty-five seconds: it is reading nothing now.
    page.setClock(sent + 25000);
    minute.release(failWith);
    await page.settle();
    await page.settle();
    if (!failWith) assert.ok(shownIds(page).includes("450"), "fixture: the late answer was not drawn");
    server.post(message({ id: "451", content: "posted after the late answer" }));
    page.setClock(sent + 30000);
    await pollOnce(page);
    assert.equal(page.timelineCalls.length - calls, 2,
      `the poll was put off after the read given up on had ${failWith ? "failed" : "answered"}: ${page.timelineCalls.slice(calls)}`);
    assert.ok(shownIds(page).includes("451"), `the poll after the read given up on ${failWith ? "failed" : "answered"} was not drawn`);
  }
});

/** A negative control: a placing read of All folded whatever the store has gained, lost or replaced since it was sent. */
const FOLD_OVER_ANY_STORE = ["  const unchanged = heldAsSent(canon, read.held);\n", "  const unchanged = true;\n"];
/** `#466 reply-promotes-thread`: a newest page taking the page's own fresh post out by its absence. */
const OWN_POST_FORGOTTEN = ["function ownPostStands(id) {\n", "function ownPostStands(id) {\n  return false;\n"];

/** `script` with one more thing broken on purpose, as `brokenScript` breaks one. */
function brokenAlso(script, [find, replace]) {
  assert.ok(script.includes(find), `the negative control patches ${JSON.stringify(find)}, which is not there`);
  return script.replace(find, replace);
}
/** A negative control: a read of All for Main's counts speaking for every message held when it lands. */
const COUNTS_SPEAK_FOR_ALL = ["    held: heldNow(channelStore.current()) };", "    held: null };"];
/** A negative control: a read bringing back a message removed upstream after it was sent (`#316 late-counts-resurrect`). */
const REMOVALS_FORGOTTEN = { file: "src/core/channels/fold.ts",
  find: "  return at !== undefined && at >= number;\n", replace: "  return false;\n" };

for (const mode of ["delta", "newest"]) {
  test(`\`#308 live-reply-in-main\`: a thread reply the stream delivers without its thread leaves Main for its thread once Main's ${mode} read behind it is done`, async () => {
    const run = async (script) => {
      const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
      const from = page.timelineCalls.length;
      const { first, pushed } = bridgeReply(page, server);
      // A forward position the server can no longer continue: the read behind the arrival is Main's newest page.
      if (mode === "newest") server.refuse = { status: 410, error: "cursor_expired" };
      await deliver(page, page.stream(), sseMessage(pushed));
      await page.settle();
      await page.settle();
      return { page, server, first, from };
    };
    const { page, first, from } = await run(SCRIPT);
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the thread reply was left on Main");
    const reads = readsSince(page, from);
    assert.deepStrictEqual(reads.at(-1), ["flat", false, "place"], `All was not read to place the reply: ${JSON.stringify(reads)}`);
    assert.equal(reads.filter(([, , reason]) => reason === "place").length, 1, `the reply was placed more than once: ${JSON.stringify(reads)}`);
    assert.equal(threadButton(rowOf(page, "201")).textContent, "2 replies", "the root did not count the reply");
    // In its thread, from what the page holds: nothing the thread is read for has to land first.
    page.timeline = errorResponse(502, "chat_error", "history read timed out");
    await pickThread(page, `thread:${first.id}`);
    assert.deepStrictEqual(shownIds(page), ["201", "202", "300"], "the reply is not in its thread");
    await pickThread(page, "main");
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the reply came back to Main");

    // THE CONTROL: without the read that places it, the reply stays on Main (a delta), or goes
    // nowhere (a newest page of Main), and its thread does not have it.
    const control = await run(brokenScript("  queuePlaceUnplaced();\n}", "}"));
    if (mode === "delta") {
      assert.deepStrictEqual(shownIds(control.page), ["200", "201", "203", "300"],
        "without the placing read the reply still left Main, so this test cannot see it");
    }
    control.page.timeline = errorResponse(502, "chat_error", "history read timed out");
    await pickThread(control.page, `thread:${control.first.id}`);
    assert.ok(!shownIds(control.page).includes("300"),
      "without the placing read the thread still had the reply, so this test cannot see it");
  });
}

test("`#308 live-reply-in-main`: THE OWNER'S CASE — reads failing as the reply arrives: it waits on Main, and the first read that lands places it in its thread", async () => {
  const run = async (script) => {
    const data = threadData();
    const page = newPage(new Map(), script);
    page.threadingSupported = true;
    page.threads = data.threads;
    page.messages = data.messages;
    const server = forwardTimeline(page, {});
    await signIn(page);
    await page.el("view-switch").click();
    await page.settle();
    await page.settle();
    // Opened in All, as a threaded channel does; the reader goes to Main.
    await pickThread(page, "main");
    const { first, pushed } = bridgeReply(page, server);
    const standard = page.timeline;
    page.timeline = errorResponse(502, "chat_error", "history read timed out");
    await deliver(page, page.stream(), sseMessage(pushed));
    await page.settle();
    await page.settle();
    return { page, first, standard };
  };
  const { page, first, standard } = await run(SCRIPT);
  // Nothing has said where it is: drawn where most of what arrives this way belongs.
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "300"]);
  page.timeline = standard;
  await pollOnce(page);
  await page.settle();
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the read that landed left the reply on Main");
  assert.equal(threadButton(rowOf(page, "201")).textContent, "2 replies", "the root did not count the reply");
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await pickThread(page, `thread:${first.id}`);
  assert.deepStrictEqual(shownIds(page), ["201", "202", "300"], "the reply is not in its thread");

  // THE CONTROL, which is the owner's report: Main's read lands and the reply is still not in its thread.
  const control = await run(brokenScript("  queuePlaceUnplaced();\n}", "}"));
  control.page.timeline = control.standard;
  await pollOnce(control.page);
  await control.page.settle();
  control.page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await pickThread(control.page, `thread:${control.first.id}`);
  assert.ok(!shownIds(control.page).includes("300"),
    "without the placing read the reply still reached its thread, so this test cannot see it");
});

for (const kind of ["replayed copy", "edit", "second copy"]) {
  test(`\`#308 live-reply-in-main\`: a later ${kind} from the stream that names the thread moves the reply out of Main at once, with no read to wait for`, async () => {
    const run = async (script) => {
      // The read of All that would place it never answers here: only the copy can.
      const { page, server } = await forwardPage({}, null, script);
      const { first, pushed } = bridgeReply(page, server);
      await deliver(page, page.stream(), sseMessage(pushed));
      await page.settle();
      assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "300"], "fixture: the reply was placed some other way");
      const placed = { ...pushed, thread: { ...first, is_root: false, reply_count: 2 } };
      // Whatever read the copy asks for fails: what moves the reply is the copy.
      const standard = page.timeline;
      page.timeline = errorResponse(502, "chat_error", "history read timed out");
      const from = page.timelineCalls.length;
      const frame = kind === "replayed copy" ? sseMessage(placed, { replayed: true })
        : kind === "edit" ? sseUpdate("edit-300", placed) : sseMessage(placed);
      await deliver(page, page.stream(), frame);
      await page.settle();
      return { page, first, from, standard };
    };
    const { page, first, from, standard } = await run(SCRIPT);
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the copy naming the thread left the reply on Main");
    assert.equal(threadButton(rowOf(page, "201")).textContent, "2 replies", "the root did not count the reply");
    if (kind === "replayed copy") assert.deepStrictEqual(page.timelineCalls.slice(from), [], "a replayed copy was read for");
    page.timeline = standard;
    await pickThread(page, `thread:${first.id}`);
    assert.deepStrictEqual(shownIds(page), ["201", "202", "300"], "the reply is not in its thread");

    // THE CONTROL: a copy that places nothing leaves the reply where the threadless one drew it.
    const control = await run(kind === "edit"
      ? brokenScript("      placeFromEdit(payload.message);\n", "")
      : brokenScript("    const places = placesHeld(message);\n", "    const places = false;\n"));
    if (kind === "second copy") {
      // The copy itself still replaces the store's, and the row on Main stays until a read redraws it.
      assert.ok(shownIds(control.page).includes("300"), "without the placement the reply still left Main, so this test cannot see it");
    } else {
      assert.deepStrictEqual(shownIds(control.page), ["200", "201", "203", "300"],
        "without the placement the reply still left Main, so this test cannot see it");
    }
  });
}

/**
 * A channel with threads taller than the list — a question with a thread at its head, then fourteen
 * long messages to the channel — on a server with a change record, opened in All and read in Main,
 * and polled once there, so Main's next read is a delta.
 */
async function tallThreadedMain(script = SCRIPT) {
  const page = newPage(new Map(), script);
  page.threadingSupported = true;
  const thread = { id: "spaces/A/threads/tall", root_message_id: "27720", is_root: true,
    reply_count: 1, reply_count_exact: true };
  const root = message({ id: "27720", content: longMessage("the question"), thread });
  const answer = message({ id: "27721", content: "the first answer", thread: { ...thread, is_root: false } });
  const rest = Array.from({ length: 14 }, (_unused, index) =>
    message({ id: `277${String(index).padStart(2, "0")}`, content: longMessage(`message ${index}`) }));
  page.messages = [root, answer, ...rest];
  page.threads = [{ id: thread.id, root, title: "The question", reply_count: 1, reply_count_exact: true,
    updated_at: answer.timestamp }];
  const server = forwardTimeline(page, {});
  await signIn(page);
  await page.el("view-switch").click();
  await page.settle();
  await page.settle();
  await pickThread(page, "main");
  await pollOnce(page);
  /** Post a reply into the question's thread, and answer the stream's copy of it, with no thread record. */
  const reply = (id) => {
    const pushed = message({ id, content: longMessage(`the bridge's answer ${id}`) });
    const count = page.messages.filter((m) => m.thread && m.thread.id === thread.id && !m.thread.is_root).length + 1;
    server.post({ ...pushed, thread: { ...thread, is_root: false, reply_count: count } });
    server.edit("27720", { thread: { ...thread, reply_count: count } });
    return pushed;
  };
  return { page, server, thread, reply };
}

test("`#308 live-reply-in-main` x `#280 scroll-stability`: the reply leaving Main moves no reader — one up the list stays on their row, and the jump no longer counts it; one on the newest line stays on it", async () => {
  const run = async (script) => {
    const { page, reply } = await tallThreadedMain(script);
    const area = page.el("scroll-area");
    assert.ok(atBottomOf(area), "the channel did not open on its newest line");
    area.scrollTop = Math.round(area.scrollHeight / 3);
    const before = readerOn(page);
    await deliver(page, page.stream(), sseMessage(reply("27790")));
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, area, before, reply };
  };
  const { page, area, before, reply } = await run(SCRIPT);
  assert.ok(!shownIds(page).includes("27790"), "the thread reply was left on Main");
  assert.deepStrictEqual(readerOn(page), before, "the reply leaving Main moved the row the reader was on");
  assert.equal(jumpSaysArrived(page), false, "the jump still counts a message that is not on Main");
  assert.equal(page.el("jump-newest").hidden, false, "the way back down went from a reader up the list");

  page.el("jump-newest").click();
  await page.settle();
  assert.ok(atBottomOf(area));
  await deliver(page, page.stream(), sseMessage(reply("27791")));
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.ok(!shownIds(page).includes("27791"), "the second thread reply was left on Main");
  assert.ok(atBottomOf(area), "a reader on the newest line was taken off it");
  assert.equal(jumpSaysArrived(page), false, "a message leaving Main was offered as news");

  // THE CONTROLS: the jump still counting what left, and the reader taken to the newest line.
  const counting = await run(brokenScript("    if (!newer && gone > 0 && typeof counted === \"number\") {",
    "    if (false) {"));
  assert.equal(jumpSaysArrived(counting.page), true, "the jump never counted the reply, so this test cannot see it");
  const thrown = await run(brokenScript("settleAfterRead(shown, { keepPosition: true, area, ...position, ownAct: !newer });",
    "settleAfterRead(shown, { area, ...position, keepPosition: false, ownAct: !newer });"));
  assert.notDeepStrictEqual(readerOn(thrown.page), thrown.before, "the reader stayed put anyway, so this test cannot see it");
});

test("`#308 live-reply-in-main`: in two columns the reply leaves Main on the left for its thread on the right", async () => {
  const run = async (script) => {
    const { page, server, data } = await twoColumnServer(null, script);
    await threadBadge(rowWithId(page, "201")).click();
    await page.settle();
    assert.deepStrictEqual(sideIds(page), ["201", "202"], "fixture: the thread is not on the right");
    const { pushed } = bridgeReply(page, server);
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, data };
  };
  const { page } = await run(SCRIPT);
  assert.ok(!shownIds(page).includes("300"), "the thread reply was left on Main");
  assert.deepStrictEqual(sideIds(page), ["201", "202", "300"], "the reply did not reach its thread on the right");
  assert.equal(threadButton(rowWithId(page, "201")).textContent, "2 replies", "Main's root did not count the reply");

  const control = await run(brokenScript("  queuePlaceUnplaced();\n}", "}"));
  assert.ok(!sideIds(control.page).includes("300"),
    "without the placing read the reply still reached the right column, so this test cannot see it");
});

test("`#308 live-reply-in-main`: a placing read that fails is asked again when the next read ends; one that lands is not asked again for what it did not carry", async () => {
  const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; });
  const serve = page.timeline;
  let failing = 1;
  page.timeline = (path, options) => {
    if (/reason=place/.test(path) && failing > 0) {
      failing -= 1;
      return errorResponse(502, "chat_error", "history read timed out")();
    }
    return serve(path, options);
  };
  const placing = () => page.timelineCalls.filter((call) => /reason=place/.test(call)).length;
  const { first, pushed } = bridgeReply(page, server);
  await deliver(page, page.stream(), sseMessage(pushed));
  await page.settle();
  await page.settle();
  assert.equal(placing(), 1, "no read of All was made to place the reply");
  assert.ok(shownIds(page).includes("300"), "fixture: the placing read did not fail");
  assert.notEqual(freshness(page).getAttribute("data-state"), "failed", "a placing read nobody asked for said the refresh failed");
  await pollOnce(page);
  await page.settle();
  assert.equal(placing(), 2, "the placing read that failed was not asked again when the poll's read ended");
  assert.ok(!shownIds(page).includes("300"), "the placing read that landed left the reply on Main");

  // A message All's newest page does not carry: asked once, and then not again on every poll.
  await deliver(page, page.stream(), sseMessage(message({ id: "310", content: "never on any page" })));
  await page.settle();
  await page.settle();
  assert.equal(placing(), 3);
  await pollOnce(page);
  await pollOnce(page);
  assert.equal(placing(), 3, "a message All's page had answered for was asked for again");
  assert.ok(first, "fixture");
});

test("`#308 live-reply-in-main`: the placing read asks only what changed in All since All's last read, and All's newest page when the server cannot continue from there", async () => {
  for (const refused of [false, true]) {
    const { page, reply } = await tallThreadedMain();
    const serve = page.timeline;
    let refusals = refused ? 1 : 0;
    page.timeline = (path, options) => {
      if (/reason=place/.test(path) && /after=/.test(path) && refusals > 0) {
        refusals -= 1;
        return json(410, { error: "cursor_expired", detail: "that position is gone" });
      }
      return serve(path, options);
    };
    const from = page.timelineCalls.length;
    await deliver(page, page.stream(), sseMessage(reply("27790")));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(!shownIds(page).includes("27790"), `the thread reply was left on Main (refused: ${refused})`);
    const placing = readsSince(page, from).filter(([, , reason]) => reason === "place");
    assert.deepStrictEqual(placing, refused ? [["flat", true, "place"], ["flat", false, "place"]] : [["flat", true, "place"]],
      `the placing read did not continue All, or fall back to its newest page: ${JSON.stringify(readsSince(page, from))}`);
  }
});

test("`#308 live-reply-in-main` x `remote-ops-audit` W7: a placing read given up on is not asked again while the server may still be reading for it, and the poll of the view on screen waits for it", async () => {
  const run = async (script) => {
    const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
    const serve = page.timeline;
    const state = { hang: true };
    page.timeline = (path, options) =>
      (/reason=place/.test(path) && state.hang ? new Promise(() => {}) : serve(path, options));
    const placing = () => page.timelineCalls.filter((call) => /reason=place/.test(call)).length;
    const { pushed } = bridgeReply(page, server);
    const sent = page.clock();
    await deliver(page, page.stream(), sseMessage(pushed));
    await page.settle();
    assert.equal(placing(), 1, "no read of All was made to place the reply");
    page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
    assert.ok(page.expireTimers(CHANNEL_READ_TIMEOUT_MS) >= 1, "nothing bounded the placing read");
    await page.settle();
    state.hang = false;
    page.setClock(sent + 30000);
    const calls = page.timelineCalls.length;
    await pollOnce(page);
    const polled = page.timelineCalls.length - calls;
    // The reader's own act reads at once, as inside any wait, and its end does not ask again.
    page.el("scroll-area").scrollTop = 0;
    await pullDown(page, PULL_ARM_PX);
    await page.settle();
    await page.settle();
    return { page, placing, sent, polled, pulled: page.timelineCalls.length - calls - polled };
  };
  const { page, placing, sent, polled, pulled } = await run(SCRIPT);
  assert.equal(polled, 0, "the poll of Main went beside a placing read the server may still be answering");
  assert.ok(pulled >= 1, "a pull to refresh inside the wait did not read");
  assert.equal(placing(), 1, "the placing read given up on was asked again inside the server's bound");
  assert.ok(shownIds(page).includes("300"), "fixture: something else placed the reply");
  page.setClock(sent + SERVER_READ_BOUND_MS + 1000);
  await pollOnce(page);
  assert.equal(placing(), 2, "the placing read was not asked again once the server's bound had passed");
  assert.ok(!shownIds(page).includes("300"), "the placing read that landed left the reply on Main");

  // THE CONTROLS: without the wait, the pull's read ending asks again at once; held for no view, the
  // poll of Main goes beside it.
  const control = await run(brokenScript(" ||\n      placeUnplacedWaits(channel)) return;", ") return;"));
  assert.equal(control.placing(), 2, "the placing read waited anyway, so this test cannot see it");
  const unheld = await run(brokenScript("  const key = inColumn(columns.main, readHoldKey);\n", "  const key = \"elsewhere\";\n"));
  assert.ok(unheld.polled > 0, "the poll of Main waited for a placing read held for no view, so this test cannot see it");
});

test("`#308 live-reply-in-main` x `remote-ops-audit` W7: the late answer of a placing read given up on places the reply and brings Main current, and the poll waits for it rather than throwing it away", async () => {
  const run = async (script) => {
    const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
    const serve = page.timeline;
    /** @type {Array<() => void>} */
    const held = [];
    // Each placing read answered as the server had it when asked, and handed over when the test says.
    page.timeline = (path, options) => {
      if (!/reason=place/.test(path)) return serve(path, options);
      const answer = serve(path, options);
      return new Promise((resolve) => held.push(() => resolve(answer)));
    };
    const { pushed } = bridgeReply(page, server);
    const sent = page.clock();
    await deliver(page, page.stream(), sseMessage(pushed));
    await page.settle();
    assert.equal(held.length, 1, "no read of All was made to place the reply");
    page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
    assert.ok(page.expireTimers(CHANNEL_READ_TIMEOUT_MS) >= 1, "nothing bounded the placing read");
    await page.settle();
    assert.ok(shownIds(page).includes("300"), "fixture: the placing read was answered before it was given up on");
    page.setClock(sent + 30000);
    const calls = page.timelineCalls.length;
    await pollOnce(page);
    const polled = page.timelineCalls.length - calls;
    // The server answers the placing read at forty seconds, as a slow bridge's newest page does.
    page.setClock(sent + 40000);
    held[0]();
    for (let i = 0; i < 4; i += 1) await page.settle();
    return { page, polled, held };
  };
  const { page, polled, held } = await run(SCRIPT);
  assert.equal(polled, 0, "the poll of Main went beside the placing read, which throws its answer away");
  assert.equal(held.length, 1, "the placing read was asked again");
  assert.ok(!shownIds(page).includes("300"), "the late answer of the placing read was thrown away");
  assert.equal(threadButton(rowOf(page, "201")).textContent, "2 replies", "the root did not count the reply");
  assertCurrent(page, "All's late answer did not bring Main current");
  /** The ids of the messages the device saved for the channel. */
  const heldIds = (shown) => savedScopes(shown)[scopeKey(CHANNEL.id)].messages.map((m) => m.id);
  assert.ok(heldIds(page).includes("204"), "All's late answer was not folded: a reply it carries is not held");
  const before = page.timelineCalls.length;
  await pollOnce(page);
  assert.deepStrictEqual(readsSince(page, before).map(([view]) => view), ["main"],
    "the poll of Main did not go once the placing read had answered");

  // THE CONTROLS: a late answer dropped, as round 1 did, leaves the reply on Main; held for no view,
  // the poll of Main goes beside the placing read, and its answer, replaced, is not folded: it only
  // places what it carries (`placeFromAnswer`, review round 3), and brings nothing else of All.
  const dropped = await run(brokenScript("(late) => landPlacingRead(late, read),", "() => {},"));
  assert.ok(shownIds(dropped.page).includes("300"), "the late answer placed the reply anyway, so this test cannot see it");
  const unheld = await run(brokenScript("  const key = inColumn(columns.main, readHoldKey);\n", "  const key = \"elsewhere\";\n"));
  assert.ok(unheld.polled > 0 && !heldIds(unheld.page).includes("204"),
    "the poll of Main waited, or the late answer stood beside it, so this test cannot see it");
});

test("`#308 live-reply-in-main`: a placing read that lands brings Main current, as any read of All does, though Main's own read failed", async () => {
  const run = async (script) => {
    const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
    const serve = page.timeline;
    page.timeline = (path, options) => (/reason=place/.test(path) ? serve(path, options)
      : errorResponse(502, "chat_error", "history read timed out")());
    const { pushed } = bridgeReply(page, server);
    // Later than Main's last read, as any arrival is.
    page.setClock(page.clock() + 5000);
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page };
  };
  const { page } = await run(SCRIPT);
  assert.ok(!shownIds(page).includes("300"), "the placing read left the reply on Main");
  assertCurrent(page, "All's answer did not bring Main current");

  const control = await run(brokenScript("  followPlacement();\n  eachColumn(catchUpFromAll);\n", "  followPlacement();\n"));
  assert.notEqual(freshness(control.page).getAttribute("data-state"), "fresh",
    "Main was current without All's answer, so this test cannot see it");
});

test("`#308 live-reply-in-main`: a placing page of All that lands after the stream delivered another message does not take that message away, and is read again", async () => {
  const run = async (script) => {
    const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
    const serve = page.timeline;
    const held = [];
    let holding = true;
    // Answered as the server had it when asked, and handed over later.
    page.timeline = (path, options) => {
      if (!/reason=place/.test(path) || !holding) return serve(path, options);
      const answer = serve(path, options);
      return new Promise((resolve) => held.push(() => resolve(answer)));
    };
    const { pushed } = bridgeReply(page, server);
    await deliver(page, page.stream(), sseMessage(pushed));
    await page.settle();
    assert.equal(held.length, 1, "no read of All was made to place the reply");
    // A message to the channel itself, delivered while that read is out: its page cannot carry it.
    const aside = server.post(message({ id: "207", content: "an aside on the main channel" }));
    await deliver(page, page.stream(), sseMessage(aside));
    await page.settle();
    assert.ok(shownIds(page).includes("207"), "fixture: the aside was not drawn");
    holding = false;
    held[0]();
    for (let i = 0; i < 4; i += 1) await page.settle();
    return { page };
  };
  const { page } = await run(SCRIPT);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "207"],
    "the placing page took away a message delivered after it was asked, or left the reply on Main");

  // THE CONTROL: folded regardless — of the stream, and of the store having gained the aside since it
  // was sent (review round 4) — the page sent before the aside takes it out as deleted.
  const control = await run(brokenScriptEach(["  const quiet = liveEvents.seen(channel) === read.live;",
    "  const quiet = true;"], FOLD_OVER_ANY_STORE));
  assert.ok(!shownIds(control.page).includes("207"),
    "the stale placing page left the aside alone anyway, so this test cannot see it");
});

// Review round 2. A read of the channel the page gave up on may still be answered (`remote-ops-audit`
// W7), and its late answer is used only if no read of the channel was sent after it. A placing read
// sent beside it threw that answer away, and was the second scan W7 exists to prevent. Behind a slow
// bridge, where newest pages take longer than the page waits for them, that is what happens to every
// message the stream delivers without a thread record whose Main read was given up on: a thread reply,
// or a message posted to the channel itself.

for (const kind of ["thread reply", "channel message"]) {
  for (const answersAt of [35000, 50000]) {
    test(`\`#308 live-reply-in-main\` x \`remote-ops-audit\` W7: a ${kind} the stream sent without its thread, Main's read given up on and answered at ${answersAt / 1000} s: the late answer lands, the pill is current, and no read went beside it`, async () => {
      const run = async (script) => {
        const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
        const placing = () => page.timelineCalls.filter((call) => /reason=place/.test(call)).length;
        // A message to the channel itself that only the read carries: the stream never delivers it.
        server.post(message({ id: "450", content: "posted to the channel, and only on its read" }));
        const minute = answerInAMinute(server);
        const pushed = kind === "thread reply" ? bridgeReply(page, server).pushed
          : server.post(message({ id: "460", content: "posted to the channel, and pushed with no thread record" }));
        const sent = page.clock();
        await deliver(page, page.stream(), sseMessage(pushed));
        await page.settle();
        assert.ok(shownIds(page).includes(pushed.id), "fixture: the pushed message was not drawn on arrival");
        page.setClock(sent + CHANNEL_READ_TIMEOUT_MS);
        assert.ok(page.expireTimers(CHANNEL_READ_TIMEOUT_MS) >= 1, "nothing bounded Main's read");
        await page.settle();
        await page.settle();
        const beside = placing();
        assert.ok(!shownIds(page).includes("450"), "fixture: Main's read was answered before it was given up on");
        // Anything sent at twenty seconds is given up on at forty.
        if (answersAt > 2 * CHANNEL_READ_TIMEOUT_MS) {
          page.setClock(sent + 2 * CHANNEL_READ_TIMEOUT_MS);
          page.expireTimers(CHANNEL_READ_TIMEOUT_MS);
          await page.settle();
        }
        page.setClock(sent + answersAt);
        minute.release();
        for (let i = 0; i < 4; i += 1) await page.settle();
        return { page, beside, placing, pushed };
      };
      const { page, beside, placing, pushed } = await run(SCRIPT);
      assert.equal(beside, 0, "a placing read went beside Main's read while the server was still answering it");
      assert.ok(shownIds(page).includes("450"), "the late answer of Main's read was thrown away");
      assertCurrent(page, "the late answer of Main's read did not bring the channel back to current");
      if (kind === "thread reply") {
        assert.ok(!shownIds(page).includes(pushed.id), "the reply was not placed once Main's late answer had landed");
        assert.equal(placing(), 1, "the reply was not placed by one read");
      } else {
        assert.ok(shownIds(page).includes(pushed.id), "the message posted to the channel left Main");
        assert.equal(placing(), 0, "a read of All went for a message Main's late answer had placed");
      }

      // THE CONTROL: placing as soon as Main's read is given up on, as round 1 did, reads beside it.
      const control = await run(brokenScript(" ||\n      placeUnplacedWaits(channel)) return;", ") return;"));
      assert.ok(control.beside > 0, "no placing read went beside Main's read without the wait, so this test cannot see it");
    });
  }
}

// Review round 3. An answer of All the page does not fold is not thrown away: each message the stream
// left unplaced that it carries takes the thread record it carries there, and one that a newest page
// unable to speak for it took out of the store is put back. Which of two newest pages of All is the
// newer is the order they were sent in, not the order they landed in. And a replayed copy that places a
// message gives the store its thread record and nothing else.

/** The store's copy of message `id`, as the device saved it, or null. */
const savedMessage = (page, id) =>
  (savedScopes(page)[scopeKey(CHANNEL.id)] || { messages: [] }).messages.find((m) => String(m.id) === id) || null;

/** Thread `threadId` drawn from what the page holds: every read it asks for fails. */
async function threadFromStore(page, threadId) {
  const standard = page.timeline;
  page.timeline = errorResponse(502, "chat_error", "history read timed out");
  await pickThread(page, `thread:${threadId}`);
  const ids = shownIds(page);
  page.timeline = standard;
  return ids;
}

/**
 * The threaded channel opened in All and read in Main, on a server with a change record, with every read
 * of All for Main's counts (`readAllForCounts`) and every placing read (`placeUnplaced`) held until the
 * test hands its answer over, answered as the server had it when asked. `quietReply` posts a reply in the
 * second discussion that the stream never delivers, so the next read of Main finds that root's count
 * outrunning the store and reads All behind it for its counts.
 */
async function heldAllReads(script = SCRIPT) {
  const data = threadData();
  const page = newPage(new Map(), script);
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  const server = forwardTimeline(page, {});
  await signIn(page);
  await page.el("view-switch").click();
  await page.settle();
  await page.settle();
  await pickThread(page, "main");
  const serve = page.timeline;
  /** @type {{counts: Array<() => void>, place: Array<() => void>}} */
  const held = { counts: [], place: [] };
  page.timeline = (path, options) => {
    const reason = new URL(path, "http://fixture.test").searchParams.get("reason");
    if (reason !== "counts" && reason !== "place") return serve(path, options);
    const answer = serve(path, options);
    return new Promise((resolve) => held[reason].push(() => resolve(answer)));
  };
  const quietReply = () => {
    const second = page.messages.find((m) => m.id === "203").thread;
    server.post(message({ id: "250", content: "a quiet reply in the second discussion",
      thread: { ...second, is_root: false, reply_count: 2 } }));
    server.edit("203", { thread: { ...second, reply_count: 2, reply_count_exact: true } });
  };
  return { page, server, held, quietReply };
}

for (const mode of ["newest", "delta"]) {
  test(`\`#308 live-reply-in-main\`: a read of All for Main's counts sent before the reply arrived, landing while the placing ${mode === "newest" ? "page" : "delta"} is out, does not lose the reply`, async () => {
    const run = async (script) => {
      const { page, server, held, quietReply } = await heldAllReads(script);
      // Past the counts interval: and past All's reconcile window, so the placing read is All's newest
      // page, or within it, so the placing read asks what changed in All since All's last read.
      page.setClock(page.clock() + (mode === "newest" ? TIMELINE_RECONCILE_MS + 60000 : COUNTS_ALL_READ_MS + 1000));
      quietReply();
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(held.counts.length, 1, "fixture: All was not read for Main's counts");
      const { first, pushed } = bridgeReply(page, server);
      await deliver(page, page.stream(), sseMessage(pushed));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(held.place.length, 1, "fixture: no read of All was made to place the reply");
      // The read for the counts, sent first, lands first. It speaks only for what the store held when it
      // was sent, so it keeps the reply, which it was sent too early to carry (`#310`, review round 4).
      // Then the placing read lands.
      page.setClock(page.clock() + 5000);
      held.counts[0]();
      for (let i = 0; i < 3; i += 1) await page.settle();
      const afterCounts = savedMessage(page, "300");
      page.setClock(page.clock() + 5000);
      held.place[0]();
      for (let i = 0; i < 3; i += 1) await page.settle();
      return { page, first, afterCounts };
    };
    /** Whether the store holds the reply in its thread, which then draws it there. */
    const placed = async (page, first) => {
      const kept = savedMessage(page, "300");
      return Boolean(kept && kept.thread && kept.thread.id === first.id) &&
        (await threadFromStore(page, first.id)).includes("300");
    };
    const { page, first, afterCounts } = await run(SCRIPT);
    assert.ok(afterCounts, "the read for the counts, sent before the reply, took it out of the store");
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the reply was left on Main");
    const kept = savedMessage(page, "300");
    assert.ok(kept && kept.thread && kept.thread.id === first.id,
      `the store lost the reply, or did not place it: ${JSON.stringify(kept)}`);
    assert.deepStrictEqual(savedScopes(page)[scopeKey(CHANNEL.id)].unplaced, [], "the reply is still saved as unplaced");
    assert.deepStrictEqual(await threadFromStore(page, first.id), ["201", "202", "300"], "the reply is not in its thread");

    // THE CONTROLS. The read for the counts speaking for every message held when it lands takes the
    // reply out of the store, as round 3 did; the placing read still puts it back in its thread.
    const speaking = await run(brokenScript(...COUNTS_SPEAK_FOR_ALL));
    assert.equal(speaking.afterCounts, null, "the read for the counts kept the reply anyway, so this test cannot see it");
    assert.ok(await placed(speaking.page, speaking.first), "the placing read did not put the reply back in its thread");
    // And with that, as round 2 did: the newest page taken as the newer because it LANDED after the
    // placing read was sent; or the delta, which no longer continues All, putting back nothing. Either
    // way the placing read is marked as having answered, and the reply is in no column and not held.
    const control = await run(mode === "newest"
      // `readOrder` is the legacy script's, found in the page's global scope at run time.
      ? await brokenModule([{ file: "src/core/channels/fold.ts", find: "      cover.fullSent = deps.numberOf();\n",
        replace: "      cover.fullSent = readOrder.asked() + 1;\n" }], brokenScriptEach(COUNTS_SPEAK_FOR_ALL))
      : await brokenModule([{ file: "src/core/channels/placement.ts", find: "  const back = newerPage(canon, \"flat\", number) ? []",
        replace: "  const back = true ? []" }], brokenScriptEach(COUNTS_SPEAK_FOR_ALL)));
    assert.equal(savedMessage(control.page, "300"), null, "the reply was held anyway, so this test cannot see it");
    assert.ok(!(await threadFromStore(control.page, control.first.id)).includes("300"),
      "the reply reached its thread anyway, so this test cannot see it");
  });
}

for (const change of ["removed", "edited"]) {
  test(`\`#308 live-reply-in-main\`: a placing page of All is not folded over a newest page of All sent after it that landed first (a reply ${change} upstream in between)`, async () => {
    const run = async (script) => {
      const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
      // All read once, so the store holds every message All's pages carry; the reader goes back to Main.
      await pickThread(page, "flat");
      for (let i = 0; i < 3; i += 1) await page.settle();
      await pickThread(page, "main");
      for (let i = 0; i < 3; i += 1) await page.settle();
      const serve = page.timeline;
      /** @type {Array<() => void>} */
      const held = [];
      page.timeline = (path, options) => {
        if (!/reason=place/.test(path)) return serve(path, options);
        // The server cannot continue All from there: the placing read is All's newest page.
        if (/after=/.test(path)) return json(410, { error: "cursor_expired", detail: "that position is gone" });
        // Answered as the server had it when asked, edits included: `server.edit` changes the messages
        // an answer not yet read out would carry.
        const answer = Promise.resolve(serve(path, options)).then(async (response) => {
          const body = await response.text();
          return { ...response, text: async () => body };
        });
        return new Promise((resolve) => held.push(() => resolve(answer)));
      };
      const { pushed } = bridgeReply(page, server);
      await deliver(page, page.stream(), sseMessage(pushed));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(held.length, 1, "fixture: no read of All was made to place the reply");
      // Removed or edited upstream once the placing page was taken; the reader opens All, whose own page
      // says so. The store then no longer holds what it held when the placing page was sent
      // (`heldAsSent`), and apart from that, the order the two pages were sent in tells which is newer.
      if (change === "removed") server.remove("202");
      else server.edit("202", { content: "a reply in the first discussion, edited" });
      page.setClock(page.clock() + 5000);
      const from = page.timelineCalls.length;
      await pickThread(page, "flat");
      for (let i = 0; i < 3; i += 1) await page.settle();
      // All's poll, the server unable to continue it from its last read: its newest page.
      server.refuse = { status: 410, error: "cursor_expired" };
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.deepStrictEqual(readsSince(page, from).at(-1), ["flat", false, "poll"],
        `fixture: the reader's last read of All was not its newest page: ${JSON.stringify(readsSince(page, from))}`);
      assert.ok(shownIds(page).includes("300") && (change === "removed" ? !shownIds(page).includes("202")
        : savedMessage(page, "202").content.endsWith("edited")), "fixture: All's own page is not the newer");
      page.setClock(page.clock() + 5000);
      held[0]();
      for (let i = 0; i < 3; i += 1) await page.settle();
      return { page };
    };
    const { page } = await run(SCRIPT);
    if (change === "removed") {
      assert.deepStrictEqual(shownIds(page), ["200", "201", "203", "204", "300"],
        "the placing page, the older, put back a reply removed upstream, or took the bridge's away");
    } else {
      assert.deepStrictEqual(shownIds(page), ["200", "201", "202", "203", "204", "300"], "the placing page took the bridge's reply away");
      assert.ok(savedMessage(page, "202").content.endsWith("edited"), "the placing page, the older, undid an edit made upstream");
    }

    // THE CONTROL: with nothing to say which was sent first, the older page is folded over the newer,
    // if also folded whatever the store has lost or replaced since it was sent (review rounds 4 and 5).
    const unordered = { file: "src/core/channels/fold.ts", find: "      cover.fullSent = deps.numberOf();\n", replace: "" };
    const control = await run(await brokenModule([unordered], brokenScriptEach(FOLD_OVER_ANY_STORE)));
    assert.ok(change === "removed" ? shownIds(control.page).includes("202") : !savedMessage(control.page, "202").content.endsWith("edited"),
      "the older page was not folded anyway, so this test cannot see it");
  });
}

test("`#308 live-reply-in-main`: a read of All for Main's counts sent after a placing page, and landing after it, is folded over it", async () => {
  const edited = "a reply in the first discussion, edited";
  const run = async (script) => {
    const { page, server, held } = await heldAllReads(script);
    // All brought up to date by what changed in it, a minute inside its reconcile window: the interval
    // for Main's counts runs from that read, and All's last newest page stays as old as it was.
    page.setClock(page.clock() + TIMELINE_RECONCILE_MS - 60000);
    await pickThread(page, "flat");
    for (let i = 0; i < 3; i += 1) await page.settle();
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    await pickThread(page, "main");
    for (let i = 0; i < 3; i += 1) await page.settle();
    // Past All's reconcile window, within the interval: the bridge's answer, pushed with no thread
    // record, moves its root's count past what the store holds, and only the placing read goes.
    page.setClock(page.clock() + 120000);
    const from = page.timelineCalls.length;
    const { pushed } = bridgeReply(page, server);
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.deepStrictEqual([held.place.length, held.counts.length], [1, 0], "fixture: not the placing read alone");
    assert.deepStrictEqual(readsSince(page, from).filter(([, , reason]) => reason === "place"), [["flat", false, "place"]],
      "fixture: the placing read is not All's newest page");
    // A reply in that discussion edited upstream, which no read of Main carries: a copy of its own, so
    // the placing page already taken keeps the text it was taken with. Past the interval, the poll's
    // read of Main brings nothing, and asks All for its counts.
    const reply = page.messages.findIndex((m) => m.id === "202");
    page.messages[reply] = { ...page.messages[reply] };
    server.edit("202", { content: edited });
    page.setClock(page.clock() + COUNTS_ALL_READ_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(held.counts.length, 1, "fixture: All was not read for Main's counts");
    // The placing page, sent first, lands first, over the store as it was when it was sent; then the
    // read for the counts, the newer.
    page.setClock(page.clock() + 5000);
    held.place[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.notEqual(savedMessage(page, "202").content, edited, "fixture: the placing page carried the edit");
    page.setClock(page.clock() + 5000);
    held.counts[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page };
  };
  const { page } = await run(SCRIPT);
  assert.equal(savedMessage(page, "202").content, edited, "the read for Main's counts was not folded, and the edit it carried is not held");

  // THE CONTROL, as round 2 did: the placing page taken as the newer because it LANDED after the read
  // for the counts was sent.
  const control = await run(brokenScript("    if (!newerPage(channelStore.current(), \"flat\", number)) {",
    "    if (!(channelStore.current().views.get(viewKey(\"flat\")) && channelStore.current().views.get(viewKey(\"flat\")).fullAt > now)) {"));
  assert.notEqual(savedMessage(control.page, "202").content, edited, "the read for the counts was folded anyway, so this test cannot see it");
});

for (const mode of ["delta", "newest"]) {
  test(`\`#308 live-reply-in-main\`: on a busy channel — the bridge editing its answer while each placing read is out — the first placing read places the reply (Main read by ${mode === "delta" ? "what changed" : "its newest page"})`, async () => {
    const run = async (script) => {
      const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
      const serve = page.timeline;
      /** @type {Array<() => void>} */
      const held = [];
      page.timeline = (path, options) => {
        if (!/reason=place/.test(path)) return serve(path, options);
        const answer = serve(path, options);
        return new Promise((resolve) => held.push(() => resolve(answer)));
      };
      // Main's every read its newest page, which never carries a thread reply, and so takes the one the
      // stream pushed out of the store.
      const newestOnly = () => {
        if (mode === "newest") server.refuse = { status: 410, error: "cursor_expired" };
      };
      const { first, pushed } = bridgeReply(page, server);
      newestOnly();
      await deliver(page, page.stream(), sseMessage(pushed));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(held.length, 1, "fixture: no read of All was made to place the reply");
      /** @type {boolean[]} after each placing read lands, whether the reply is on Main */
      const onMain = [];
      for (let cycle = 0; cycle < 3; cycle += 1) {
        // The bridge edits its answer while the placing read is out, as one that streams its text does,
        // and the edit comes without its thread, as the push does.
        const edited = { ...pushed, content: `the bridge's answer, edit ${cycle}` };
        server.edit("300", { content: edited.content });
        newestOnly();
        await deliver(page, page.stream(), sseUpdate(`edit-300-${cycle}`, edited));
        for (let i = 0; i < 3; i += 1) await page.settle();
        held[held.length - 1]();
        for (let i = 0; i < 3; i += 1) await page.settle();
        onMain.push(shownIds(page).includes("300"));
      }
      return { page, first, held, onMain };
    };
    const { page, first, held, onMain } = await run(SCRIPT);
    assert.equal(held.length, 1, "a placing page was thrown away and asked again");
    assert.deepStrictEqual(onMain, [false, false, false], "the reply stayed on Main while the channel was busy");
    const kept = savedMessage(page, "300");
    // Held in its thread: what draws it there once All or the thread is covered.
    assert.ok(kept && kept.thread && kept.thread.id === first.id, `the reply is not held in its thread: ${JSON.stringify(kept)}`);
    assert.deepStrictEqual(savedScopes(page)[scopeKey(CHANNEL.id)].unplaced, [], "the reply is still saved as unplaced");

    // THE CONTROLS: a placing page the stream has overtaken thrown away whole, as round 2 did, is asked
    // again on every edit; and one the store no longer holds, not put back, is lost.
    const thrown = await run(brokenScript("  if (!folded) {\n", "  if (!folded && !delta && !quiet) return false;\n  if (!folded) {\n"));
    assert.ok(thrown.held.length > 1, "the stale placing page was not asked again anyway, so this test cannot see it");
    if (mode === "delta") {
      assert.deepStrictEqual(thrown.onMain, [true, true, true], "the reply left Main anyway, so this test cannot see it");
    } else {
      const lost = await run(await brokenModule([{ file: "src/core/channels/placement.ts", find: "  const back = newerPage(canon, \"flat\", number) ? []",
        replace: "  const back = true ? []" }]));
      assert.equal(savedMessage(lost.page, "300"), null, "the reply was held anyway, so this test cannot see it");
    }
  });
}

test("`#308 live-reply-in-main` x `remote-ops-audit` W7: behind a slow bridge editing its answer, a placing read given up on and then replaced still places the reply when its late answer lands", async () => {
  const run = async (script) => {
    const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
    const serve = page.timeline;
    /** @type {Array<() => void>} */
    const held = [];
    page.timeline = (path, options) => {
      if (!/reason=place/.test(path)) return serve(path, options);
      const answer = serve(path, options);
      return new Promise((resolve) => held.push(() => resolve(answer)));
    };
    const { first, pushed } = bridgeReply(page, server);
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(held.length, 1, "fixture: no read of All was made to place the reply");
    page.setClock(page.clock() + CHANNEL_READ_TIMEOUT_MS);
    assert.ok(page.expireTimers(CHANNEL_READ_TIMEOUT_MS) >= 1, "nothing bounded the placing read");
    for (let i = 0; i < 3; i += 1) await page.settle();
    // The bridge edits its answer while that read is still out at the server. The edit says the service
    // is answering, and the read behind it asks All again beside the one given up on.
    const edited = { ...pushed, content: "the bridge's answer, edited" };
    server.edit("300", { content: edited.content });
    await deliver(page, page.stream(), sseUpdate("edit-300", edited));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(held.length, 2, "fixture: no second placing read went once the edit said the service is answering");
    // The first read's late answer lands now, replaced by the edit and the read after it.
    page.setClock(page.clock() + 10000);
    held[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, first };
  };
  const { page, first } = await run(SCRIPT);
  assert.ok(!shownIds(page).includes("300"), "the late answer something newer replaced left the reply on Main");
  const kept = savedMessage(page, "300");
  assert.ok(kept && kept.thread && kept.thread.id === first.id, `the reply is not held in its thread: ${JSON.stringify(kept)}`);

  // THE CONTROL, as round 2 did: a replaced late answer thrown away whole.
  const control = await run(brokenScript("            if (unused) placeFromAnswer(unused, read);\n", ""));
  assert.ok(shownIds(control.page).includes("300"), "the reply left Main anyway, so this test cannot see it");
});

for (const hidden of [false, true]) {
  test(`\`#308 live-reply-in-main\`: a replayed copy that places a message gives the store its thread record and nothing else${hidden ? ", the page hidden" : ""}`, async () => {
    const run = async (script) => {
      const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
      // The store's copy of 200 is the read's, edited since it was published.
      server.edit("200", { content: "main channel announcement, edited" });
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(savedMessage(page, "200").content, "main channel announcement, edited", "fixture: the edit was not read");
      if (hidden) await page.setVisibility("hidden");
      // The stream replays it as first published, with a thread record its reads do not carry: the root
      // of a thread with no reply yet, as an adapter may say of a message to the channel.
      const record = { id: "spaces/A/threads/solo", root_message_id: "200", is_root: true, reply_count: 0,
        reply_count_exact: true };
      const replayed = { ...page.messages.find((m) => m.id === "200"), content: "main channel announcement", thread: record };
      const from = page.timelineCalls.length;
      await deliver(page, page.stream(), sseMessage(replayed, { replayed: true }));
      for (let i = 0; i < 3; i += 1) await page.settle();
      const reads = page.timelineCalls.slice(from);
      if (hidden) {
        await page.setVisibility("visible");
        for (let i = 0; i < 3; i += 1) await page.settle();
      }
      return { page, reads, record };
    };
    const { page, reads, record } = await run(SCRIPT);
    assert.deepStrictEqual(reads, [], "a replayed copy was read for");
    const kept = savedMessage(page, "200");
    assert.equal(kept.content, "main channel announcement, edited", "the replayed copy replaced the store's newer one");
    assert.deepStrictEqual(kept.thread, record, "the replayed copy did not place the message");
    assert.ok(shownIds(page).includes("200"), "a root left Main");

    // THE CONTROL, as round 2 did: the replayed copy folded in whole.
    const control = await run(hidden
      ? brokenScript("        if (held) takeThreadRecord(message);\n        else foldLiveMessage(message, true, replayed);\n",
        "        foldLiveMessage(message, true, replayed);\n")
      : brokenScript("    } else if (places) {\n      takeThreadRecord(message);\n", "    } else if (places) {\n      foldLiveMessage(message, true, replayed);\n"));
    assert.equal(savedMessage(control.page, "200").content, "main channel announcement",
      "the store kept its own copy anyway, so this test cannot see it");
  });
}

// Review round 4. A newest page of All sent before the store gained a message by any way but the stream
// — the reader's own send, acknowledged; a message the stream missed, brought by Main's poll; a reply
// placed by a delta of All while the page was still on its way — took that message out of the store
// as deleted when it landed (`#310 stale-live-read-flicker`). The placing page is now folded only over
// the messages the store held when it was sent, and a read of All for Main's counts speaks only for
// those.

for (const gained of ["own send", "poll"]) {
  test(`\`#308 live-reply-in-main\`: a placing page of All does not take away ${gained === "own send" ? "the reader's own send, acknowledged" : "a message the stream missed, brought by Main's poll,"} while it was out`, async () => {
    const run = async (script) => {
      const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
      const serve = page.timeline;
      /** @type {Array<() => void>} */
      const held = [];
      page.timeline = (path, options) => {
        if (!/reason=place/.test(path)) return serve(path, options);
        const answer = serve(path, options);
        return new Promise((resolve) => held.push(() => resolve(answer)));
      };
      // All last read past its reconcile window: the placing read is All's newest page.
      page.setClock(page.clock() + TIMELINE_RECONCILE_MS + 60000);
      const from = page.timelineCalls.length;
      const { first, pushed } = bridgeReply(page, server);
      await deliver(page, page.stream(), sseMessage(pushed));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.deepStrictEqual(readsSince(page, from).filter(([, , reason]) => reason === "place"), [["flat", false, "place"]],
        "fixture: the placing read is not All's newest page");
      let id;
      if (gained === "own send") {
        // Acknowledged, and not yet echoed by the stream: nothing but the send has said it exists.
        const mine = message({ id: "9000000000000000077", author: "alice", content: "my own message",
          timestamp: new Date(page.clock()).toISOString() });
        page.replyResponse = async () => {
          server.post(mine);
          return json(200, { posted: mine });
        };
        await sendChannel(page, "my own message");
        id = mine.id;
      } else {
        // A message to the channel the stream never delivers: the poll's read of Main brings it.
        server.post(message({ id: "350", content: "a message the stream missed" }));
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
        id = "350";
      }
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.deepStrictEqual(shownIds(page), ["200", "201", "203", id], "fixture: the message gained is not on Main");
      held[0]();
      for (let i = 0; i < 3; i += 1) await page.settle();
      return { page, first, id, held };
    };
    const { page, first, id, held } = await run(SCRIPT);
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203", id],
      "the placing page took away what the store gained while it was out, or left the reply on Main");
    assert.ok(savedMessage(page, id), "the message gained is gone from the store");
    const kept = savedMessage(page, "300");
    assert.ok(kept && kept.thread && kept.thread.id === first.id, `the reply is not held in its thread: ${JSON.stringify(kept)}`);
    assert.equal(held.length, 1, "the placing read was asked again");

    // THE CONTROL, as round 3 did: folded over whatever the store gained since it was sent, the page
    // takes the message out as deleted.
    // The page's own fresh post is kept by `ownPostStands` too, so for it that goes as well.
    const broken = brokenScript(...FOLD_OVER_ANY_STORE);
    const control = await run(gained === "own send" ? brokenAlso(broken, OWN_POST_FORGOTTEN) : broken);
    assert.ok(!shownIds(control.page).includes(control.id) && savedMessage(control.page, control.id) === null,
      "the placing page left the message alone anyway, so this test cannot see it");
  });
}

test("`#308 live-reply-in-main`: a read of All for Main's counts sent between two replies the stream delivers without their thread keeps the second, which a placing delta placed before it landed", async () => {
  const run = async (script) => {
    const { page, server, held } = await heldAllReads(script);
    // Past the counts interval, and within All's reconcile window: each placing read is a delta of All.
    page.setClock(page.clock() + COUNTS_ALL_READ_MS + 1000);
    const from = page.timelineCalls.length;
    const { first, pushed } = bridgeReply(page, server, "300", 2);
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.deepStrictEqual([held.counts.length, held.place.length], [1, 1],
      `fixture: not one read for the counts and one placing read: ${JSON.stringify(readsSince(page, from))}`);
    // The first placing read lands and places the first reply. The read for the counts, sent after
    // that reply arrived, is still on its way.
    page.setClock(page.clock() + 1000);
    held.place[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    // The bridge's second reply, two seconds later, placed by a delta of its own.
    page.setClock(page.clock() + 2000);
    const second = bridgeReply(page, server, "301", 3);
    await deliver(page, page.stream(), sseMessage(second.pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(held.place.length, 2, "fixture: no read of All was made to place the second reply");
    page.setClock(page.clock() + 1000);
    held.place[1]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.deepStrictEqual(readsSince(page, from).filter(([, , reason]) => reason === "place"),
      [["flat", true, "place"], ["flat", true, "place"]], "fixture: the placing reads are not deltas of All");
    const placedSecond = savedMessage(page, "301");
    assert.ok(placedSecond && placedSecond.thread && placedSecond.thread.id === first.id,
      "fixture: the second reply was not placed before the read for the counts landed");
    // Now the read for the counts lands.
    page.setClock(page.clock() + 5000);
    held.counts[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, first };
  };
  const { page, first } = await run(SCRIPT);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "a reply was left on Main");
  assert.ok(savedMessage(page, "301"), "the read for the counts took the second reply out of the store");
  assert.deepStrictEqual(await threadFromStore(page, first.id), ["201", "202", "300", "301"], "a reply is not in its thread");

  // THE CONTROL, as round 3 did: the read for the counts speaking for every message held when it lands
  // takes the second reply out as deleted.
  const control = await run(brokenScript(...COUNTS_SPEAK_FOR_ALL));
  assert.equal(savedMessage(control.page, "301"), null, "the read for the counts kept the second reply anyway, so this test cannot see it");
  assert.ok(!(await threadFromStore(control.page, control.first.id)).includes("301"),
    "the second reply reached its thread anyway, so this test cannot see it");
});

// Review round 5. A reply removed upstream while the placing read was out came back from that read's
// answer, in its thread and counted as unread, and stayed until the thread was read: the removal left
// it unplaced, and the answer, older than the removal, put it back or placed it where Main's read
// behind the removal no longer looks. A removal now says the message is gone (`noteLiveRemoval`).

for (const placing of ["newest", "delta"]) {
  for (const landsFirst of ["removal", "placing"]) {
    test(`\`#308 live-reply-in-main\`: a reply removed upstream while the placing ${placing === "newest" ? "page" : "delta"} is out does not come back from its answer (${landsFirst === "removal" ? "Main's read behind the removal lands first" : "the placing read lands first"})`, async () => {
      const run = async (script) => {
        const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
        if (placing === "delta") {
          // All read once, and recently: the placing read asks what changed in All since.
          await pickThread(page, "flat");
          for (let i = 0; i < 3; i += 1) await page.settle();
          await pickThread(page, "main");
          for (let i = 0; i < 3; i += 1) await page.settle();
        } else {
          // All never read, and Main's last read past its reconcile window.
          page.setClock(page.clock() + TIMELINE_RECONCILE_MS + 60000);
        }
        const serve = page.timeline;
        /** @type {{place: Array<() => void>, mutation: Array<() => void>}} */
        const held = { place: [], mutation: [] };
        let holdRemovalRead = landsFirst === "placing";
        page.timeline = (path, options) => {
          const reason = new URL(path, "http://fixture.test").searchParams.get("reason");
          if (reason !== "place" && !(reason === "mutation" && holdRemovalRead)) return serve(path, options);
          // Answered as the server had it when asked, and handed over later.
          const answer = Promise.resolve(serve(path, options)).then(async (response) => {
            const body = await response.text();
            return { ...response, text: async () => body };
          });
          return new Promise((resolve) => held[reason].push(() => resolve(answer)));
        };
        const from = page.timelineCalls.length;
        const { first, pushed } = bridgeReply(page, server);
        await deliver(page, page.stream(), sseMessage(pushed));
        for (let i = 0; i < 3; i += 1) await page.settle();
        assert.equal(held.place.length, 1, "fixture: no read of All was made to place the reply");
        // The bridge takes its reply back. Main's read behind the removal is its newest page, the server
        // unable to continue Main: it cannot carry a thread reply, so it cannot say the reply is gone.
        server.remove("300");
        server.edit("201", { thread: { ...first, reply_count: 1 } });
        server.refuse = { status: 410, error: "cursor_expired" };
        await deliver(page, page.stream(), sseDelete("del-300", CHANNEL.id, "300"));
        for (let i = 0; i < 4; i += 1) await page.settle();
        held.place[0]();
        for (let i = 0; i < 4; i += 1) await page.settle();
        holdRemovalRead = false;
        for (const answer of held.mutation.splice(0)) answer();
        for (let i = 0; i < 4; i += 1) await page.settle();
        const reads = readsSince(page, from);
        assert.deepStrictEqual(reads.filter(([, , reason]) => reason === "place"), [["flat", placing === "delta", "place"]],
          `fixture: not one placing read of the kind meant: ${JSON.stringify(reads)}`);
        assert.ok(reads.some(([view, delta, reason]) => view === "main" && !delta && reason === "mutation"),
          `fixture: Main's read behind the removal was not its newest page: ${JSON.stringify(reads)}`);
        // And the poll after that.
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
        for (let i = 0; i < 3; i += 1) await page.settle();
        return { page, first };
      };
      const { page, first } = await run(SCRIPT);
      assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the reply removed upstream is on Main");
      assert.equal(savedMessage(page, "300"), null, "the reply removed upstream came back to the store");
      assert.deepStrictEqual(savedScopes(page)[scopeKey(CHANNEL.id)].unplaced, [], "the reply removed upstream is still unplaced");
      assert.ok(!(await threadFromStore(page, first.id)).includes("300"), "the reply removed upstream came back to its thread");

      // THE CONTROLS. A removal that leaves the reply unplaced, as round 4 did: the placing read's answer
      // puts it back in its thread, or places it there, where Main's read no longer takes it out.
      /** Whether the store holds the removed reply in its thread, as the control's answer put it there. */
      const inThread = (control) => {
        const kept = savedMessage(control.page, "300");
        return Boolean(kept && kept.thread && kept.thread.id === control.first.id);
      };
      const unplaced = await run(brokenScript("  if (channelStore.current().channel !== String(channel)) return;\n  noteRemoved(channelStore.current(), String(id), observedMs);\n",
        "  if (channelStore.current().channel !== String(channel)) return;\n"));
      assert.ok(inThread(unplaced), "the reply stayed gone anyway, so this test cannot see it");
      if (placing === "delta") {
        // And a placing delta folded whatever the stream removed since it was sent, its copy of the reply
        // taken though the reply was removed after it was sent (`#316 late-counts-resurrect`), brings the
        // reply back itself: landing first, as it is; landing after Main's read took the reply out, only
        // if also folded whatever the store has lost since (`heldAsSent`).
        const unremoved = ["  const unremoved = liveEvents.removals(channel) === read.removals;\n",
          "  const unremoved = true;\n"];
        const folded = await run(await brokenModule([REMOVALS_FORGOTTEN], landsFirst === "placing"
          ? brokenScriptEach(unremoved) : brokenScriptEach(unremoved, FOLD_OVER_ANY_STORE)));
        assert.ok(inThread(folded), "the placing delta left the reply gone anyway, so this test cannot see it");
      }
    });
  }
}

// And a placing read sent before an edit that Main's read brought while it was out was folded over the
// store's newer copy, and undid the edit until the next read to carry the message: never, for a
// newest page of Main, once that message has been placed in a thread. A placing read is now folded
// only over the store's copies as they were when it was sent (`heldAsSent`).

for (const placing of ["newest", "delta"]) {
  test(`\`#308 live-reply-in-main\`: a placing ${placing === "newest" ? "page" : "delta"} of All does not undo an edit Main's poll brought while it was out`, async () => {
    const edited = "first thread root, edited while the placing read was out";
    const run = async (script) => {
      const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
      if (placing === "delta") {
        // All read once, and recently: the placing read asks what changed in All since.
        await pickThread(page, "flat");
        for (let i = 0; i < 3; i += 1) await page.settle();
        await pickThread(page, "main");
        for (let i = 0; i < 3; i += 1) await page.settle();
      } else {
        page.setClock(page.clock() + TIMELINE_RECONCILE_MS + 60000);
      }
      const serve = page.timeline;
      /** @type {Array<() => void>} */
      const held = [];
      page.timeline = (path, options) => {
        if (!/reason=place/.test(path)) return serve(path, options);
        // Answered as the server had it when asked: `server.edit` changes the messages an answer not yet
        // read out would carry.
        const answer = Promise.resolve(serve(path, options)).then(async (response) => {
          const body = await response.text();
          return { ...response, text: async () => body };
        });
        return new Promise((resolve) => held.push(() => resolve(answer)));
      };
      const from = page.timelineCalls.length;
      // The bridge's reply moves its root's count on, so a delta of All carries the root too.
      const { first, pushed } = bridgeReply(page, server);
      await deliver(page, page.stream(), sseMessage(pushed));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.deepStrictEqual(readsSince(page, from).filter(([, , reason]) => reason === "place"), [["flat", placing === "delta", "place"]],
        `fixture: not one placing read of the kind meant: ${JSON.stringify(readsSince(page, from))}`);
      // The root edited upstream once the placing read was taken, and brought by Main's poll: the stream
      // says nothing of it.
      server.edit("201", { content: edited });
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(savedMessage(page, "201").content, edited, "fixture: Main's poll did not bring the edit");
      held[0]();
      for (let i = 0; i < 3; i += 1) await page.settle();
      // And two polls after, which bring nothing more of the root.
      for (let poll = 0; poll < 2; poll += 1) {
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
        for (let i = 0; i < 3; i += 1) await page.settle();
      }
      return { page, first, held };
    };
    const { page, first, held } = await run(SCRIPT);
    assert.equal(savedMessage(page, "201").content, edited, "the placing read undid an edit made after it was sent");
    assert.ok(bodyOf(rowOf(page, "201")).text().includes(edited), "Main shows the root as it was before the edit");
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the reply was left on Main");
    const kept = savedMessage(page, "300");
    assert.ok(kept && kept.thread && kept.thread.id === first.id, `the reply is not held in its thread: ${JSON.stringify(kept)}`);
    assert.equal(held.length, 1, "the placing read was asked again");

    // THE CONTROL, as round 4 did: folded whatever the store has replaced since it was sent, the
    // placing read puts back the root as it was before the edit.
    const control = await run(brokenScript(...FOLD_OVER_ANY_STORE));
    assert.equal(savedMessage(control.page, "201").content, "first thread root", "the edit survived anyway, so this test cannot see it");
  });
}

// --- `#208 persistent-cache`: saved messages on this server --------------------------------------
//
// The server's own copy of the messages it reads, which the owner may keep, turn off, or erase from
// Settings. Its status comes from client-config, only from a server whose configuration has the
// table and only to a write token; both erasing acts ask before anything is sent.

/** Every id inside the Saved messages group, as web/voice.html must declare them. */
const MESSAGE_CACHE_IDS = [
  "message-cache-enabled", "message-cache-state", "message-cache-confirm", "message-cache-confirm-text",
  "message-cache-confirm-yes", "message-cache-confirm-no", "message-cache-erase", "help-link-message-cache",
];

/** A signed-in page whose server reports `status` (none for null) to a token of `scope`. */
async function messageCachePage(status = messageCacheStatus(), scope = "write") {
  const page = newPage();
  page.messageCache = status === null ? undefined : status;
  page.tokenScope = scope;
  await signIn(page);
  return page;
}

/** The requests the page made to the switch route, whatever it answered. */
const messageCacheRequests = (page) => page.requests.filter((r) => r.includes("/api/v1/message-cache"));

test("`#208 persistent-cache`: the Saved messages group and its help entry show only to a write token, and only with a status", async () => {
  const none = await messageCachePage(null);
  assert.equal(none.el("message-cache-group").hidden, true, "a server with no message cache shows the group");
  assert.equal(none.el("help-message-cache").hidden, true, "Help describes a cache this server does not have");

  // A read token is shown nothing, even should a status reach it: it cannot switch the cache.
  const read = await messageCachePage(messageCacheStatus(), "read");
  assert.equal(read.el("message-cache-group").hidden, true, "a read token was shown the switch");
  assert.equal(read.el("help-message-cache").hidden, true);

  const write = await messageCachePage();
  assert.equal(write.el("message-cache-group").hidden, false, "the write token was not shown the group");
  assert.equal(write.el("help-message-cache").hidden, false, "the group showed without its help entry");
  assert.equal(write.el("message-cache-enabled").checked, true);
  assert.equal(write.el("message-cache-enabled").disabled, false);
  assert.equal(write.el("message-cache-erase").disabled, false);
  assert.equal(write.el("message-cache-confirm").hidden, true, "a question was asked before anything was pressed");
  // Drawing it asks the server nothing: the status came with client-config.
  for (const page of [none, read, write]) assert.deepStrictEqual(messageCacheRequests(page), []);
});

test("`#208 persistent-cache`: the line under the switch says what the server holds, or why it holds nothing", async () => {
  const LIMITS = "It keeps at most 14 days and 256 MB.";
  for (const [status, line, ticked, locked] of [
    [messageCacheStatus(),
      `On: 1,200 messages from 3 channels, 3.4 MB. Complete in places back to Aug 10, Mon. ${LIMITS}`, true, false],
    [messageCacheStatus({ messages: 1, channels: 1, bytes: 2048, oldest_covered: undefined, max_days: 1, max_megabytes: 16 }),
      "On: 1 message from 1 channel, 2 KB. It keeps at most 1 day and 16 MB.", true, false],
    [messageCacheStatus({ messages: 0, channels: 0, bytes: 32768, oldest_covered: undefined }),
      `On, and nothing is saved yet. ${LIMITS}`, true, false],
    [messageCacheStatus({ available: false, enabled: false, reason: "switched_off", messages: 0, channels: 0, bytes: 0 }),
      "Off: you turned it off, and its file was erased. Turn it on to start again with an empty file.", false, false],
    [messageCacheStatus({ available: false, enabled: false, reason: "config_disabled", messages: 0, channels: 0, bytes: 0 }),
      "Off in this server's configuration; only its operator can turn it on.", false, true],
    [messageCacheStatus({ available: false, reason: "locked", messages: 0, channels: 0, bytes: 0 }),
      "On, but no file is open, so nothing is being saved: another process holds the file.", true, false],
    // A code this page does not know reads as "not open", as the contract asks.
    [messageCacheStatus({ available: false, reason: "a_code_from_later", messages: 0, channels: 0, bytes: 0 }),
      "On, but no file is open, so nothing is being saved.", true, false],
  ]) {
    const page = await messageCachePage(status);
    assert.equal(page.el("message-cache-state").textContent, line);
    assert.equal(page.el("message-cache-enabled").checked, ticked, line);
    assert.equal(page.el("message-cache-enabled").disabled, locked, line);
    // Only an open file can be erased: with none, Erase would erase nothing.
    assert.equal(page.el("message-cache-erase").disabled, !status.available, line);
  }
  // The help entry's limits are this server's, not the figures in the markup.
  const page = await messageCachePage(messageCacheStatus({ max_days: 30, max_megabytes: 512 }));
  assert.equal(page.el("message-cache-help-days").textContent, "30 days");
  assert.equal(page.el("message-cache-help-megabytes").textContent, "512 MB");
});

test("`#595 cache-full-history`: the line says how many channels the server holds whole and how far back it has reached, and the search says when the server holds every channel whole", async () => {
  const full = (history) => messageCacheStatus({
    max_days: 65535,
    history: { mode: "full", channels: 3, complete: 1, ...history },
  });
  const held = "On: 1,200 messages from 3 channels, 3.4 MB. Complete in places back to Aug 10, Mon.";
  let page = await messageCachePage(full({ reached: "2025-03-02T10:00:00Z" }));
  assert.match(page.el("message-cache-state").textContent, new RegExp(`^${held.replace(/[.]/g, "\\.")} ` +
    "Complete history of 1 of 3 channels\\. Gathering older messages, back to .+ so far\\. " +
    "It keeps every day, up to 256 MB\\.$"));
  assert.equal(page.el("message-cache-help-days").textContent, "their whole history");
  page = await messageCachePage(full({ complete: 3, paused: "storage" }));
  assert.equal(page.el("message-cache-state").textContent, `${held} Complete history of every channel. ` +
    "Gathering waits: the size limit is nearly reached. It keeps every day, up to 256 MB.");
  // The recent days alone say nothing new.
  page = await messageCachePage(messageCacheStatus({
    history: { mode: "recent", channels: 3, complete: 0 },
  }));
  assert.equal(page.el("message-cache-state").textContent, `${held} It keeps at most 14 days and 256 MB.`);

  // A search the browser's messages do not answer says the server holds the channel whole: every
  // channel it caches is, and the status names none.
  for (const [complete, tail] of [
    [3, " This server holds the complete history of this channel, which this search does not reach yet."],
    [2, ""],
  ]) {
    const searching = newPage();
    searching.messageCache = full({ complete });
    const all = searchHistory(searching);
    await signIn(searching);
    await showDiscord(searching, searching.messages);
    await search(searching, "engine");
    await searching.settle();
    const day = searchMoment(searching, all[20].timestamp);
    assert.equal(searching.el("search-empty").textContent,
      `Searched 4 messages in this browser, a complete history back to ${day} (no gaps). None match.${tail}`);
  }
});

test("`#208 persistent-cache`: turning it off sends PUT only once the question is answered yes, and nothing on Keep", async () => {
  const page = await messageCachePage();
  await page.el("message-cache-enabled").setChecked(false);
  await page.settle();
  assert.deepStrictEqual(page.messageCacheCalls, [], "unticking erased the file without asking");
  assert.equal(page.el("message-cache-confirm").hidden, false, "nothing asked before the file is erased");
  assert.equal(page.el("message-cache-confirm-text").textContent,
    "Turn saving off and erase the 1,200 saved messages (3.4 MB) now?");
  assert.equal(page.el("message-cache-confirm-yes").textContent, "Turn off and erase");
  assert.equal(page.el("message-cache-confirm-no").textContent, "Keep saving");
  assert.equal(page.el("message-cache-enabled").checked, true, "the box says off while the server still saves");
  assert.equal(focusName(page), "#message-cache-confirm-no", "the answer waits anywhere but at the safe button");

  await page.el("message-cache-confirm-no").click();
  await page.settle();
  assert.deepStrictEqual(page.messageCacheCalls, [], "Keep saving sent something");
  assert.deepStrictEqual(messageCacheRequests(page), []);
  assert.equal(page.el("message-cache-confirm").hidden, true);
  assert.equal(page.el("message-cache-enabled").checked, true);
  assert.equal(focusName(page), "#message-cache-enabled", "Keep did not go back to the switch");

  await page.el("message-cache-enabled").setChecked(false);
  await page.el("message-cache-confirm-yes").click();
  await page.settle();
  assert.deepStrictEqual(page.messageCacheCalls, ['PUT {"enabled":false}']);
  assert.equal(page.el("message-cache-confirm").hidden, true);
  assert.equal(page.el("message-cache-enabled").checked, false, "the box was not redrawn from what the server left");
  assert.equal(page.el("message-cache-erase").disabled, true, "Erase is offered with no file to erase");
  assert.match(page.el("message-cache-state").textContent, /^Off: you turned it off, and its file was erased\./);
});

test("`#208 persistent-cache`: turning it on sends at once, and a refused switch keeps what the server last said, and says why", async () => {
  const page = await messageCachePage(messageCacheStatus({
    available: false, enabled: false, reason: "switched_off", messages: 0, channels: 0, bytes: 0, oldest_covered: undefined,
  }));
  await page.el("message-cache-enabled").setChecked(true);
  await page.settle();
  assert.deepStrictEqual(page.messageCacheCalls, ['PUT {"enabled":true}'], "turning it on asked, or sent nothing");
  assert.equal(page.el("message-cache-confirm").hidden, true);
  assert.equal(page.el("message-cache-enabled").checked, true);
  assert.equal(page.el("message-cache-state").textContent, "On, and nothing is saved yet. It keeps at most 14 days and 256 MB.");

  // Off, refused: the marker could not be written, so nothing was erased, and the box stays on.
  page.messageCacheFailure = {
    status: 500, error: "message_cache_switch_unsaved",
    detail: "the message cache's switch could not be saved beside the store; nothing was erased and nothing changed",
  };
  await page.el("message-cache-enabled").setChecked(false);
  await page.el("message-cache-confirm-yes").click();
  await page.settle();
  assert.equal(page.el("message-cache-enabled").checked, true, "a refused off left the box saying off");
  assert.match(page.el("message-cache-state").textContent,
    /^The server did not confirm that: the message cache's switch could not be saved beside the store; nothing was erased and nothing changed On, and nothing is saved yet\./);

  // On, refused: the operator's configuration wins, and the box goes back to off.
  const off = await messageCachePage(messageCacheStatus({
    available: false, enabled: false, reason: "switched_off", messages: 0, channels: 0, bytes: 0,
  }));
  off.messageCacheFailure = { status: 409, error: "message_cache_not_enabled", detail: "only the operator can turn it on" };
  await off.el("message-cache-enabled").setChecked(true);
  await off.settle();
  assert.equal(off.el("message-cache-enabled").checked, false, "a refused on left the box saying on");
  assert.match(off.el("message-cache-state").textContent, /^The server did not confirm that: only the operator can turn it on Off:/);
});

test("`#208 persistent-cache`: Erase asks first, then sends DELETE, and the cache stays on", async () => {
  const page = await messageCachePage();
  await page.el("message-cache-erase").click();
  await page.settle();
  assert.deepStrictEqual(page.messageCacheCalls, [], "Erase erased without asking");
  assert.equal(page.el("message-cache-confirm").hidden, false);
  assert.equal(page.el("message-cache-confirm-text").textContent,
    "Erase the 1,200 saved messages (3.4 MB) now? Saving carries on in an empty file.");
  assert.equal(page.el("message-cache-confirm-yes").textContent, "Erase now");
  assert.equal(page.el("message-cache-confirm-no").textContent, "Keep them");
  assert.equal(page.el("message-cache-erase").hidden, true, "a third button stands beside the question's two");

  await page.el("message-cache-confirm-no").click();
  await page.settle();
  assert.deepStrictEqual(page.messageCacheCalls, [], "Keep them sent something");
  assert.equal(page.el("message-cache-erase").hidden, false, "Erase did not come back once the question was answered");
  assert.equal(focusName(page), "#message-cache-erase", "Keep did not go back to Erase");

  await page.el("message-cache-erase").click();
  await page.el("message-cache-confirm-yes").click();
  await page.settle();
  assert.deepStrictEqual(page.messageCacheCalls, ["DELETE"]);
  assert.equal(page.el("message-cache-enabled").checked, true, "erasing turned the cache off");
  assert.equal(page.el("message-cache-state").textContent,
    "Erased. On, and nothing is saved yet. It keeps at most 14 days and 256 MB.");
});

test("`#208 persistent-cache`: the ? opens the help entry about saved messages, and the entry says what M5 really does", async () => {
  const page = await messageCachePage();
  await page.el("open-settings").click();
  await page.el("help-link-message-cache").click();
  assert.equal(page.screen(), "help");
  assert.deepStrictEqual(openHelp(page), ["message-cache"], "the ? opened some other entry");
  assert.equal(page.el("help-message-cache").hidden, false);
  assert.match(settingsGroup("Saved messages on this server"), /data-help="message-cache"/);
  const entry = helpEntry("message-cache").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");
  assert.match(entry, /kept unencrypted in a file only this account can read/);
  assert.match(entry, /Slack included, the next time the server reads that part of the channel/);
  assert.match(entry, /re-checked for a week is erased anyway/);
  assert.match(entry, /nothing it shows you is read from it yet/);
  // No refresher re-reads Slack yet (#208's M7), so the help must not promise one.
  assert.doesNotMatch(entry, /next refresh|two hours at most/, "the help promises a refresher this server does not run");
});

test("`#208 persistent-cache`: every id the group uses is declared in the markup, in the Storage section, served hidden", () => {
  for (const id of ["message-cache-group", ...MESSAGE_CACHE_IDS, "help-message-cache", "message-cache-help-days",
    "message-cache-help-megabytes"]) {
    assert.ok(PAGE_IDS.has(id), `web/voice.html does not declare #${id}`);
  }
  for (const id of MESSAGE_CACHE_IDS) assertMarkupContains("message-cache-group", id);
  for (const id of ["message-cache-help-days", "message-cache-help-megabytes"]) assertMarkupContains("help-message-cache", id);
  assertMarkupContains("settings-storage", "message-cache-group");
  assert.deepStrictEqual(groupsIn("Storage and connection").slice(0, 2), ["Stored conversations", "Saved messages on this server"],
    "the group is not straight after Stored conversations");
  assert.equal(PAGE_ELEMENTS.get("message-cache-group").hidden, true, "the group is served showing");
  assert.equal(PAGE_ELEMENTS.get("help-message-cache").hidden, true, "the help entry is served showing");
  assert.equal(PAGE_ELEMENTS.get("message-cache-confirm").hidden, true, "the question is served asked");
  assert.equal(PAGE_ELEMENTS.get("message-cache-enabled").type, "checkbox");
  for (const id of ["message-cache-erase", "message-cache-confirm-yes", "message-cache-confirm-no"]) {
    assert.equal(PAGE_ELEMENTS.get(id).tag, "button", `#${id} is not a button`);
  }
  assert.match(HTML, /<p id="message-cache-state" class="hint" aria-live="polite"><\/p>/, "the line is not announced");
});

test("`#208 persistent-cache`: a help entry hidden with its feature never holds the top button at Expand all", async () => {
  // A server with no message cache hides that entry. The reader opens every entry they can see,
  // one by one: the button must offer to fold them, and folding must fold them, not open the
  // hidden one.
  const page = await messageCachePage(null);
  await page.el("open-settings").click();
  await page.el("open-help").click();
  const shown = HELP_SLUGS.filter((slug) => !page.el(`help-${slug}`).hidden);
  assert.ok(!shown.includes("message-cache"));
  for (const slug of shown) {
    page.el(`help-${slug}`).open = true;
    await page.el(`help-${slug}`).dispatch("toggle");
  }
  assert.equal(page.el("fold-help").textContent, "Fold all", "a hidden entry kept the button at Expand all");
  await page.el("fold-help").click();
  assert.deepStrictEqual(openHelp(page), [], "Fold all opened the hidden entry instead of folding the rest");
  assert.equal(page.el("fold-help").textContent, "Expand all");

  // With a message cache the entry is on the screen, and Expand all opens it with the rest.
  const cached = await messageCachePage();
  await cached.el("open-settings").click();
  await cached.el("open-help").click();
  await cached.el("fold-help").click();
  assert.deepStrictEqual(openHelp(cached), HELP_SLUGS);
});

// --- `#316 late-counts-resurrect`: a late read of All never brings back a removal or undoes an edit -----
//
// The read of All the page makes for Main's counts can be sent before an upstream removal or edit and
// land after Main's own read has brought it. It was folded over the store whatever the store had lost
// or replaced since, and brought the removed message back onto Main, or put back the text from before
// the edit, until the next full read. Now a late page of All is folded only over the store's copies
// older than it (`pageFold`, `copyOlderThan`; `#310 stale-live-read-flicker` put them in the order reads
// were sent), and never takes a message removed upstream after it was sent (`removedSince`). And a
// placing page that is not folded gives a message the stream left unplaced its whole copy where the
// store's is older: a bridge that streams its answer by editing it is placed with the text it had
// reached, not the text it was first pushed with.

/** The read of All for Main's counts sent, and held (`heldAllReads`), past the interval: a quiet reply outruns the store. */
async function countsReadOut(script) {
  const { page, server, held, quietReply } = await heldAllReads(script);
  page.setClock(page.clock() + COUNTS_ALL_READ_MS + 1000);
  quietReply();
  await pollOnce(page);
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(held.counts.length, 1, "fixture: All was not read for Main's counts");
  return { page, server, held };
}

for (const via of ["stream", "poll"]) {
  test(`\`#316 late-counts-resurrect\`: a read of All for Main's counts sent before a message is removed upstream, landing after Main's read took it out, does not bring it back (${via === "stream" ? "the stream says so" : "only Main's newest page says so"})`, async () => {
    const run = async (script) => {
      const { page, server, held } = await countsReadOut(script);
      server.remove("200");
      if (via === "stream") {
        // The stream says so, and Main's read behind the removal is what changed in it, which says so too.
        await deliver(page, page.stream(), sseDelete("del-200", CHANNEL.id, "200"));
      } else {
        // A provider that pushes no removals: Main's poll, its newest page, no longer carries it.
        server.refuse = { status: 410, error: "cursor_expired" };
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
      }
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.deepStrictEqual(shownIds(page), ["201", "203"], "fixture: Main's read did not take the message out");
      held.counts[0]();
      for (let i = 0; i < 3; i += 1) await page.settle();
      const landed = { shown: shownIds(page), saved: savedMessage(page, "200") };
      // And the poll after that, which brings nothing of it.
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      return { page, landed };
    };
    const { page, landed } = await run(SCRIPT);
    assert.deepStrictEqual(landed.shown, ["201", "203"], "the read for the counts brought the removed message back onto Main");
    assert.equal(landed.saved, null, "the read for the counts brought the removed message back into the store");
    assert.deepStrictEqual(shownIds(page), ["201", "203"], "the removed message came back by the next poll");
    assert.equal(savedMessage(page, "250").content, "a quiet reply in the second discussion", "the read for the counts brought nothing");

    // THE CONTROL, as #308 left it: folded whatever the store has lost since it was sent, and whatever was
    // removed after, the read for the counts puts the message back, and the poll leaves it there.
    const control = await run(await brokenModule([REMOVALS_FORGOTTEN], brokenScriptEach(COUNTS_SPEAK_FOR_ALL)));
    assert.deepStrictEqual(control.landed.shown, ["200", "201", "203"],
      "the read for the counts left the message out anyway, so this test cannot see it");
    assert.deepStrictEqual(shownIds(control.page), ["200", "201", "203"]);
  });
}

test("`#316 late-counts-resurrect`: a read of All for Main's counts does not bring back a reply the stream delivered after it was sent and then took back", async () => {
  const run = async (script) => {
    const { page, server, held } = await heldAllReads(script);
    page.setClock(page.clock() + COUNTS_ALL_READ_MS + 1000);
    // The bridge answers in the first discussion: Main's poll finds the root's count past what the store
    // holds, and reads All behind it, which carries the reply. The stream delivers it only after that.
    const { first } = bridgeReply(page, server);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(held.counts.length, 1, "fixture: All was not read for Main's counts");
    const placed = page.messages.find((m) => m.id === "300");
    await deliver(page, page.stream(), sseMessage(placed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(savedMessage(page, "300"), "fixture: the store did not gain the reply");
    // The bridge takes it back, and Main's read behind the removal says so.
    server.remove("300");
    server.edit("201", { thread: { ...first, reply_count: 1 } });
    await deliver(page, page.stream(), sseDelete("del-300", CHANNEL.id, "300"));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(savedMessage(page, "300"), null, "fixture: Main's read behind the removal did not take the reply out");
    held.counts[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, first };
  };
  const { page, first } = await run(SCRIPT);
  assert.equal(savedMessage(page, "300"), null, "the read for the counts brought the reply back into the store");
  assert.ok(!(await threadFromStore(page, first.id)).includes("300"), "the read for the counts brought the reply back to its thread");

  // THE CONTROL: the store never held the reply as the read was sent, nor holds it now, so only the
  // removal says the read is older than it; forgotten, the read puts the reply back in its thread.
  const control = await run(await brokenModule([REMOVALS_FORGOTTEN]));
  assert.ok(savedMessage(control.page, "300"), "the reply stayed gone anyway, so this test cannot see it");
  assert.ok((await threadFromStore(control.page, control.first.id)).includes("300"));
});

test("`#316 late-counts-resurrect`: a read of All for Main's counts sent before an edit that Main's poll brought does not undo it", async () => {
  const edited = "main channel announcement, edited while the read for the counts was out";
  const run = async (script) => {
    const { page, server, held } = await countsReadOut(script);
    // A copy of its own, so the read for the counts already taken keeps the text it was taken with.
    const at = page.messages.findIndex((m) => m.id === "200");
    page.messages[at] = { ...page.messages[at] };
    server.edit("200", { content: edited });
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(savedMessage(page, "200").content, edited, "fixture: Main's poll did not bring the edit");
    held.counts[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    const landed = { saved: savedMessage(page, "200").content, row: bodyOf(rowOf(page, "200")).text() };
    // And two polls after, which bring nothing more of it.
    for (let poll = 0; poll < 2; poll += 1) {
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
    }
    return { page, landed };
  };
  const { page, landed } = await run(SCRIPT);
  assert.equal(landed.saved, edited, "the read for the counts put back the text from before the edit");
  assert.ok(landed.row.includes(edited), "Main shows the message as it was before the edit");
  assert.equal(savedMessage(page, "200").content, edited, "the edit was undone by the polls after");
  assert.equal(savedMessage(page, "250").content, "a quiet reply in the second discussion", "the read for the counts brought nothing");

  // THE CONTROL, as #308 left it: folded whatever the store has replaced since it was sent, the read for
  // the counts undoes the edit, and the polls after bring nothing to correct it.
  const control = await run(brokenScript(...COUNTS_SPEAK_FOR_ALL));
  assert.equal(control.landed.saved, "main channel announcement", "the edit survived anyway, so this test cannot see it");
  assert.equal(savedMessage(control.page, "200").content, "main channel announcement");
});

for (const main of ["delta", "newest"]) {
  test(`\`#316 late-counts-resurrect\`: a bridge that streams its answer by editing it while the placing page is out is placed with the text it had reached (Main read by ${main === "delta" ? "what changed" : "its newest page"})`, async () => {
    const finished = "the bridge's answer, finished while the placing page was out";
    const run = async (script) => {
      const { page, server } = await forwardPage({}, (p) => { p.servePlacing = true; }, script);
      const serve = page.timeline;
      /** @type {Array<() => void>} */
      const held = [];
      page.timeline = (path, options) => {
        if (!/reason=place/.test(path)) return serve(path, options);
        // A slow bridge: the read is taken when the answer goes out, after the edits.
        return new Promise((resolve) => held.push(() => resolve(serve(path, options))));
      };
      const { first, pushed } = bridgeReply(page, server);
      await deliver(page, page.stream(), sseMessage(pushed));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(held.length, 1, "fixture: no read of All was made to place the reply");
      // The bridge edits its answer, twice; the stream's copies, like its first, carry no thread record.
      if (main === "newest") server.refuse = { status: 410, error: "cursor_expired" };
      for (const content of ["the bridge's answer, half done", finished]) {
        server.edit("300", { content });
        await deliver(page, page.stream(), sseUpdate(`upd-${content.length}`, { ...pushed, content }));
        for (let i = 0; i < 3; i += 1) await page.settle();
        page.expireTimers(50);
        for (let i = 0; i < 3; i += 1) await page.settle();
      }
      held[0]();
      for (let i = 0; i < 4; i += 1) await page.settle();
      return { page, first };
    };
    const { page, first } = await run(SCRIPT);
    const kept = savedMessage(page, "300");
    assert.ok(kept && kept.thread && kept.thread.id === first.id, `the reply is not placed in its thread: ${JSON.stringify(kept)}`);
    assert.equal(kept.content, finished, "the reply was placed with the text it was first pushed with");
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the reply was left on Main");
    await pickThread(page, `thread:${first.id}`);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(bodyOf(rowOf(page, "300")).text().includes(finished), "its thread shows the reply as first pushed");

    // THE CONTROL, as #308 left it: the store's copy takes only the answer's thread record. Where Main's
    // newest page took the bare copy out, the answer puts it back whole either way.
    const control = await run(await brokenModule([{ file: "src/core/channels/placement.ts",
      find: "    if (held && ledger.copyOlderThan(stored, id, number, held, true)) {\n", replace: "    if (false) {\n" }]));
    const controlled = savedMessage(control.page, "300");
    if (main === "delta") {
      assert.equal(controlled.content, "the bridge's answer, in the first discussion",
        "the reply had the finished text anyway, so this test cannot see it");
    } else {
      assert.equal(controlled.content, finished, "fixture: Main's newest page did not take the bare copy out");
    }
  });
}

// A reply's own read of All, which places the message it answers (`placeReplyTarget`), gives each message
// the stream left unplaced the copy it found, standing where that read was sent (`foldLiveMessage`'s
// `read`). Taken for the stream's copy instead, which a page that carries the message takes its own copy
// over, it let the read of All for Main's counts, sent before and landing after, put back the text a
// bridge's reply was pushed with over the finished answer the reply's read had brought.

test("`#316 late-counts-resurrect`: a read of All for Main's counts does not undo the finished answer a reply's own read of All brought for the message it answers", async () => {
  const pushedText = "the bridge's answer, in the first discussion";
  const finished = "the bridge's answer, finished before the reader answered it";
  const run = async (script) => {
    const { page, server, held } = await heldAllReads(script);
    page.setClock(page.clock() + COUNTS_ALL_READ_MS + 1000);
    // The bridge answers in the first discussion: Main's poll finds the root's count past the store's,
    // and reads All behind it for the counts, which is answered with the reply as it was pushed.
    const { first, pushed } = bridgeReply(page, server);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(held.counts.length, 1, "fixture: All was not read for Main's counts");
    // The stream delivers the reply without its thread, so it is left unplaced, on Main.
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(rowOf(page, "300"), "fixture: the bare reply is not on Main");
    // The bridge finishes its answer, on a copy of its own, so the read for the counts keeps the pushed text.
    const at = page.messages.findIndex((m) => m.id === "300");
    page.messages[at] = { ...page.messages[at] };
    server.edit("300", { content: finished });
    // The reader answers it, and the reply's own read of All finds the finished text and the thread.
    distinctReplies(page);
    await replyQuietly(page, "300", "an answer to the bridge");
    for (let i = 0; i < 6; i += 1) {
      await page.settle();
      page.expireTimers(50);
    }
    assert.deepStrictEqual(postedWhere(page), [["an answer to the bridge", first.id]],
      "fixture: the answer was not posted into the bridge's thread");
    assert.equal(savedMessage(page, "300").content, finished, "fixture: the reply's read of All did not bring the finished text");
    // The read for the counts, sent before all of this, lands.
    held.counts[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    const landed = savedMessage(page, "300").content;
    // And three polls of Main, which never carry a thread reply.
    const polls = [];
    for (let poll = 0; poll < 3; poll += 1) {
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      polls.push(savedMessage(page, "300").content);
    }
    // Its thread, drawn from what the page holds.
    page.timeline = errorResponse(502, "chat_error", "history read timed out");
    await pickThread(page, `thread:${first.id}`);
    return { page, landed, polls, row: bodyOf(rowOf(page, "300")).text() };
  };
  const { landed, polls, row } = await run(SCRIPT);
  assert.equal(landed, finished, "the read for the counts put back the text the reply was pushed with");
  assert.deepStrictEqual(polls, Array(3).fill(finished), "the reply lost its finished text by the polls after");
  assert.ok(row.includes(finished), `its thread shows the reply as it was pushed: ${JSON.stringify(row)}`);

  // THE CONTROL, as the page was: the reply's read of All gave the reply the stream's kind of copy, as
  // posted, and the read for the counts takes its own over it, which the polls after never correct.
  const control = await run(brokenScript("foldLiveMessage(message, false, false, number)", "foldLiveMessage(message)"));
  assert.equal(control.landed, pushedText, "the finished answer survived anyway, so this test cannot see it");
  assert.deepStrictEqual(control.polls, Array(3).fill(pushedText));
  assert.ok(control.row.includes(pushedText));
});

test("`#316 late-counts-resurrect`: a read of All for Main's counts does not bring back a message to the channel that arrived after it was sent and that a newer page of Main says is gone", async () => {
  const run = async (script) => {
    const { page, server, quietReply } = await heldAllReads(script);
    // The read for the counts, held: the server takes its answer at `take`, and the page has it at `give`.
    const inner = page.timeline;
    /** @type {{take: null | (() => Promise<void>), give: null | (() => void)}} */
    const counts = { take: null, give: null };
    page.timeline = (path, options) => {
      if (new URL(path, "http://fixture.test").searchParams.get("reason") !== "counts") return inner(path, options);
      return new Promise((resolve) => {
        /** @type {any} */
        let answer = null;
        counts.take = async () => {
          const response = await server.answer(path);
          const body = await response.text();
          answer = { ...response, text: async () => body };
        };
        counts.give = () => resolve(answer);
      });
    };
    page.setClock(page.clock() + COUNTS_ALL_READ_MS + 1000);
    quietReply();
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(counts.take, "fixture: All was not read for Main's counts");
    // A message to the channel, posted after the read was sent and before the server answered it.
    server.post(message({ id: "400", content: "posted while the read for the counts was out",
      timestamp: new Date(page.clock()).toISOString() }));
    await counts.take();
    // Main's poll brings it; then it is deleted upstream, by a provider that pushes no removals, and
    // Main's next poll, its newest page, no longer carries it.
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(shownIds(page).includes("400"), "fixture: Main's poll did not bring the message");
    server.remove("400");
    server.refuse = { status: 410, error: "cursor_expired" };
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(!shownIds(page).includes("400"), "fixture: Main's newest page did not take the message out");
    counts.give?.();
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page };
  };
  const { page } = await run(SCRIPT);
  assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the read for the counts brought back a message Main's newer page says is gone");
  assert.equal(savedMessage(page, "400"), null);
  assert.equal(savedMessage(page, "250").content, "a quiet reply in the second discussion", "the read for the counts brought nothing");

  // THE CONTROL: taking every message the store has not held, the read for the counts puts it back.
  const control = await run(await brokenModule([{ file: "src/core/channels/fold.ts",
    find: "      return !deps.newerPageSpeaksFor(canon, message, number);\n", replace: "      return true;\n" }]));
  assert.ok(shownIds(control.page).includes("400"), "the read for the counts left the message out anyway, so this test cannot see it");
});

// A newest page of Main sent before a removal the stream reported takes the message out when it lands,
// whatever it carries. It used to leave that to the read behind the removal: a page answered after the
// removal does not carry the message, and its cursor is past the deletion, so that read, a delta from it,
// never reported it, and the message stayed on Main, and saved, until the next newest page fifteen
// minutes on. A removal replayed from the stream's tail asks for no read at all.
const REMOVED_LEFT_TO_ITS_READ = { file: "src/core/channels/fold.ts",
  find: "      if (removedSince(canon, id, number) || deps.removedAfterRead(canon, id, versions)) return true;\n",
  replace: "      if (removedSince(canon, id, number) || deps.removedAfterRead(canon, id, versions)) return false;\n" };

for (const removal of ["after", "before", "tail"]) {
  const named = { after: "answered after the removal", before: "answered before the removal",
    tail: "the removal replayed from the stream's tail, the page answered after it" }[removal];
  test(`\`#316 late-counts-resurrect\`: a message removed upstream while Main's newest page is out leaves Main and the store when that page lands (${named})`, async () => {
    const run = async (script) => {
      const { page, server } = await forwardPage({}, null, script);
      const inner = page.timeline;
      /** @type {Array<() => void>} */
      const held = [];
      let hold = false;
      page.timeline = (path, options) => {
        const url = new URL(path, "http://fixture.test");
        if (!hold || url.searchParams.get("view") !== "main" || url.searchParams.get("after")) return inner(path, options);
        hold = false;
        // Answered after the removal: the server takes the read as it hands it over. Before: at once.
        if (removal !== "before") return new Promise((resolve) => held.push(() => resolve(inner(path, options))));
        const answer = Promise.resolve(inner(path, options)).then(async (response) => {
          const body = await response.text();
          return { ...response, text: async () => body };
        });
        return new Promise((resolve) => held.push(() => resolve(answer)));
      };
      // One poll that is a delta, so the page knows this server's deltas carry edits and deletions.
      server.post(message({ id: "290", content: "posted before the removal" }));
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      // Main's poll reads its newest page, its cursor refused, and that read is held.
      server.refuse = { status: 410, error: "cursor_expired" };
      hold = true;
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(held.length, 1, "fixture: Main's newest page was not held");
      // The message is removed upstream, and the stream says so.
      server.remove("200");
      const tail = removal === "tail" ? { replayed: true, from_tail: true } : {};
      await deliver(page, page.stream(), sseDelete("del-200", CHANNEL.id, "200", tail));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.ok(shownIds(page).includes("200"), "fixture: the message left Main before Main's page landed");
      held.shift()();
      for (let i = 0; i < 4; i += 1) {
        await page.settle();
        page.expireTimers(50);
      }
      const landed = { shown: shownIds(page), saved: savedMessage(page, "200") };
      // And three polls after, deltas from that page's cursor.
      for (let poll = 0; poll < 3; poll += 1) {
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
        for (let i = 0; i < 3; i += 1) await page.settle();
      }
      return { page, server, landed };
    };
    const { page, server, landed } = await run(SCRIPT);
    assert.deepStrictEqual(landed.shown, ["201", "203", "290"], "the removed message is still on Main once Main's page landed");
    assert.equal(landed.saved, null, "the removed message is still in the saved store once Main's page landed");
    assert.deepStrictEqual(shownIds(page), ["201", "203", "290"], "the removed message is still on Main three polls later");
    assert.equal(savedMessage(page, "200"), null, "the removed message is still in the saved store three polls later");
    assert.ok(server.reads.slice(-3).every((read) => read.after !== null), "fixture: the polls after were not deltas");
    if (removal === "before") return;

    // THE CONTROL: the removal left to the read behind it, the message stays, on Main and saved, through
    // every poll after.
    const control = await run(await brokenModule([REMOVED_LEFT_TO_ITS_READ]));
    assert.ok(control.landed.shown.includes("200"), "the message left Main anyway, so this test cannot see it");
    assert.ok(control.landed.saved, "the message left the store anyway, so this test cannot see it");
    assert.ok(shownIds(control.page).includes("200") && savedMessage(control.page, "200"));
  });
}

// --- `#328 removal-outside-window` -------------------------------------------------------------------
//
// Found in the final review of #316 late-counts-resurrect and #310 stale-live-read-flicker, older than
// both. (1) A newest page keeps every held message older than its own oldest row, which it cannot speak
// for: so a message the stream reported removed stayed, shown and saved, when it was older than the
// page's window, though the README said the first newest page to land takes it out. (2) A reply the
// stream left unplaced, taken out of the store by a newest page of Main, was put back by a reply's own
// read of All (`reason=reply-place`) as a new reply, and counted again on its root.

/** The line of `mergeIntoCanon` that drops a held message removed upstream below a newest page's window, for a negative control. */
const REMOVED_BELOW_WINDOW = { file: "src/core/channels/fold.ts",
  find: "timeOf(item, timeField) < edge && !removed(item))", replace: "timeOf(item, timeField) < edge)" };
/** The thread the device's copy of message `id` is in, or null. */
const threadOfSaved = (page, id) => {
  const saved = savedMessage(page, id);
  return saved && saved.thread ? saved.thread.id : null;
};
/** The rule that a reply's read of All counts no reply on its root, for a negative control. */
const REPLY_PLACE_COUNTS_NOTHING = { file: "src/core/channels/live.ts",
  find: "!held && !replayed && read === null && thread !== null", replace: "!held && !replayed && thread !== null" };

test("`#328 removal-outside-window`: a message the stream reports removed is taken out by the next newest page of its view even when it is older than that page reaches back to", async () => {
  const run = async (script = SCRIPT) => {
    // A backend whose deltas carry additions only, so the read behind a removal is a newest page.
    const { page, server } = await forwardPage({ complete: false }, null, script);
    assert.deepStrictEqual(shownIds(page), ["200", "201", "203"], "the premise is Main holding 200");
    // From now on a newest page of Main carries its newest row and says there is more: 200 and 201 are
    // below it.
    server.newestLimit = 1;
    server.remove("200");
    const reads = page.timelineCalls.length;
    await deliver(page, page.stream(), sseDelete("del-200", CHANNEL.id, "200"));
    for (let i = 0; i < 4; i += 1) await page.settle();
    assert.deepStrictEqual(readsSince(page, reads).map(([view, delta]) => [view, delta]), [["main", false]],
      "the premise is the read behind the removal, a newest page of Main");
    return page;
  };
  const page = await run();
  assert.deepStrictEqual(shownIds(page), ["201", "203"], "a message reported removed stays on Main below the page's window");
  assert.equal(savedMessage(page, "200"), null, "a message reported removed stays in the store below the page's window");
  // And the poll after, a newest page again, does not bring it back.
  page.setClock(page.clock() + DISCORD_POLL_MS);
  await pollOnce(page);
  assert.deepStrictEqual(shownIds(page), ["201", "203"]);
  // THE CONTROL: below the window every held message stays, removed or not, until a read reaches it.
  const control = await run(await brokenModule([REMOVED_BELOW_WINDOW]));
  assert.deepStrictEqual(shownIds(control), ["200", "201", "203"], "below the window the removed message went anyway, so this test cannot see it");
  assert.notEqual(savedMessage(control, "200"), null);
});

test("`#328 removal-outside-window`: a reply the stream left unplaced, taken out by a newest page of Main, is placed by a reply's own read of All without counting twice on its root", async () => {
  const run = async (script = SCRIPT) => {
    const { page, server } = await heldAllReads(script);
    // The bridge answers in the first discussion: the root says 2 replies, and the stream carries the
    // reply with no thread record, so it is left unplaced, on Main (its placing read held).
    const { first, pushed } = bridgeReply(page, server);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(rowOf(page, "300"), "the premise is the bare reply on Main");
    // The reader opens Reply on it, and types.
    distinctReplies(page);
    await replyButton(rowOf(page, "300")).click();
    await page.el("reply-text").setValue("an answer to the bridge");
    // Meanwhile a newest page of Main, which never carries a thread reply, takes it out of the store.
    server.refuse = { status: 410, error: "cursor_expired" };
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(savedMessage(page, "300"), null, "the premise is the reply taken out by Main's newest page");
    assert.equal(savedMessage(page, "201").thread.reply_count, 2, "the premise is the root counting the reply");
    const reads = page.timelineCalls.length;
    // Send: the reply's own read of All places it.
    page.el("reply-send").click();
    for (let i = 0; i < 6; i += 1) {
      await page.settle();
      page.expireTimers(50);
    }
    assert.deepStrictEqual(readsSince(page, reads), [["flat", false, "reply-place"]], "the premise is the reply's own read of All");
    assert.deepStrictEqual(postedWhere(page), [["an answer to the bridge", first.id]], "the answer was not posted into the bridge's thread");
    assert.ok(savedMessage(page, "300") && threadOfSaved(page, "300") === first.id,
      "the reply's read of All did not put the bridge's reply back, in its thread");
    return page;
  };
  const page = await run();
  assert.equal(savedMessage(page, "201").thread.reply_count, 2, "the root counts the reply its read of All put back twice");
  assert.equal(page.screen(), "main", "the premise is Main on screen after the send");
  assert.equal(threadButton(rowOf(page, "201")).textContent, "2 replies", "Main's root says another count than its thread has");
  // THE CONTROL: the reply's read of All counting what it puts back as a new reply: 3 for a thread of 2.
  const control = await run(await brokenModule([REPLY_PLACE_COUNTS_NOTHING]));
  assert.equal(savedMessage(control, "201").thread.reply_count, 3, "the put-back reply was not counted again anyway, so this test cannot see it");
});

// --- `#310 stale-live-read-flicker`: a late live read never takes away what the stream drew after it ------
//
// A live message is drawn the moment the stream delivers it, and the page then reads the channel again.
// When a second live message arrives while the first message's read is still on the wire, that read's
// answer, taken before the second message existed, landed after the stream had drawn it, and redrew the
// column without it until the second message's own read put it back: a message the reader saw arrive
// disappeared and came back (against `#280 scroll-stability`). The column's own newest page is now
// folded only over the store's copies older than it (`pageFold`, `copyOlderThan`), as a late read of All
// is (`#316 late-counts-resurrect`): a message the stream delivered after it was sent is newer, and stays.

for (const view of ["flat", "main"]) {
  test(`\`#310 stale-live-read-flicker\`: a second live message drawn while the first one's read is out stays drawn when that read lands late (${view === "flat" ? "All" : "Main"})`, async () => {
    const run = async (script) => {
      const page = newPage(new Map(), script);
      const data = threadData();
      page.threadingSupported = true;
      page.threads = data.threads;
      await signIn(page);
      await showDiscord(page, data.messages);
      if (view === "main") await pickThread(page, "main");
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(page.el("thread-select").value, view, "fixture: not the view meant");
      const serve = page.timeline;
      /** @type {Array<() => void>} */
      const held = [];
      page.timeline = (path, options) => {
        if (new URL(path, "http://fixture.test").searchParams.get("reason") !== "live") return serve(path, options);
        // Answered as the server had it when asked, and handed over later.
        const answer = Promise.resolve(serve(path, options)).then(async (response) => {
          const body = await response.text();
          return { ...response, text: async () => body };
        });
        return new Promise((resolve) => held.push(() => resolve(answer)));
      };
      const first = message({ id: "300", content: "the first live message" });
      page.messages.push(first);
      await deliver(page, page.stream(), sseMessage(first));
      for (let i = 0; i < 2; i += 1) await page.settle();
      assert.equal(held.length, 1, "fixture: the first message's read was not made");
      // Thirteen milliseconds later, the second, while the first one's read is still out.
      page.setClock(page.clock() + 13);
      // From someone else, so the two are rows of their own.
      const second = message({ id: "301", content: "the second live message", author: "alice",
        author_id: "1000000000000000005", author_is_bot: false });
      page.messages.push(second);
      await deliver(page, page.stream(), sseMessage(second));
      for (let i = 0; i < 2; i += 1) await page.settle();
      const drawn = rowOf(page, "301");
      assert.ok(drawn, "fixture: the stream did not draw the second message");
      // The first message's read lands, late: taken before the second message existed.
      held.shift()();
      for (let i = 0; i < 3; i += 1) await page.settle();
      const landed = { ids: shownIds(page), same: rowOf(page, "301") === drawn, saved: savedMessage(page, "301") !== null,
        asked: held.length };
      // And the second message's own read.
      while (held.length) held.shift()();
      for (let i = 0; i < 3; i += 1) await page.settle();
      return { page, landed };
    };
    const { page, landed } = await run(SCRIPT);
    assert.equal(landed.asked, 1, "fixture: the second message's read was not asked behind the first");
    assert.ok(landed.ids.includes("301"), `the late read took away the message the stream drew after it was sent: ${JSON.stringify(landed.ids)}`);
    assert.ok(landed.same, "the late read redrew the second message's row");
    assert.ok(landed.saved, "the late read took the second message out of the store");
    assert.ok(shownIds(page).includes("300") && shownIds(page).includes("301"), "a live message is missing once both reads landed");

    // THE CONTROL, as before: the column's own newest page folded whatever the store gained since it was
    // sent takes the second message away until its own read lands.
    const control = await run(brokenScript("      const held = heldNow(canon);\n", "      const held = null;\n"));
    assert.ok(!control.landed.ids.includes("301"), "the late read left the second message drawn anyway, so this test cannot see it");
    assert.ok(shownIds(control.page).includes("301"), "the control's second read did not put the message back");
  });
}

// Ordered by when each read was sent (`copyOrder`), not by whether a copy changed while the page was out.
// The read of All for Main's counts, sent first and answered after an edit, landing while Main's newer
// newest page is out, replaced the store's copy; Main's page, answered after the next edit or a removal,
// took that for a copy newer than itself, and refused its own copy, or the removal, until the next
// newest page: fifteen minutes on a server whose deltas carry edits and deletions, since the polls
// between are deltas from the refused page's cursor.

for (const change of ["edit", "remove"]) {
  test(`\`#310 stale-live-read-flicker\`: Main's newest page takes its ${change === "edit" ? "copy of an edit" : "removal of a message"} over the copy an older read of All brought while it was out`, async () => {
    const run = async (script) => {
      const { page, server, quietReply } = await heldAllReads(script);
      const inner = page.timeline;
      // The read for the counts, held: the server takes its answer at `take`, and the page has it at `give`.
      /** @type {{take: null | (() => Promise<void>), give: null | (() => void)}} */
      const counts = { take: null, give: null };
      /** @type {Array<() => void>} */
      const mainHeld = [];
      let holdMain = false;
      page.timeline = (path, options) => {
        const url = new URL(path, "http://fixture.test");
        if (url.searchParams.get("reason") === "counts") {
          return new Promise((resolve) => {
            /** @type {any} */
            let answer = null;
            counts.take = async () => {
              const response = await server.answer(path);
              const body = await response.text();
              answer = { ...response, text: async () => body };
            };
            counts.give = () => resolve(answer);
          });
        }
        if (holdMain && url.searchParams.get("view") === "main" && !url.searchParams.get("after")) {
          // Answered as the server has the channel when asked, and handed over later.
          const answer = Promise.resolve(inner(path, options)).then(async (response) => {
            const body = await response.text();
            return { ...response, text: async () => body };
          });
          return new Promise((resolve) => mainHeld.push(() => resolve(answer)));
        }
        return inner(path, options);
      };
      page.setClock(page.clock() + COUNTS_ALL_READ_MS + 1000);
      quietReply();
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.ok(counts.take, "fixture: All was not read for Main's counts");
      // The server answers the read for the counts after a first edit of the message...
      server.edit("200", { content: "the announcement, edited once" });
      await counts.take();
      // ...then it changes again upstream — by a provider that pushes no removals, for a removal — and
      // Main's poll, its cursor refused, reads its newest page, which is held.
      if (change === "edit") server.edit("200", { content: "the announcement, edited twice" });
      else server.remove("200");
      server.refuse = { status: 410, error: "cursor_expired" };
      holdMain = true;
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(mainHeld.length, 1, "fixture: Main's newest page was not held");
      holdMain = false;
      // The older read lands first, over the store as it was when it was sent...
      counts.give?.();
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(savedMessage(page, "200").content, "the announcement, edited once",
        "fixture: the read for the counts did not bring the first edit");
      // ...then Main's newer page.
      mainHeld.shift()();
      for (let i = 0; i < 4; i += 1) await page.settle();
      const landed = { saved: savedMessage(page, "200"), shown: shownIds(page) };
      // And three polls after, deltas from that page's cursor.
      for (let poll = 0; poll < 3; poll += 1) {
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
        for (let i = 0; i < 3; i += 1) await page.settle();
      }
      assert.ok(server.reads.slice(-3).every((read) => read.after !== null), "fixture: the polls after were not deltas");
      return { page, landed };
    };
    const { page, landed } = await run(SCRIPT);
    if (change === "edit") {
      assert.equal(landed.saved && landed.saved.content, "the announcement, edited twice",
        "Main's newer page did not bring the second edit");
      assert.ok(bodyOf(rowOf(page, "200")).text().includes("edited twice"), "Main does not show the second edit");
      assert.equal(savedMessage(page, "200").content, "the announcement, edited twice", "the second edit is gone three polls later");
    } else {
      assert.ok(!landed.shown.includes("200"), "Main's newer page did not take the removed message off Main");
      assert.equal(landed.saved, null, "Main's newer page did not take the removed message out of the store");
      assert.ok(!shownIds(page).includes("200"), "the removed message is back on Main three polls later");
    }
    assert.equal(savedMessage(page, "250").content, "a quiet reply in the second discussion", "the read for the counts brought nothing");

    // THE CONTROL, as the page was: a copy replaced while the page was out taken for newer than it, the
    // page refuses its own, and the polls after, deltas from its cursor, never bring it.
    const control = await run(await brokenModule([{ file: "src/core/channels/order.ts",
      find: "      if (at !== undefined) return at < number;\n", replace: "" }]));
    if (change === "edit") {
      assert.equal(control.landed.saved.content, "the announcement, edited once",
        "Main's page brought the second edit anyway, so this test cannot see it");
      assert.equal(savedMessage(control.page, "200").content, "the announcement, edited once");
    } else {
      assert.ok(control.landed.shown.includes("200") && control.landed.saved,
        "Main's page took the message out anyway, so this test cannot see it");
      assert.ok(shownIds(control.page).includes("200"));
    }
  });
}

// A copy the stream made says nothing newer than the store's copy it was built on (`copyBuiltOn`,
// `postedCopies`). The stream carries no edits: its copy of a message is the message as it was posted,
// and a count or a thread record it puts on the store's copy changes nothing else. Stamped as new as the
// moment it went into the store, such a copy made a newest page sent before then refuse its own, though
// that page, answered after an edit, carried the newer text; the polls after, deltas from that page's
// cursor, already past the edit, never brought it, and the old text stayed shown and saved until the next
// newest page, fifteen minutes on a server whose deltas carry edits.

/**
 * The forward-capable channel read in `view`, Main or All, with one delta poll behind it, so the page knows
 * the server's deltas carry edits and deletions; then the view's next poll, its cursor refused, reads its
 * newest page, which is held. `land` hands that page over, answered as the server has the channel then.
 *
 * @param {string} script
 * @param {"main" | "flat"} view
 */
async function newestPageOut(script, view) {
  const { page, server } = await forwardPage({}, null, script);
  if (view === "flat") await pickThread(page, "flat");
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(page.el("thread-select").value, view, "fixture: not the view meant");
  const inner = page.timeline;
  /** @type {Array<() => void>} */
  const held = [];
  let hold = false;
  page.timeline = (path, options) => {
    const url = new URL(path, "http://fixture.test");
    if (!hold || url.searchParams.get("view") !== view || url.searchParams.get("after")) return inner(path, options);
    hold = false;
    return new Promise((resolve) => held.push(() => resolve(inner(path, options))));
  };
  server.post(message({ id: "290", content: "posted before the newest page was sent" }));
  page.setClock(page.clock() + DISCORD_POLL_MS);
  await pollOnce(page);
  for (let i = 0; i < 3; i += 1) await page.settle();
  server.refuse = { status: 410, error: "cursor_expired" };
  hold = true;
  page.setClock(page.clock() + DISCORD_POLL_MS);
  await pollOnce(page);
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(held.length, 1, "fixture: the view's newest page was not held");
  const land = async () => {
    held.shift()();
    for (let i = 0; i < 6; i += 1) {
      await page.settle();
      page.expireTimers(50);
    }
  };
  return { page, server, land };
}

/** Three polls, deltas from the cursor of the page that landed last: message `id`'s saved text after each. */
async function threePolls(page, server, id) {
  /** @type {Array<string | null>} */
  const texts = [];
  for (let poll = 0; poll < 3; poll += 1) {
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    const saved = savedMessage(page, id);
    texts.push(saved && saved.content);
  }
  assert.ok(server.reads.slice(-3).every((read) => read.after !== null), "fixture: the polls after were not deltas");
  return texts;
}

/** The text of message `id`'s row in the column, or null if it has none. */
const rowText = (page, id) => {
  const row = rowOf(page, id);
  return row ? bodyOf(row).text() : null;
};

test("`#310 stale-live-read-flicker`: Main's newest page brings the edit of a message the stream drew while it was out", async () => {
  const answer = "Here is the answer.";
  const run = async (script) => {
    const { page, server, land } = await newestPageOut(script, "main");
    // A bot posts its placeholder, which the stream draws, and edits it to its answer, pushed as an update.
    const placeholder = message({ id: "310", content: "Working…" });
    server.post({ ...placeholder });
    await deliver(page, page.stream(), sseMessage(placeholder));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(rowOf(page, "310"), "fixture: the stream did not draw the placeholder");
    server.edit("310", { content: answer });
    await deliver(page, page.stream(), sseUpdate("upd-310", { ...placeholder, content: answer }));
    for (let i = 0; i < 3; i += 1) await page.settle();
    // Main's page, answered after the edit, lands.
    await land();
    const saved = savedMessage(page, "310");
    const landed = { saved: saved && saved.content, row: rowText(page, "310") };
    return { page, landed, polls: await threePolls(page, server, "310") };
  };
  const { page, landed, polls } = await run(SCRIPT);
  assert.equal(landed.saved, answer, "Main's newest page did not bring the edit");
  assert.ok(landed.row && landed.row.includes(answer), `Main does not show the edit: ${JSON.stringify(landed.row)}`);
  assert.deepStrictEqual(polls, [answer, answer, answer], "the edit is gone in the polls after");
  assert.ok((rowText(page, "310") || "").includes(answer), "Main does not show the edit three polls later");

  // THE CONTROL: the stream's copy taken for newer in what it says than every read sent before it arrived,
  // Main's page refuses its own, and the polls after, deltas from its cursor, never bring the edit.
  const control = await run(await brokenModule([{ file: "src/core/channels/order.ts",
    find: "      if (says && postedCopies.has(copy)) return true;\n", replace: "" }]));
  assert.equal(control.landed.saved, "Working…", "Main's page brought the edit anyway, so this test cannot see it");
  assert.deepStrictEqual(control.polls, ["Working…", "Working…", "Working…"]);
});

for (const change of ["edit", "pushed edit", "removal"]) {
  const named = { edit: "edit", "pushed edit": "edit, pushed as an update,", removal: "removal" }[change];
  test(`\`#310 stale-live-read-flicker\`: Main's newest page brings a root's ${named} though a live reply moved its count on while the page was out`, async () => {
    const edited = "first thread root, edited";
    const run = async (script) => {
      const { page, server, land } = await newestPageOut(script, "main");
      const first = page.messages.find((m) => m.id === "201").thread;
      if (change !== "removal") {
        server.edit("201", { content: edited });
        if (change === "pushed edit") {
          await deliver(page, page.stream(), sseUpdate("upd-201", page.messages.find((m) => m.id === "201")));
          for (let i = 0; i < 3; i += 1) await page.settle();
        }
      }
      // A reply to it arrives on the stream, and the store's copy of the root counts it.
      const reply = message({ id: "260", content: "a live reply", thread: { ...first, is_root: false } });
      server.post(reply);
      server.edit("201", { thread: { ...first, reply_count: 2 } });
      await deliver(page, page.stream(), sseMessage(reply));
      for (let i = 0; i < 3; i += 1) await page.settle();
      assert.equal(savedMessage(page, "201").thread.reply_count, 2, "fixture: the live reply did not move the root's count on");
      // Removed upstream after that, by a provider that pushes no removals.
      if (change === "removal") server.remove("201");
      // Main's page, answered now, lands.
      await land();
      const landed = { saved: savedMessage(page, "201"), shown: shownIds(page) };
      return { page, landed, polls: await threePolls(page, server, "201") };
    };
    const { page, landed, polls } = await run(SCRIPT);
    if (change === "removal") {
      assert.ok(!landed.shown.includes("201"), "Main's newest page did not take the removed root off Main");
      assert.equal(landed.saved, null, "Main's newest page did not take the removed root out of the store");
      assert.deepStrictEqual(polls, [null, null, null], "the removed root is back in the polls after");
      assert.ok(!shownIds(page).includes("201"), "the removed root is back on Main three polls later");
    } else {
      assert.equal(landed.saved && landed.saved.content, edited, "Main's newest page did not bring the root's edit");
      assert.equal(landed.saved.thread.reply_count, 2, "Main's newest page lost the live reply from the root's count");
      assert.deepStrictEqual(polls, [edited, edited, edited], "the root's edit is gone in the polls after");
      assert.ok((rowText(page, "201") || "").includes(edited), "Main does not show the root's edit three polls later");
    }

    // THE CONTROL: the root's count stamped as new as the moment the live reply moved it on, Main's page,
    // sent before then, refuses its copy of the root, or its removal, and the polls after never bring it.
    const control = await run(await brokenModule([{ file: "src/core/channels/live.ts",
      find: "    deps.ledger.buildCopyOn(counted, root);\n", replace: "    deps.ledger.orderCopy(counted, deps.orderNow());\n" }]));
    if (change === "removal") {
      assert.ok(control.landed.shown.includes("201") && control.landed.saved,
        "Main's page took the root out anyway, so this test cannot see it");
      assert.ok(shownIds(control.page).includes("201"));
    } else {
      assert.equal(control.landed.saved.content, "first thread root", "Main's page brought the edit anyway, so this test cannot see it");
      assert.deepStrictEqual(control.polls, ["first thread root", "first thread root", "first thread root"]);
    }
  });
}

test("`#310 stale-live-read-flicker`: All's newest page brings a bridge's finished answer though the stream placed it while the page was out", async () => {
  const finished = "the bridge's answer, finished";
  const run = async (script) => {
    const { page, server, land } = await newestPageOut(script, "flat");
    // The bridge pushes its reply without a thread record, then edits it, and the edit's copy on the
    // stream names its thread, which places it (`placeFromEdit`).
    const { first, pushed } = bridgeReply(page, server);
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    server.edit("300", { content: finished });
    await deliver(page, page.stream(),
      sseUpdate("upd-300", { ...pushed, content: finished, thread: { ...first, is_root: false, reply_count: 2 } }));
    for (let i = 0; i < 3; i += 1) await page.settle();
    const placed = savedMessage(page, "300");
    assert.ok(placed && placed.thread && placed.thread.id === first.id, "fixture: the edit's copy did not place the reply");
    assert.equal(placed.content, pushed.content, "fixture: the reply's text changed before All's page landed");
    // All's page, answered after the edit, lands.
    await land();
    const saved = savedMessage(page, "300");
    const landed = { saved: saved && saved.content, row: rowText(page, "300") };
    return { page, landed, polls: await threePolls(page, server, "300") };
  };
  const { page, landed, polls } = await run(SCRIPT);
  assert.equal(landed.saved, finished, "All's newest page did not bring the finished answer");
  assert.ok(landed.row && landed.row.includes(finished), `All does not show the finished answer: ${JSON.stringify(landed.row)}`);
  assert.deepStrictEqual(polls, [finished, finished, finished], "the finished answer is gone in the polls after");
  assert.ok((rowText(page, "300") || "").includes(finished), "All does not show the finished answer three polls later");

  // THE CONTROL: the thread record the stream put on stamped as new as the moment it went in, All's page,
  // sent before then, refuses its copy, and the polls after never bring the finished text.
  const control = await run(brokenScript("    buildCopyOn(placed, held);\n", "    orderCopy(placed, orderNow());\n"));
  assert.equal(control.landed.saved, "the bridge's answer, in the first discussion",
    "All's page brought the finished answer anyway, so this test cannot see it");
  assert.deepStrictEqual(control.polls, Array(3).fill("the bridge's answer, in the first discussion"));
});

// Not every copy the stream made is older than every page: a count or a thread record put on a copy that a
// read sent after the page brought is as new as that read (`copyBuiltOn`), and a page sent before it,
// answered before the edit that read brought, would undo the edit.

test("`#316 late-counts-resurrect`: a read of All for Main's counts does not undo an edit Main's poll brought, though a live reply moved that root's count on since", async () => {
  const edited = "first thread root, edited while the read for the counts was out";
  const run = async (script) => {
    const { page, server, held } = await countsReadOut(script);
    // A copy of its own, so the read for the counts already taken keeps the text it was taken with.
    const at = page.messages.findIndex((m) => m.id === "201");
    page.messages[at] = { ...page.messages[at] };
    const first = page.messages[at].thread;
    server.edit("201", { content: edited });
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(savedMessage(page, "201").content, edited, "fixture: Main's poll did not bring the edit");
    // A reply to it arrives on the stream, and the store's copy of the root counts it. The read behind
    // the reply is held, so the read for the counts lands first.
    const inner = page.timeline;
    /** @type {Array<() => void>} */
    const live = [];
    page.timeline = (path, options) => {
      if (new URL(path, "http://fixture.test").searchParams.get("reason") !== "live") return inner(path, options);
      return new Promise((resolve) => live.push(() => resolve(inner(path, options))));
    };
    const reply = message({ id: "260", content: "a live reply", thread: { ...first, is_root: false } });
    server.post(reply);
    server.edit("201", { thread: { ...first, reply_count: 2 } });
    await deliver(page, page.stream(), sseMessage(reply));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(savedMessage(page, "201").thread.reply_count, 2, "fixture: the live reply did not move the root's count on");
    held.counts[0]();
    for (let i = 0; i < 3; i += 1) await page.settle();
    const saved = savedMessage(page, "201");
    const landed = { saved: saved && saved.content, count: saved && saved.thread.reply_count, row: rowText(page, "201") };
    page.timeline = inner;
    while (live.length) live.shift()();
    for (let i = 0; i < 3; i += 1) await page.settle();
    return { page, landed };
  };
  const { page, landed } = await run(SCRIPT);
  assert.equal(landed.saved, edited, "the read for the counts put back the root's text from before the edit");
  assert.ok(landed.row && landed.row.includes(edited), "Main shows the root as it was before the edit");
  assert.equal(landed.count, 2, "the read for the counts lost the live reply from the root's count");
  assert.equal(savedMessage(page, "201").content, edited, "the edit is gone once the read behind the reply landed");

  // THE CONTROL: the root's count taken for older in what it says than any page, as a live message is, the
  // read for the counts takes its copy of the root over it, from before the edit.
  const control = await run(await brokenModule([{ file: "src/core/channels/live.ts", find: "    deps.ledger.buildCopyOn(counted, root);\n",
    replace: "    deps.ledger.buildCopyOn(counted, root);\n    deps.ledger.markPosted(counted);\n" }]));
  assert.equal(control.landed.saved, "first thread root", "the edit survived anyway, so this test cannot see it");
});

// A message the store never held as a late read was sent, and does not hold as it lands, is not taken from
// it once a newer newest page of the view the message belongs in has landed (`newerPageSpeaksFor`): its
// thread's, for a thread reply. Asking only of Main, the read of All for Main's counts put back a reply
// that a newer newest page of its thread said was gone, into the thread and the saved store, and the
// polls after, deltas from that page's cursor, never took it out again.

for (const when of ["held", "never held"]) {
  const named = when === "held" ? "which took it out" : "the reply removed before its thread was read";
  test(`\`#316 late-counts-resurrect\`: a read of All for Main's counts does not put back a reply removed upstream that a newer newest page of its thread says is gone (${named})`, async () => {
    const run = async (script) => {
      const { page, server, held } = await countsReadOut(script);
      const second = page.messages.find((m) => m.id === "203").thread;
      // Removed upstream, by a provider that pushes no removals.
      const remove = () => {
        server.remove("250");
        server.edit("203", { thread: { ...second, reply_count: 1, reply_count_exact: true } });
      };
      if (when === "never held") remove();
      await pickThread(page, `thread:${second.id}`);
      for (let i = 0; i < 3; i += 1) await page.settle();
      if (when === "held") {
        assert.ok(savedMessage(page, "250"), "fixture: the thread's read did not bring the reply");
        remove();
        // The thread's poll, its cursor refused, reads its newest page, which no longer carries it.
        server.refuse = { status: 410, error: "cursor_expired" };
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
        for (let i = 0; i < 3; i += 1) await page.settle();
      }
      assert.ok(!savedMessage(page, "250") && !shownIds(page).includes("250"),
        "fixture: the reply is in its thread before the read for the counts lands");
      held.counts[0]();
      for (let i = 0; i < 3; i += 1) await page.settle();
      const landed = { shown: shownIds(page).includes("250"), saved: Boolean(savedMessage(page, "250")) };
      // And three polls after, deltas from the thread page's cursor.
      const polls = [];
      for (let poll = 0; poll < 3; poll += 1) {
        page.setClock(page.clock() + DISCORD_POLL_MS);
        await pollOnce(page);
        for (let i = 0; i < 3; i += 1) await page.settle();
        polls.push({ shown: shownIds(page).includes("250"), saved: Boolean(savedMessage(page, "250")) });
      }
      assert.ok(server.reads.slice(-3).every((read) => read.view === "thread" && read.after !== null),
        "fixture: the polls after were not deltas of the thread");
      return { page, landed, polls };
    };
    const { page, landed, polls } = await run(SCRIPT);
    assert.deepStrictEqual(landed, { shown: false, saved: false }, "the read for the counts put the removed reply into its thread");
    assert.ok(polls.every((state) => !state.shown && !state.saved), `the removed reply is back in the polls after: ${JSON.stringify(polls)}`);
    assert.ok(shownIds(page).includes("204"), "the thread lost a reply it still has");

    // THE CONTROL, as the page was: asking only whether a newer newest page of Main has landed, the read for
    // the counts puts the reply into its thread and the saved store, and the polls after keep it there.
    // In the module, `answeredThread` and `newerPage` are the legacy script's own functions, found at run
    // time in the page's global scope as every top-level function of a classic script is.
    const control = await run(await brokenModule([{ file: "src/core/channels/fold.ts",
      find: "      return !deps.newerPageSpeaksFor(canon, message, number);\n",
      replace: "      return answeredThread(message) !== null || !newerPage(canon, \"main\", number);\n" }]));
    assert.deepStrictEqual(control.landed, { shown: true, saved: true }, "the reply stayed out anyway, so this test cannot see it");
    assert.ok(control.polls.every((state) => state.shown && state.saved));
  });
}

test("`#316 late-counts-resurrect`: a placing read that is not folded does not put back a reply removed upstream that a newer newest page of its thread says is gone", async () => {
  const run = async (script) => {
    const { page, server, held } = await heldAllReads(script);
    // The bridge pushes a reply without its thread, and All is read to place it, held.
    const { first, pushed } = bridgeReply(page, server);
    await deliver(page, page.stream(), sseMessage(pushed));
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(held.place.length, 1, "fixture: no read of All was made to place the reply");
    // Main's newest page, which carries no thread reply, takes the bare copy out.
    server.refuse = { status: 410, error: "cursor_expired" };
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.equal(savedMessage(page, "300"), null, "fixture: Main's newest page did not take the bare copy out");
    // The reply is removed upstream, by a provider that pushes no removals, and the reader opens its
    // thread, whose newest page no longer carries it.
    server.remove("300");
    server.edit("201", { thread: { ...first, reply_count: 1 } });
    await pickThread(page, `thread:${first.id}`);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.ok(!shownIds(page).includes("300"), "fixture: the thread's page carried the removed reply");
    // The placing read, answered when it was asked, lands.
    held.place[0]();
    for (let i = 0; i < 4; i += 1) await page.settle();
    const landed = { shown: shownIds(page).includes("300"), saved: Boolean(savedMessage(page, "300")) };
    const polls = [];
    for (let poll = 0; poll < 3; poll += 1) {
      page.setClock(page.clock() + DISCORD_POLL_MS);
      await pollOnce(page);
      for (let i = 0; i < 3; i += 1) await page.settle();
      polls.push({ shown: shownIds(page).includes("300"), saved: Boolean(savedMessage(page, "300")) });
    }
    assert.ok(server.reads.slice(-3).every((read) => read.view === "thread" && read.after !== null),
      "fixture: the polls after were not deltas of the thread");
    return { page, landed, polls };
  };
  const { page, landed, polls } = await run(SCRIPT);
  assert.deepStrictEqual(landed, { shown: false, saved: false }, "the placing read put the removed reply back into its thread");
  assert.ok(polls.every((state) => !state.shown && !state.saved), `the removed reply is back in the polls after: ${JSON.stringify(polls)}`);
  assert.ok(shownIds(page).includes("202"), "the thread lost a reply it still has");

  // THE CONTROL, as the page was: asking only whether a newer newest page of Main has landed, the placing
  // read puts the reply back into its thread and the saved store, and the polls after keep it there.
  const control = await run(await brokenModule([{ file: "src/core/channels/placement.ts",
    find: "    : [...carried.values()].filter((copy) => !newerPageSpeaksFor(canon, copy, number));\n",
    replace: "    : [...carried.values()].filter((copy) => answeredThread(copy) !== null || !newerPage(canon, \"main\", number));\n" }]));
  assert.deepStrictEqual(control.landed, { shown: true, saved: true }, "the reply stayed out anyway, so this test cannot see it");
  assert.ok(control.polls.every((state) => state.shown && state.saved));
});

/**
 * A draft from the voice agent on the card, then the channel in two columns with the first thread on
 * the right: the card at the foot of the left column's list, under Main's 200.
 */
async function draftInTwoColumns(script = SCRIPT, arrange = null) {
  const page = newPage(new Map(), script);
  const data = threadData();
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  if (arrange) arrange(page, data);
  forwardTimeline(page);
  await proposedDuringACall(page);
  await showDiscord(page, data.messages);
  await twoColumns(page);
  await threadBadge(rowWithId(page, "201")).click();
  await page.settle();
  return page;
}

test("`#277 reply-in-column` with `#269 approval-card-inline`: a reply open at the foot of either column outlines the card's Send, as the reply screen does, and the card stays where it was", async () => {
  const page = await draftInTwoColumns();
  const card = page.el("post-confirm");
  assert.ok(inTwoColumns(page), "the premise is two columns");
  assert.equal(card.parentNode, page.el("pane-discord"), "the premise is the card under the left column's list");
  assert.equal(card.hasAttribute("data-beside-reply"), false, "no reply is open, yet the card's Send is outlined");
  assert.match(cssBlock('#post-confirm[data-beside-reply] #post-confirm-send'), /background:\s*transparent/,
    "beside a reply's box the card's Send is as filled as the reply's own");
  for (const [column, row] of [["left", () => rowWithId(page, "200")], ["right", () => sideRow(page, "202")]]) {
    await replyButton(row()).click();
    await page.settle();
    assert.equal(replyBoxAt(page), column, `${column}: the premise is a reply open at the foot of that column`);
    assert.equal(card.hasAttribute("data-beside-reply"), true,
      `${column}: a reply's filled Send is at a column's foot and the card's is filled as well`);
    // The card is where it was, a message of the left column's list, and still the draft's.
    assert.equal(card.parentNode, page.el("pane-discord"), `${column}: the reply moved the card`);
    assert.equal(card.nextSibling, page.el("channel-composer"), `${column}: the card left its place above the channel's box`);
    assert.equal(card.hidden, false, `${column}: the reply hid the draft`);
    await page.el("reply-cancel").click();
    await page.settle();
    assert.equal(card.hasAttribute("data-beside-reply"), false, `${column}: the card's Send stayed outlined after the reply went`);
  }
  // Sent from the card with a reply open beside it, the draft goes once, and the reply's words stay.
  await replyButton(rowWithId(page, "200")).click();
  await page.el("reply-text").setValue("still writing");
  await page.el("post-confirm-send").click();
  await page.settle();
  assert.equal(page.proposalCommits.length, 1, "Send on the card beside a reply did not commit the draft once");
  assert.equal(page.repliesPosted.length, 0, "the card's Send posted the reply");
  assert.equal(replyBoxAt(page), "left", "sending the draft closed the reply");
  assert.equal(page.el("reply-text").value, "still writing");

  // THE CONTROL: the card not told when a reply opens in a column. The walk above sees it.
  const control = await draftInTwoColumns(brokenScript(
    '  for (const id of ["channel-composer", "side-composer"]) leftEl(id).hidden = true;\n  renderReplyingMarks();\n  markCardBesideReply();\n',
    '  for (const id of ["channel-composer", "side-composer"]) leftEl(id).hidden = true;\n  renderReplyingMarks();\n'));
  await replyButton(rowWithId(control, "200")).click();
  await control.settle();
  assert.equal(control.el("post-confirm").hasAttribute("data-beside-reply"), false,
    "the control marked the card all the same, so this test cannot see the mark go missing");
});

test("`#277 reply-in-column` with `#269 approval-card-inline` and `#467 inline-send-in-place`: a reply of its own that did not go stays where it was written, and the jump goes on naming the draft below", async () => {
  const page = await draftInTwoColumns(SCRIPT, tallColumns);
  const area = page.el("scroll-area");
  const label = page.el("jump-newest-label");
  area.scrollTop = 0;
  await area.dispatch("scroll");
  assert.equal(page.el("jump-newest").hidden, false, "the premise is the jump up over a long column");
  assert.equal(label.textContent, "Draft to send", "the premise is the draft below naming the jump");
  page.replyResponse = errorResponse(422, "chat_error", "Google Chat returned HTTP 400");
  await replyButton(rowWithId(page, "200")).click();
  await page.el("reply-text").setValue("this will not go");
  await page.el("reply-send").click();
  await page.settle();
  // The reply's Retry is where the reader wrote it, under the message it answers, not at the foot.
  const held = inPlaceRows(page).find((one) => one.descendants().some((node) => node.textContent === "this will not go"));
  assert.ok(held && outgoingRetry(held), "the reply that did not go is not where it was written");
  assert.equal(label.textContent, "Draft to send", "a reply drawn where the reader is took the jump from the draft below");
  assert.equal(page.el("jump-newest").getAttribute("aria-label"), "Jump to the draft waiting to be sent");
});

// `#269 approval-card-inline` x `#280 scroll-stability`. On the channel the card is the list's tentative
// next message, and it moves the reader no more than a message arriving live does (`liveFollowers`):
// a reader on the newest line follows it down, and no other rule's place is undone by it. Asking only
// whether the reader was at the foot let go a message a filter change holds there, and undid a short
// thread's landing, which ends within the slack of the foot on purpose.

/** The card's follow asking only whether the reader is at the foot. */
const cardFollowsFootOnly = () => brokenScript(
  '  return currentView === "discord" ? liveFollowers().main : atBottom(leftEl("scroll-area"));',
  '  return atBottom(leftEl("scroll-area"));'
);

test("`#269 approval-card-inline` x `#280 scroll-stability`: a draft arriving under a message a filter change holds at the newest line leaves it there, and held", async () => {
  const run = async (script) => {
    const { page, messages } = pinnedPlacePage(["12", "27", "40"], script);
    await startTalking(page);
    await page.settle();
    await showDiscord(page, messages);
    await showPinned(page);
    // The newest message, pinned and chosen: once the filter goes it is at the newest line, held.
    await clickRow(page, "40", "pinned-log");
    await page.el("pinned-filter").click();
    await page.settle();
    return page;
  };
  const page = await run(SCRIPT);
  const area = page.el("scroll-area");
  assert.ok(atBottomOf(area), "the premise: the pin is at the newest line once the filter goes");
  const y = yOf(page, "40");
  page.propose(proposal());
  await page.settle();
  assert.equal(page.el("post-confirm").hidden, false, "the draft never reached the card");
  assert.equal(page.el("post-confirm").parentNode, page.el("pane-discord"), "the card is not the channel's next message");
  keptAt(page, "40", y, "the draft arriving");
  assert.equal(page.el("jump-newest").hidden, false, "nothing says a draft waits below");
  // ...and still held: a read bringing a message below moves it no more than it would have without the card.
  page.messages = [...page.messages, message({ id: "41", content: longMessage("arrived") })];
  await reReadChannel(page);
  keptAt(page, "40", y, "a read after the draft arrived");

  // THE CONTROL: the card followed by the foot alone lets the held message go, and the read moves it.
  const control = await run(cardFollowsFootOnly());
  const before = yOf(control, "40");
  control.propose(proposal());
  await control.settle();
  control.messages = [...control.messages, message({ id: "41", content: longMessage("arrived") })];
  await reReadChannel(control);
  assert.ok(Math.abs(yOf(control, "40") - before) > 1,
    "the control kept the message too, so this test cannot see the card let it go");
});

test("`#269 approval-card-inline` x `#280 scroll-stability`: a draft arriving under a short thread's landing leaves its first unread at the head, where the entry put it", async () => {
  const run = async (script) => {
    const page = newPage(new Map(), script);
    const messages = runThread(page, { tail: [["agent", false]] });
    await startTalking(page);
    await page.settle();
    await showDiscord(page, messages);
    await chooseReadMode(page, "Hide read");
    // The list as tall as the dock leaves it at a desk once the thread's name wraps it: the landing's
    // foot within the slack, as "...and the dock and the chips changing as the thread opens" has it
    // (456px there too since `#324 live-pill-covers-row` lands the answer's day rule with it).
    const area = page.el("scroll-area");
    area.clientHeight = 456;
    await pickThread(page, `thread:${RUN_THREAD}`);
    const answer = () => rowWithId(page, runId(500));
    const landed = atTheHead(page, answer());
    const short = area.scrollHeight - area.scrollTop - area.clientHeight;
    page.propose(proposal());
    await page.settle();
    return {
      landed,
      nearTheEnd: short >= 0 && short <= BOTTOM_SLACK_PX,
      shown: !page.el("post-confirm").hidden && page.el("post-confirm").parentNode === page.el("pane-discord"),
      kept: atTheHead(page, answer()),
      offered: !page.el("jump-newest").hidden,
    };
  };
  const real = await run(SCRIPT);
  assert.equal(real.landed, true, "the entry did not land the first unread at the head");
  assert.equal(real.nearTheEnd, true, "the premise: the landing is within the slack of the list's foot");
  assert.equal(real.shown, true, "the draft is not the thread's next message");
  assert.equal(real.kept, true, "the draft arriving took the landed message up under the floating line");
  assert.equal(real.offered, true, "nothing says a draft waits below");
  // THE CONTROL: followed by the foot alone, the card undoes the landing.
  const control = await run(cardFollowsFootOnly());
  assert.deepStrictEqual(control, { ...real, kept: false, offered: control.offered });
});

// `#269 approval-card-inline` x `#281 view-keys`. The thread list (Ctrl+3, Cmd+3 on a Mac) is the right
// column's cards across the window, with no box of its own, over the left column laid out behind it. A
// card left at the foot of the left column's list would wait there unseen, so the list docks it under
// its cards, as the Threads screen it replaced did; and the view keys, pressed from anywhere, a card's
// own buttons included, must move the card and do nothing else to it.

const LONG_CARD_DRAFT = "The release notes for this week, as agreed: the runner fix landed and the nightly is green. ".repeat(4);

/**
 * A draft from the voice agent on the card, and the channel of `manyThreads(16)` in Main, one column, at
 * its newest message, the card at its foot. The fixture lays nothing out, so the page is given what the
 * browser gives it: while the card is docked on the main screen it takes its `offsetHeight`, `docked` px
 * to begin with, from the foot of both columns' lists. `settled()` is a browser's next layout: a list
 * grown shorter than its place is clamped to its foot, the sizes are reported, and what was waiting runs.
 * `arrange` is handed the page before it loads, as `newPage`'s is (a session history, `enableHistory`).
 */
async function draftOverThreads(script, docked, { proposed = true, arrange = null } = {}) {
  const data = manyThreads(16);
  const page = newPage(new Map(), script, (p) => {
    p.enableResizeObserver();
    if (arrange) arrange(p);
  });
  page.threadingSupported = true;
  page.threads = data.threads;
  page.messages = data.messages;
  forwardTimeline(page);
  const pending = proposal({ text: LONG_CARD_DRAFT });
  if (proposed) {
    await proposedDuringACall(page, pending);
  } else {
    await startTalking(page);
    await page.settle();
  }
  await showDiscord(page, data.messages);
  const card = page.el("post-confirm");
  const main = page.el("scroll-area");
  const side = page.el("side-scroll");
  const isDocked = () => !card.hidden && card.parentNode === page.el("frame-body");
  card.offsetHeight = docked;
  for (const area of [main, side]) {
    let base = area.clientHeight;
    Object.defineProperty(area, "clientHeight", {
      configurable: true,
      get: () => Math.max(0, base - (isDocked() ? card.offsetHeight : 0)),
      set: (value) => {
        base = Number(value) || 0;
      },
    });
  }
  const settled = async () => {
    for (const box of [main, side]) box.scrollTop = box.scrollTop;
    for (const box of [main, side]) page.resized(box);
    page.expireTimers(0);
    await page.settle();
  };
  await press(page, main, "1", { metaKey: true });
  await settled();
  main.scrollTop = main.scrollHeight;
  await main.dispatch("scroll");
  await settled();
  return { page, pending, card, main, side, isDocked, settled };
}

test("`#269 approval-card-inline` x `#281 view-keys`: CMD-3 AND CMD-4 WHILE A SEND CARD IS UP send nothing, withdraw nothing and hide nothing — the thread list docks the card under its cards, which open at their foot above it, and Main takes it back at its foot, a reader on its newest line following it down", async () => {
  const DOCKED = 60;
  const run = async (script) => {
    const { page, pending, card, main, isDocked, settled } = await draftOverThreads(script, DOCKED);
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const untouched = () => page.proposalCommits.length === 0 && page.proposalCancels.length === 0 &&
      !card.hidden && page.el("post-confirm-text").textContent === pending.text && page.screen() === "main";
    assert.equal(card.parentNode, page.el("pane-discord"), "the premise: the card is the next message of Main");
    assert.ok(atBottomOf(main), "the premise: the reader is on Main's newest line");
    assert.ok(main.scrollHeight > main.clientHeight + 200, "the premise: Main runs past the screen");
    assert.ok(card.outerHeight() - DOCKED > BOTTOM_SLACK_PX + 10,
      "the premise: the card in the list is taller than docked, as a long draft is in Chromium");

    // Cmd-3 with the keys on the card's own Send: the thread list, the card docked under its cards.
    const three = await press(page, page.el("post-confirm-send"), "3", { metaKey: true });
    const listUp = layout() === "list" && three.defaulted === false;
    const docked = isDocked() && card.getAttribute("data-place") === "dock" &&
      card.getAttribute("data-screen") === "main" && !nodeWithinFixture(main, card);
    const listFoot = sideAtFoot(page);
    const noJump = page.el("jump-newest-label").textContent !== "Draft to send";
    await settled();
    const listFootAfter = sideAtFoot(page);
    const untouchedInList = untouched();

    // Cmd-4 on Don't send, in a window too narrow for two columns: the list stays, and the card with it.
    page.wideDesk = false;
    await press(page, page.el("post-confirm-cancel"), "4", { metaKey: true });
    const narrowKept = layout() === "list" && isDocked() && /wider window/.test(page.el("status").textContent) &&
      untouched();

    // Cmd-4 at a desk: two columns, the card back at the foot of Main, followed down to by its reader.
    page.wideDesk = true;
    await press(page, page.el("post-confirm-cancel"), "4", { metaKey: true });
    const two = inTwoColumns(page);
    const backInMain = card.parentNode === page.el("pane-discord") &&
      card.nextSibling === page.el("channel-composer") && !card.hidden;
    const mainFollowed = atBottomOf(main);
    await settled();
    const twoHeld = sideAtFoot(page) && atBottomOf(main) && untouched();

    // Cmd-3 from two columns: the cards stay at their foot, above the card docked again.
    await press(page, focused(page) || main, "3", { metaKey: true });
    const againDocked = layout() === "list" && isDocked() && sideAtFoot(page);
    await settled();
    // Cmd-1: Main in one column, its reader on its newest line following the card down to it.
    await press(page, focused(page) || main, "1", { metaKey: true });
    const oneFollowed = layout() === null && card.parentNode === page.el("pane-discord") && atBottomOf(main);
    await settled();
    return {
      listUp, docked, listFoot, noJump, listFootAfter, untouchedInList, narrowKept, two, backInMain,
      mainFollowed, twoHeld, againDocked, oneFollowed, untouched: untouched(),
    };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, {
    listUp: true, docked: true, listFoot: true, noJump: true, listFootAfter: true, untouchedInList: true,
    narrowKept: true, two: true, backInMain: true, mainFollowed: true, twoHeld: true, againDocked: true,
    oneFollowed: true, untouched: true,
  });

  // THE CONTROLS. The card placed as before the thread list: it stays at the foot of Main, behind the
  // cards, where nobody can see it.
  const behind = await run(brokenScript(
    "  if (screen === \"main\" && !(threadListShown && currentView === \"discord\")) {",
    "  if (screen === \"main\") {"));
  assert.equal(behind.docked, false, "the control docked the card too, so this test cannot see it left behind");
  // Docked only once the cards have been opened: they open at the foot the card then covers. (The
  // dock-growth keeper takes them the rest of the way once the browser reports the sizes.)
  const late = SCRIPT.replace(
    "  placePostCard();\n  leftColumnToMain();\n  if (from === \"two\"",
    "  leftColumnToMain();\n  if (from === \"two\"").replace(
    "  renderSearchInAll();\n  refreshSearchCount();\n}\n\n/**\n * Out of the thread list",
    "  renderSearchInAll();\n  refreshSearchCount();\n  placePostCard();\n}\n\n/**\n * Out of the thread list");
  assert.notEqual(late, SCRIPT, "the late-docking control changed nothing");
  const dockedLate = await run(late);
  assert.deepStrictEqual({ listFoot: dockedLate.listFoot, againDocked: dockedLate.againDocked },
    { listFoot: false, againDocked: true }, "docked after the cards were opened, the thread list still opened at its foot");
  // Main told nothing as the card comes back to its foot: its reader is left above the card.
  const unfollowed = await run(brokenScript(
    "    if (holds && keepReaderPlace(held.place)) return held.place.pinned;\n    if (follows) scrollToNewest();\n    return follows;",
    "    return false;"));
  assert.deepStrictEqual({ mainFollowed: unfollowed.mainFollowed, oneFollowed: unfollowed.oneFollowed },
    { mainFollowed: false, oneFollowed: false }, "the control followed the card down too");
});

test("`#269 approval-card-inline` x `#281 view-keys`: a draft arriving under the thread list docks there and moves the cards no more than the dock growing does — a reader at their foot stays on it, at once and as the card grows, one up them on their card — Main behind it is kept on its newest line, and Send there leaves the keys on the cards", async () => {
  const DOCKED = 60;
  const GROWTH = 80;
  const run = async (script) => {
    const { page, card, main, side, isDocked, settled } = await draftOverThreads(script, DOCKED, { proposed: false });
    await press(page, main, "3", { metaKey: true });
    await settled();
    assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", "Cmd-3 did not show the thread list");
    assert.ok(sideAtFoot(page), "the premise: the thread list opened at its foot");
    assert.ok(atBottomOf(main), "the premise: Main behind the list is at its newest");
    assert.ok(side.scrollHeight > side.clientHeight + 200, "the premise: the cards run past the column's foot");

    // At the foot of the cards: the draft docks under them, and the reader is at their foot at once, in
    // the same task, before the browser reports a size; Main behind them on its newest line too.
    page.propose(proposal({ text: LONG_CARD_DRAFT }));
    await page.settle();
    const docked = isDocked();
    const footAtOnce = sideAtFoot(page) && atBottomOf(main);
    await settled();
    const footKept = sideAtFoot(page) && atBottomOf(main);
    const offered = page.el("jump-newest-label").textContent !== "Draft to send";
    // The card growing with nothing redrawn, as the dock does: both feet kept once the size is reported.
    card.offsetHeight = DOCKED + GROWTH;
    await settled();
    const grewFoot = sideAtFoot(page) && atBottomOf(main);
    card.offsetHeight = DOCKED;
    await settled();
    // Cmd-1: the card at the foot of Main, the reader on its newest line still.
    await press(page, focused(page) || side, "1", { metaKey: true });
    const mainKept = card.parentNode === page.el("pane-discord") && atBottomOf(main);
    await settled();
    await press(page, focused(page) || main, "3", { metaKey: true });
    await settled();

    // Up the cards: the draft taken down and another arriving leave the reader on the very card.
    page.expireTimers(2000);
    page.settleProposal("withdrawn");
    await page.settle();
    await settled();
    side.scrollTop = 100;
    await side.dispatch("scroll");
    page.propose(proposal({ serial: 8, handle: "hAnDlE-second-draft-0001", text: LONG_CARD_DRAFT }));
    await page.settle();
    await settled();
    const upHeld = isDocked() && side.scrollTop === 100;

    // Send under the thread list: posted once, and the keys go to the cards, not behind them.
    page.expireTimers(2000);
    page.el("post-confirm-send").focus();
    await page.el("post-confirm-send").click();
    await page.settle();
    const sent = page.proposalCommits.length === 1 && card.hidden;
    const keysOnCards = nodeWithinFixture(side, focused(page));
    return { docked, footAtOnce, footKept, offered, grewFoot, mainKept, upHeld, sent, keysOnCards };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, {
    docked: true, footAtOnce: true, footKept: true, offered: true, grewFoot: true, mainKept: true, upHeld: true,
    sent: true, keysOnCards: true,
  });

  // THE CONTROLS. The cards not kept as the card comes: off their foot until the browser reports the size.
  const late = await run(brokenScript("  if (cardsFoot) keepCardsAtFoot();\n", ""));
  assert.deepStrictEqual(late, { ...real, footAtOnce: false });
  // The keepers told of the dock alone: the card growing leaves both readers above their foot.
  assert.ok(GROWTH > BOTTOM_SLACK_PX, "the premise: the growth is more than the slack");
  const dockOnly = await run(brokenScript(
    "  return (el(\"dock\").offsetHeight || 0) + (docked ? card.offsetHeight || 0 : 0);",
    "  return el(\"dock\").offsetHeight || 0;"));
  assert.deepStrictEqual(dockOnly, { ...real, grewFoot: false });
  // The keys given to the box of the list behind the thread list, as on the channel.
  const behind = await run(brokenScript(
    "  if (threadListOnScreen()) {\n    focusList(sidePane);\n    return;\n  }\n", ""));
  assert.deepStrictEqual(behind, { ...real, keysOnCards: false });
});

test("`#336 draft-return-thread-list` x `#280 scroll-stability`: A DRAFT ARRIVING WHILE SETTINGS OR HELP IS UP finds a reader who left the thread list from the foot of its cards at their foot again on the way back, over the card docked under them, as one is whom it reaches on the list; one who left up the cards is on their card", async () => {
  // From the final review of #269 approval-card-inline, in Chromium at 393x852 and 1280x800: at the foot of
  // the thread list's cards, to Settings or Help, a draft arriving there, and back, the reader was the card's
  // height above the foot (247px for a long draft). The right column's dock-growth keeper took its last
  // report from the column hidden with the main screen, measured at nothing, and reckoned no growth from it.
  // The fixture measures a hidden column at nothing as a browser does, and keeps its place while hidden and
  // clamps it as it is shown (`clampedAsShownLikeChromium`).
  const DOCKED = 60;
  const run = async (script) => {
    const { page, main, side, isDocked, settled } = await draftOverThreads(script, DOCKED, { proposed: false });
    clampedAsShownLikeChromium(page, side);
    await press(page, main, "3", { metaKey: true });
    await settled();
    assert.equal(page.el("screen-main").getAttribute("data-columns"), "list", "Cmd-3 did not show the thread list");
    assert.ok(side.scrollHeight > side.clientHeight + 200, "the premise: the cards run past the column's foot");
    const shown = side.clientHeight;
    const gap = () => side.scrollHeight - side.scrollTop - side.clientHeight;
    const routes = {
      settings: [() => page.el("open-settings").click(), () => page.el("close-settings").click()],
      help: [() => press(page, side, "?", { shiftKey: true }), () => press(page, focused(page) || side, "Escape")],
    };
    const result = {};
    let serial = 80;
    for (const [name, [open, close]] of Object.entries(routes)) {
      for (const up of [0, 100]) {
        side.scrollTop = side.scrollHeight - side.clientHeight - up;
        await side.dispatch("scroll");
        await settled();
        assert.equal(gap(), up, `the premise: the reader is ${up}px up the cards`);
        const before = cardOnScreen(page);
        await open();
        await page.settle();
        assert.notEqual(page.screen(), "main", `the premise: ${name} is up`);
        side.clientHeight = 0;
        page.resized(side);
        page.expireTimers(0);
        page.propose(proposal({ serial, handle: `hAnDlE-away-draft-00${serial}`, text: LONG_CARD_DRAFT }));
        serial += 1;
        await watchTurns(page);
        assert.ok(isDocked(), `the premise: the draft docked under ${name}`);
        page.expireTimers(2000);
        await close();
        await page.settle();
        side.clientHeight = shown;
        await settled();
        result[`${name}${up}`] = page.screen() === "main" && isDocked() &&
          (up === 0 ? gap() === 0 : cardOnScreen(page) === before);
        page.lapseProposal();
        await watchTurns(page);
        page.expireTimers(sourceConstant("POST_ROOM_KEEP_MS"));
        await settled();
        assert.ok(!isDocked(), "the premise: the draft was taken down before the next trip");
      }
    }
    return result;
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { settings0: true, settings100: true, help0: true, help100: true });

  // THE CONTROL: the keeper's report from the hidden column kept. The reader who left from the foot comes back
  // the card's height above it; the one up the cards is on their card all the same.
  const hiddenKept = await run(brokenScript("    if (currentScreen === \"main\") sideUnderLists = now;\n",
    "    sideUnderLists = now;\n"));
  assert.deepStrictEqual(hiddenKept, { ...real, settings0: false, help0: false });
});

/** The place read in All put back in All once the way out of the thread list has shown it, and not. */
const BACK_IN_ALL = 'const backInAll = next === "one" &&';
const NOT_BACK_IN_ALL = "const backInAll = false &&";

/** `script`, already a broken copy, broken in one more place, which it must contain. */
function brokenFurther(script, find, replace) {
  assert.ok(script.includes(find), `the negative control patches ${JSON.stringify(find)}, which the copy no longer contains`);
  return script.replace(find, replace);
}

test("`#269 approval-card-inline` x `#281 view-keys` x `#280 scroll-stability`: LEAVING THE THREAD LIST FOR ALL with a send card up finds a reader who was at All's foot at its foot, Send on screen — by Esc, where the list was chosen from All, and by Cmd-2", async () => {
  const DOCKED = 60;
  const run = async (script) => {
    const { page, card, main, settled } = await draftOverThreads(script, DOCKED);
    const picker = page.el("thread-select");
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const sendShown = () => {
      const box = page.el("post-confirm-send").getBoundingClientRect();
      return box.height > 0 && box.top >= 0 && box.bottom <= main.clientHeight;
    };
    await press(page, main, "2", { metaKey: true });
    await settled();
    main.scrollTop = main.scrollHeight;
    await main.dispatch("scroll");
    await settled();
    assert.equal(picker.value, "flat", "the premise: Cmd-2 shows All");
    assert.equal(card.parentNode, page.el("pane-discord"), "the premise: the card is All's next message");
    assert.ok(atBottomOf(main) && sendShown(), "the premise: the reader is at All's foot, Send on screen");
    assert.ok(card.outerHeight() - DOCKED > BOTTOM_SLACK_PX + 10,
      "the premise: the card in the list is taller than docked, as a long draft is in Chromium");
    const back = {};
    for (const [name, key, fields] of [["escape", "Escape", {}], ["cmd2", "2", { metaKey: true }]]) {
      await press(page, focused(page) || main, "3", { metaKey: true });
      await settled();
      assert.equal(layout(), "list", `the premise: Cmd-3 shows the thread list (${name})`);
      await press(page, focused(page) || page.el("side-scroll"), key, fields);
      const atOnce = layout() === null && picker.value === "flat" && card.parentNode === page.el("pane-discord") &&
        atBottomOf(main) && sendShown();
      await settled();
      back[name] = atOnce && atBottomOf(main) && sendShown();
    }
    return { ...back, untouched: page.proposalCommits.length === 0 && page.proposalCancels.length === 0 && !card.hidden };
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { escape: true, cmd2: true, untouched: true });

  // THE CONTROL. Main's reader told nothing as the card comes back, before All is decided, and All's own
  // place not put back once it is: All keeps the place Main's reader is at, now above the card, and draws
  // them on the rows they could see, Send below the screen — 369px above All's foot on a phone in
  // Chromium, and 337px at a desk. Either alone puts them back at All's foot.
  const untold = await run(brokenFurther(brokenScript(
    "    if (holds && keepReaderPlace(held.place)) return held.place.pinned;\n    if (follows) scrollToNewest();\n    return follows;",
    "    return false;"), BACK_IN_ALL, NOT_BACK_IN_ALL));
  assert.deepStrictEqual(untold, { ...real, escape: false, cmd2: false });
});

test("`#269 approval-card-inline` x `#281 view-keys` x `#280 scroll-stability`: A MAIN READER UP THE LIST with a send card up stays on their row through Cmd-3 — behind the cards once the sizes are reported, the card's going not taken for the dock growing — and is on it again by Cmd-1, Esc and Cmd-4, a reader the card's going moved included", async () => {
  const DOCKED = 60;
  const run = async (script) => {
    const { page, card, main, settled } = await draftOverThreads(script, DOCKED);
    page.wideDesk = true;
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const inList = card.outerHeight();
    assert.ok(inList - DOCKED > 2 * BOTTOM_SLACK_PX + 20,
      "the premise: the card in the list is taller than docked, as a long draft is in Chromium");
    const result = {};
    // `keeper`: as far up as the card is tall, where the card's going leaves them DOCKED px from the foot,
    // inside the dock-growth keeper's reach (the slack plus the growth it is told of) and moved by nothing
    // else. `moved`: nearer the foot than the card is taller in the list than docked, where the card's
    // going leaves Main too short to keep them where they were.
    for (const [name, up] of [["keeper", inList], ["moved", Math.round((inList - DOCKED) / 2)]]) {
      let behind = true;
      for (const [route, key, fields] of [["One", "1", { metaKey: true }], ["Escape", "Escape", {}], ["Two", "4", { metaKey: true }]]) {
        if (layout() !== null) {
          await press(page, focused(page) || main, "1", { metaKey: true });
          await settled();
        }
        main.scrollTop = main.scrollHeight - main.clientHeight - up;
        await main.dispatch("scroll");
        await settled();
        const before = rowOnScreen(page);
        assert.ok(!atBottomOf(main) && main.scrollHeight - main.scrollTop - main.clientHeight === up,
          `the premise: the reader is ${up}px up Main, not at its foot (${name}, ${route})`);
        await press(page, focused(page) || main, "3", { metaKey: true });
        await settled();
        assert.equal(layout(), "list", "the premise: Cmd-3 shows the thread list");
        behind = behind && rowOnScreen(page) === before;
        await press(page, focused(page) || page.el("side-scroll"), key, fields);
        const atOnce = rowOnScreen(page) === before && card.parentNode === page.el("pane-discord");
        await settled();
        assert.equal(layout(), key === "4" ? "two" : null, `the premise: ${key} left the thread list`);
        result[`${name}${route}`] = atOnce && rowOnScreen(page) === before;
      }
      if (name === "keeper") result.keeperBehind = behind;
    }
    result.untouched = page.proposalCommits.length === 0 && page.proposalCancels.length === 0 && !card.hidden;
    return result;
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, {
    keeperOne: true, keeperEscape: true, keeperTwo: true, keeperBehind: true, movedOne: true, movedEscape: true,
    movedTwo: true, untouched: true,
  });

  // THE CONTROLS. Main's keeper not told the card moved: the card's height going from under Main is taken
  // for the dock growing, and a reader within it of the foot is pulled to the foot behind the cards. The
  // place held from Cmd-3 still puts them back on the way out.
  const unmoved = brokenScript("  if (moving) cardMovedUnderLists(heightUnderLists() - under);\n", "");
  assert.deepStrictEqual(await run(unmoved), { ...real, keeperBehind: false });
  // No place held from Cmd-3: a reader the card's going moved is followed down to the foot as one on it.
  const held = "    if (holds && keepReaderPlace(held.place)) return held.place.pinned;\n";
  assert.deepStrictEqual(await run(brokenScript(held, "")), { ...real, movedOne: false, movedEscape: false, movedTwo: false });
  // Neither, as the card's docking first left it: every one of them found at the foot of Main on the way back.
  assert.deepStrictEqual(await run(unmoved.replace(held, "")), {
    keeperOne: false, keeperEscape: false, keeperTwo: false, keeperBehind: false, movedOne: false, movedEscape: false,
    movedTwo: false, untouched: true,
  });
});

// `#269 approval-card-inline` x `#281 view-keys` x `#280 scroll-stability`, from review. Two ways the thread
// list moved the left column's reader that the card's move did not cover. A draft ARRIVING under the list
// docks there and gives the main screen `data-draft`, which shrinks the box at the foot of Main's list to
// one line in the same layout: in Chromium 192px to 90px as the card took 246px from under the list, and
// Main's dock-growth keeper took readers up to 110px up for readers at its foot. And a reader up All goes
// to Main behind the list, which lacks All's replies, so their place near All's foot was clamped at Main's,
// and the way back read Main's reader as on its newest line and took All to its foot.

/** The parts of a broken copy that put the page back as it was before review's second round. */
const CAME_UNDER = "  if (coming) cardCameUnderLists(gap);\n";
const HELD_ALWAYS = '  listHeldPlace = inColumn(columns.main, () => (selection().view === "main" || selection().view === "flat"\n';
const HELD_WITH_CARD = "  listHeldPlace = inColumn(columns.main, () => (postWatch.shown !== null && " +
  'leftEl("post-confirm").getAttribute("data-place") === "channel" && (selection().view === "main" || selection().view === "flat")\n';

test("`#269 approval-card-inline` x `#281 view-keys` x `#280 scroll-stability`: A DRAFT ARRIVING UNDER THE THREAD LIST leaves a Main reader up the list on their row, behind the cards and back by Cmd-1, Esc and Cmd-4, offered the jump — the box at Main's foot shrinking to one line in the same layout not taken for their being at the foot — and keeps one at Main's foot at it", async () => {
  const DOCKED = 200;
  // web/voice.css's box at the foot of the channel's list: whole, and one line under a draft.
  const BOX = { whole: 192, draft: 90 };
  const run = async (script) => {
    const { page, card, main, settled } = await draftOverThreads(script, DOCKED, { proposed: false });
    page.wideDesk = true;
    page.el("channel-composer").height = () => (page.el("screen-main").hasAttribute("data-draft") ? BOX.draft : BOX.whole);
    await settled();
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const gap = () => main.scrollHeight - main.scrollTop - main.clientHeight;
    const sendShown = () => {
      const box = page.el("post-confirm-send").getBoundingClientRect();
      return box.height > 0 && box.top >= 0 && box.bottom <= main.clientHeight;
    };
    const behind = {};
    const back = {};
    let serial = 30;
    // `near` and `up`: both within the keeper's reach as it reckoned it, the slack and the card's height,
    // once the box has given up its room; `near` just past the slack.
    for (const [name, up] of [["foot", 0], ["near", BOTTOM_SLACK_PX + 16], ["up", 100]]) {
      assert.ok(up + DOCKED - (BOX.whole - BOX.draft) <= BOTTOM_SLACK_PX + DOCKED,
        `the premise: the ${name} reader is left within the slack and the growth of Main's foot`);
      for (const [route, key, fields] of [["One", "1", { metaKey: true }], ["Escape", "Escape", {}], ["Two", "4", { metaKey: true }]]) {
        if (layout() !== null) {
          await press(page, focused(page) || main, "1", { metaKey: true });
          await settled();
        }
        main.scrollTop = main.scrollHeight - main.clientHeight - up;
        await main.dispatch("scroll");
        await settled();
        const before = rowOnScreen(page);
        assert.ok(card.hidden && gap() === up, `the premise: no card, and the reader ${up}px up Main (${name}, ${route})`);
        await press(page, focused(page) || main, "3", { metaKey: true });
        await settled();
        assert.equal(layout(), "list", "the premise: Cmd-3 shows the thread list");
        serial += 1;
        page.propose(proposal({ serial, handle: `hAnDlE-arriving-${String(serial).padStart(8, "0")}`, text: LONG_CARD_DRAFT }));
        await page.settle();
        assert.ok(!card.hidden && card.parentNode === page.el("frame-body") && page.el("screen-main").hasAttribute("data-draft"),
          "the premise: the draft docks under the cards, and the box at Main's foot gives up its room");
        await settled();
        behind[`${name}${route}`] = up === 0 ? atBottomOf(main) : rowOnScreen(page) === before;
        await press(page, focused(page) || page.el("side-scroll"), key, fields);
        await settled();
        assert.equal(layout(), key === "4" ? "two" : null, `the premise: ${key} left the thread list`);
        assert.equal(card.parentNode, page.el("pane-discord"), "the premise: the card is back at the foot of Main");
        back[`${name}${route}`] = up === 0 ? atBottomOf(main) && sendShown() : rowOnScreen(page) === before && jumpShown(page);
        // The draft taken down, and the box given back its room a second later.
        page.settleProposal("cancelled");
        await page.settle();
        page.expireTimers(1000);
        await settled();
        assert.ok(card.hidden && !page.el("screen-main").hasAttribute("data-draft"), "the premise: the draft and its room are gone");
      }
    }
    return { behind, back, untouched: page.proposalCommits.length === 0 && page.proposalCancels.length === 0 };
  };
  const every = (value) => Object.fromEntries(["foot", "near", "up"].flatMap((name) =>
    ["One", "Escape", "Two"].map((route) => [`${name}${route}`, typeof value === "function" ? value(name) : value])));
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { behind: every(true), back: every(true), untouched: true });

  // THE CONTROLS. Main's keeper not told where its reader was before the card came: the box's room going
  // in the same layout leaves the near and up readers within the slack and the growth of the foot, and
  // they are pulled down to it behind the cards. The place held from Cmd-3 still puts them back.
  const told = brokenScript(CAME_UNDER, "");
  assert.deepStrictEqual(await run(told), { ...real, behind: every((name) => name === "foot") });
  // As the page was before: no place held from Cmd-3 without a card either, and on the way back they
  // follow the card down from the foot they were pulled to.
  const before = brokenFurther(brokenFurther(told, HELD_ALWAYS, HELD_WITH_CARD), BACK_IN_ALL, NOT_BACK_IN_ALL);
  assert.deepStrictEqual(await run(before), {
    behind: every((name) => name === "foot"), back: every((name) => name === "foot"), untouched: true,
  });
});

test("`#269 approval-card-inline` x `#281 view-keys` x `#280 scroll-stability`: LEAVING THE THREAD LIST FOR ALL finds a reader who was up All on their row, card or no card — by Esc, where the list was chosen from All, and by Cmd-2 — though Main behind the list lacks All's replies, and offers the jump to a card below them", async () => {
  const UPS = [BOTTOM_SLACK_PX + 6, 80, 120, 200];
  const run = async (script, proposed) => {
    const { page, card, main, settled } = await draftOverThreads(script, 60, { proposed });
    const picker = page.el("thread-select");
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const kept = {};
    for (const up of UPS) {
      for (const [route, key, fields] of [["Escape", "Escape", {}], ["Two", "2", { metaKey: true }]]) {
        if (layout() !== null || picker.value !== "flat") {
          await press(page, focused(page) || main, "2", { metaKey: true });
          await settled();
        }
        main.scrollTop = main.scrollHeight - main.clientHeight - up;
        await main.dispatch("scroll");
        await settled();
        const before = rowOnScreen(page);
        assert.ok(picker.value === "flat" && main.scrollHeight - main.scrollTop - main.clientHeight === up,
          `the premise: the reader ${up}px up All`);
        assert.equal(!card.hidden && card.parentNode === page.el("pane-discord"), proposed,
          "the premise: the card is All's next message, or none is up");
        await press(page, focused(page) || main, "3", { metaKey: true });
        await settled();
        assert.equal(layout(), "list", "the premise: Cmd-3 shows the thread list");
        await press(page, focused(page) || page.el("side-scroll"), key, fields);
        const atOnce = rowOnScreen(page) === before;
        await settled();
        assert.ok(layout() === null && picker.value === "flat", `the premise: ${key} showed All`);
        kept[`${up}${route}`] = atOnce && rowOnScreen(page) === before && (!proposed || jumpShown(page));
      }
    }
    return kept;
  };
  const all = (value) => Object.fromEntries(UPS.flatMap((up) =>
    ["Escape", "Two"].map((route) => [`${up}${route}`, typeof value === "function" ? value(up) : value])));
  const real = { card: await run(SCRIPT, true), none: await run(SCRIPT, false) };
  assert.deepStrictEqual(real, { card: all(true), none: all(true) });

  // THE CONTROL. All's place not put back in All: whoever Main's place clamped at Main's foot is taken to
  // All's foot — with a card, every reader but one whose rows Main still holds above its own foot, and
  // without one, every reader.
  const unkept = brokenScript(BACK_IN_ALL, NOT_BACK_IN_ALL);
  assert.deepStrictEqual({ card: await run(unkept, true), none: await run(unkept, false) },
    { card: all((up) => up === 200), none: all(false) });
});

// `#280 scroll-stability` x `#269 approval-card-inline` x `#281 view-keys`, from review's third round. A send
// card docked under the thread list takes its height from the foot of the cards, so their column is that
// much shorter there than two columns' right column, where the card is back at the foot of Main. In Chromium
// at a desk, the long draft's card 246px tall, every reader 60 to 300px up the list's cards was at their foot
// in two columns — the one 300px up 10px from it, inside the slack — and Ctrl+3 put them at the list's foot,
// whether the list had been opened from Main or from two columns.

/** The first of the cards on screen, and where it stands: "id@top". */
const cardOnScreen = (page) => {
  const card = firstCardInView(page);
  const side = page.el("side-scroll");
  return card ? `${card.getAttribute("data-id")}@${card.getBoundingClientRect().top - side.getBoundingClientRect().top}` : null;
};

test("`#280 scroll-stability` x `#269 approval-card-inline` x `#281 view-keys`: A READER UP THE THREAD LIST'S CARDS with a send card docked under them is on their card again by Cmd-4 and Cmd-3, the list opened from Main or from two columns — at the cards' foot in two columns only because the card leaves their column, or within the slack of it — and one who moved in two columns is placed where they moved to", async () => {
  const DOCKED = 150;
  const run = async (script) => {
    const { page, card, main, side, isDocked, settled } = await draftOverThreads(script, DOCKED);
    page.wideDesk = true;
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const key = async (digit) => {
      await press(page, focused(page) || main, digit, { metaKey: true });
      await settled();
    };
    const gap = () => side.scrollHeight - side.scrollTop - side.clientHeight;
    const scrollUp = async (up) => {
      side.scrollTop = side.scrollHeight - side.clientHeight - up;
      await side.dispatch("scroll");
      await settled();
      assert.equal(gap(), up, `the premise: the reader is ${up}px up the cards`);
    };
    const toList = async (from) => {
      if (from === "Main" && layout() !== null) await key("1");
      if (from === "Two" && layout() !== "two") await key("4");
      assert.equal(layout(), from === "Main" ? null : "two", `the premise: the list is opened from ${from}`);
      await key("3");
      assert.ok(layout() === "list" && isDocked(), "the premise: Cmd-3 shows the thread list, the card docked under it");
    };
    const result = {};
    // `near` and `mid`: nearer the foot than the card is tall, at the cards' foot in two columns. `slack`: 10px
    // from it there, inside the slack, and read there as at it. `far`: up the cards in two columns too.
    for (const [name, up] of [["near", 40], ["mid", 100], ["slack", DOCKED + 10], ["far", DOCKED + 100]]) {
      for (const from of ["Main", "Two"]) {
        await toList(from);
        await scrollUp(up);
        const before = cardOnScreen(page);
        await key("4");
        assert.ok(inTwoColumns(page) && card.parentNode === page.el("pane-discord"),
          "the premise: Cmd-4 shows two columns, the card back in Main");
        assert.equal(gap(), Math.max(0, up - DOCKED), `the premise: two columns' cards ${up - DOCKED}px from their foot`);
        await press(page, focused(page) || main, "3", { metaKey: true });
        const atOnce = layout() === "list" && cardOnScreen(page) === before;
        await settled();
        result[`${name}${from}`] = atOnce && cardOnScreen(page) === before;
      }
    }
    // A reader who moves the cards in two columns: where they are there, not where they were in the list.
    await toList("Main");
    await scrollUp(40);
    await key("4");
    await scrollUp(200);
    const moved = cardOnScreen(page);
    await press(page, focused(page) || main, "3", { metaKey: true });
    const movedAtOnce = cardOnScreen(page) === moved;
    await settled();
    result.moved = movedAtOnce && cardOnScreen(page) === moved && gap() === 200 + DOCKED;
    result.untouched = page.proposalCommits.length === 0 && page.proposalCancels.length === 0 && !card.hidden;
    return result;
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, {
    nearMain: true, nearTwo: true, midMain: true, midTwo: true, slackMain: true, slackTwo: true, farMain: true,
    farTwo: true, moved: true, untouched: true,
  });

  // THE CONTROLS. No place held in the list's cards: whoever two columns read as at the foot of the cards is
  // put at the list's foot. The reader up the cards in two columns too is kept as before.
  const unheld = await run(brokenScript(
    "  if (place && kept && now === \"list\" && next === \"two\") listCardsHeld = { place: kept, placed: cardAnchor(), channel };\n",
    ""));
  assert.deepStrictEqual(unheld, {
    ...real, nearMain: false, nearTwo: false, midMain: false, midTwo: false, slackMain: false, slackTwo: false,
  });
  // The right column's keeper not told the cards were placed: the card docked again under the list is taken
  // for the dock growing, and anyone within the slack plus its height of the list's foot is pulled to it.
  // The reader further up, and the one who moved 200px up two columns' cards, 350px up the list's, are kept.
  assert.ok(DOCKED + BOTTOM_SLACK_PX < DOCKED + 100, "the premise: the far reader is beyond the keeper's reach");
  const untold = await run(brokenScript("    cardsPlacedUnderLists();\n", ""));
  assert.deepStrictEqual(untold, {
    ...real, nearMain: false, nearTwo: false, midMain: false, midTwo: false, slackMain: false, slackTwo: false,
  });
  // The held place put back whether or not the reader moved the cards in two columns.
  const always = await run(brokenScript("function sameCardPlace(one, other) {\n", "function sameCardPlace(one, other) {\n  return true;\n"));
  assert.deepStrictEqual(always, { ...real, moved: false });
});

// `#280 scroll-stability` x `#269 approval-card-inline`, from review's fourth round. A second after the send
// card goes, the main screen gives the box at the foot of the channel's list back the room it gave up for the
// card (`POST_ROOM_KEEP_MS`): in Chromium the box grows from 90px to 192px. Nothing in the page followed a
// reader at the list's foot through that; Chromium's own scroll anchoring did, until the list was redrawn in
// that second — the message the card sent arriving by the live feed, a re-read, the poll — which left them
// 142px above the foot with the jump up, no longer following the channel. With the browser's anchoring off,
// it happened every time the card went. And the card going with the focus inside it, a mouse or the keys in
// use, hands the focus to the box, which opens to its typing height at once: 42px more in Chromium, followed
// by nothing either. This fixture has no scroll anchoring at all.

/** web/voice.css's box at the foot of the channel's list in Chromium: whole, one line under a draft, and
 * open for typing under a draft. */
const CHANNEL_BOX = { whole: 192, draft: 90, typing: 132 };

test("`#280 scroll-stability` x `#269 approval-card-inline`: THE BOX TAKING BACK ITS ROOM a second after the send card goes keeps a reader on the newest line of Main or All on it, and one up the list on their row — the card gone from the channel, from under the thread list with Cmd-1 or Esc pressed within that second, and with the focus on its More, which hands the focus to the box", async () => {
  const run = async (script) => {
    const { page, card, main, settled } = await draftOverThreads(script, 60);
    // The page reads the list's layout as the card leaves it (`placeChipsOverComposer`), and a browser
    // clamps a list grown shorter than its place to its foot there, before anything else the card's going
    // does: before the box takes the focus.
    let hidden = card.hidden;
    Object.defineProperty(card, "hidden", {
      configurable: true,
      get: () => hidden,
      set: (value) => {
        hidden = Boolean(value);
        main.scrollTop = main.scrollTop;
      },
    });
    const roomy = () => page.el("screen-main").hasAttribute("data-draft");
    const typing = () => focused(page) === page.el("channel-compose-text");
    page.el("channel-composer").height = () =>
      (roomy() ? (typing() ? CHANNEL_BOX.typing : CHANNEL_BOX.draft) : CHANNEL_BOX.whole);
    await settled();
    const picker = page.el("thread-select");
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const gap = () => main.scrollHeight - main.scrollTop - main.clientHeight;
    let serial = 40;
    const result = {};
    const routes = [
      ["Main", "1", null, false], ["All", "2", null, false], ["ListOne", "1", "1", false],
      ["ListEscape", "2", "Escape", false], ["MainMore", "1", null, true], ["AllMore", "2", null, true],
    ];
    for (const [route, view, back, more] of routes) {
      for (const name of ["foot", "up"]) {
        if (card.hidden) {
          serial += 1;
          page.propose(proposal({ serial, handle: `hAnDlE-room-${String(serial).padStart(8, "0")}`, text: LONG_CARD_DRAFT }));
          await page.settle();
          await settled();
        }
        if (layout() !== null || picker.value !== (view === "1" ? "main" : "flat")) {
          await press(page, focused(page) || main, view, { metaKey: true });
          await settled();
        }
        assert.ok(layout() === null && card.parentNode === page.el("pane-discord") && roomy(),
          `the premise: the card is the next message of ${view === "1" ? "Main" : "All"}, the box one line under it`);
        // `up`: further up than the card is tall, so that the card's going leaves them where they were.
        const up = name === "foot" ? 0 : card.outerHeight() + 100;
        main.scrollTop = main.scrollHeight - main.clientHeight - up;
        await main.dispatch("scroll");
        await settled();
        assert.equal(gap(), up, `the premise: the reader is ${up}px up the list (${route})`);
        const before = rowOnScreen(page);
        if (back) {
          await press(page, focused(page) || main, "3", { metaKey: true });
          await settled();
          assert.ok(layout() === "list" && card.parentNode === page.el("frame-body"), "the premise: Cmd-3 docks the card");
        }
        // The keys in use, the focus left on the card's More.
        if (more) page.el("post-confirm-more").focus();
        page.settleProposal("committed");
        await page.settle();
        await settled();
        assert.ok(card.hidden && roomy(), `the premise: the card is gone and its room kept a moment (${route})`);
        assert.equal(typing(), more, `the premise: the box has the focus exactly where the card took it with it (${route})`);
        if (back) {
          await press(page, focused(page) || page.el("side-scroll"), back, back === "Escape" ? {} : { metaKey: true });
          await settled();
          assert.ok(layout() === null && roomy(), `the premise: ${back} left the thread list within the second`);
        }
        const kept = name === "foot" ? gap() === 0 : rowOnScreen(page) === before;
        assert.equal(page.expireTimers(1000), 1, "the premise: the room is taken back a second after the card goes");
        assert.ok(!roomy(), "the premise: the box has its room again");
        await settled();
        result[`${name}${route}`] = kept && (name === "foot" ? gap() === 0 && !jumpShown(page) : rowOnScreen(page) === before);
        if (more) {
          page.el("channel-compose-text").blur();
          await settled();
        }
      }
    }
    return result;
  };
  const every = (value) => Object.fromEntries(["foot", "up"].flatMap((name) =>
    ["Main", "All", "ListOne", "ListEscape", "MainMore", "AllMore"]
      .map((route) => [`${name}${route}`, typeof value === "function" ? value(name, route) : value])));
  assert.ok(CHANNEL_BOX.whole - CHANNEL_BOX.typing > BOTTOM_SLACK_PX && CHANNEL_BOX.typing - CHANNEL_BOX.draft > BOTTOM_SLACK_PX,
    "the premise: the box grows by more than the slack, as it takes the focus and as it takes back its room");
  assert.deepStrictEqual(await run(SCRIPT), every(true));

  // THE CONTROLS. The room taken back with nothing holding the reader: the box grows under the one at the
  // foot, who is left its growth above it, past the slack, the jump up. The reader up the list is not moved.
  const unheld = await run(brokenScript(
    '      if (postWatch.shown === null) holdingReader(() => main.toggleAttribute("data-draft", false));\n',
    '      if (postWatch.shown === null) main.toggleAttribute("data-draft", false);\n'));
  assert.deepStrictEqual(unheld, every((name) => name === "up"));
  // The focus handed to the box with nothing holding the reader: the one at the foot is left the box's
  // opening above it, past the slack, and is no longer on the newest line when the room comes back.
  const unparked = await run(brokenScript(
    "  holdingReader(() => target.focus({ preventScroll: true }));\n", "  target.focus({ preventScroll: true });\n"));
  assert.deepStrictEqual(unparked, every((name, route) => name === "up" || !route.endsWith("More")));
});

// `#269 approval-card-inline` x `#280 scroll-stability`, from review's fourth round. A long draft opened by
// More is taller than a phone's list — in Chromium 907px in a list 739px tall — so a reader near its foot
// sees the card and no row at all. The place a trip through the thread list puts back, and a switch between
// Main and All keeps, was the rows on screen, and theirs had none: Cmd-3 and Esc brought a reader 40px up the
// foot, Send on screen, back 995px above it, the card 919px lower and Send off screen, and one 200px up was
// taken to the foot; Cmd-2 and Cmd-1 took both to the foot.

test("`#269 approval-card-inline` x `#280 scroll-stability`: A READER WHOSE SCREEN SHOWS ONLY THE SEND CARD, a long draft opened by More, finds it where it stood — through the thread list and back by Esc, by the view's own key and by Cmd-4, and across a switch between Main and All", async () => {
  // Thirty lines of words in the fixture's layout: as More lays a long draft out, taller than the list.
  const TALL_DRAFT = "The release notes for this week, as agreed, with every target named. ".repeat(18);
  const run = async (script) => {
    const { page, card, main, settled } = await draftOverThreads(script, 60);
    page.wideDesk = true;
    page.propose(proposal({ serial: 60, handle: "hAnDlE-tall-0000000060", text: TALL_DRAFT }));
    await page.settle();
    await page.el("post-confirm-more").click();
    await settled();
    const picker = page.el("thread-select");
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const gap = () => main.scrollHeight - main.scrollTop - main.clientHeight;
    const cardTop = () => card.getBoundingClientRect().top - main.getBoundingClientRect().top;
    const sendShown = () => {
      const box = page.el("post-confirm-send").getBoundingClientRect();
      return box.height > 0 && box.top >= 0 && box.bottom <= main.clientHeight;
    };
    assert.ok(!card.hasAttribute("data-folded") && card.outerHeight() > main.clientHeight + 100,
      "the premise: the draft is opened, and the card is taller than the list");
    const result = {};
    const routes = [
      ["MainEscape", "1", ["3", "Escape"]], ["MainOne", "1", ["3", "1"]], ["MainTwo", "1", ["3", "4"]],
      ["AllEscape", "2", ["3", "Escape"]], ["AllTwo", "2", ["3", "2"]], ["MainToAll", "1", ["2"]], ["AllToMain", "2", ["1"]],
    ];
    const toView = async (view) => {
      if (layout() !== null || picker.value !== (view === "1" ? "main" : "flat")) {
        await press(page, focused(page) || main, view, { metaKey: true });
        await settled();
      }
    };
    for (const [route, view, keys] of routes) {
      for (const up of [40, 150]) {
        // A switch between Main and All goes to a view this page holds: left at its foot first, so that
        // where it was left is no help.
        if (keys.length === 1) {
          await toView(keys[0]);
          main.scrollTop = main.scrollHeight;
          await main.dispatch("scroll");
          await settled();
        }
        await toView(view);
        assert.ok(layout() === null && card.parentNode === page.el("pane-discord"),
          `the premise: the card is the next message of ${view === "1" ? "Main" : "All"} (${route})`);
        main.scrollTop = main.scrollHeight - main.clientHeight - up;
        await main.dispatch("scroll");
        await settled();
        const before = cardTop();
        assert.ok(gap() === up && rowOnScreen(page) === null && before < 0,
          `the premise: the reader ${up}px up sees the card and no row (${route}, ${rowOnScreen(page)})`);
        const sent = sendShown();
        for (const key of keys) {
          await press(page, focused(page) || (layout() === "list" ? page.el("side-scroll") : main), key,
            key === "Escape" ? {} : { metaKey: true });
          await settled();
        }
        assert.ok(layout() === (keys[keys.length - 1] === "4" ? "two" : null) && card.parentNode === page.el("pane-discord"),
          `the premise: the card is back at the foot of the left column (${route})`);
        result[`${route}${up}`] = cardTop() === before && sendShown() === sent;
      }
    }
    // A reader who can see a row is placed by the rows, as before, card or no card: All's last row, a
    // thread's reply, with the card under it, and Main, which lacks that reply and was left at its foot,
    // put back at its foot by Cmd-1 rather than held on the card.
    await toView("1");
    main.scrollTop = main.scrollHeight;
    await main.dispatch("scroll");
    await settled();
    await toView("2");
    const rows = page.el("discord-log").children.filter((li) => li.getAttribute("data-id"));
    const reply = rows[rows.length - 1];
    main.scrollTop = reply.offsetTop() + 1;
    await main.dispatch("scroll");
    await settled();
    const deep = (node) => [node.textContent, ...node.children.map(deep)].join(" ");
    assert.ok(String(rowOnScreen(page)).startsWith(`${reply.getAttribute("data-id")}@`) &&
      /thread \d+ answer/.test(deep(reply)) && cardTop() > 0 && cardTop() < main.clientHeight,
      `the premise: All's last row, a reply, and the card under it on screen (${rowOnScreen(page)})`);
    await toView("1");
    result.byRow = picker.value === "main" && gap() === 0;
    result.untouched = page.proposalCommits.length === 0 && page.proposalCancels.length === 0 && !card.hidden;
    return result;
  };
  const real = await run(SCRIPT);
  const every = (value) => ({
    ...Object.fromEntries(["MainEscape", "MainOne", "MainTwo", "AllEscape", "AllTwo", "MainToAll", "AllToMain"]
      .flatMap((route) => [40, 150].map((up) => [`${route}${up}`, typeof value === "function" ? value(route, up) : value]))),
    byRow: true,
    untouched: true,
  });
  assert.deepStrictEqual(real, every(true));

  // THE CONTROLS. The card not a place: the reader who sees only the card is put wherever the rows above
  // it leave them, or at the foot.
  const unplaced = await run(brokenScript(
    '  const card = typeof place.card === "number" ? channelCard() : null;\n', "  const card = null;\n"));
  assert.deepStrictEqual(unplaced, every(false));
  // The card a place whatever else is on screen: the reader who could see All's reply is held on the card.
  const always = await run(brokenScript(
    "  const card = seen.length === 0 ? channelCard() : null;\n", "  const card = channelCard();\n"));
  assert.deepStrictEqual(always, { ...real, byRow: false });
});

// `#269 approval-card-inline` x `#280 scroll-stability`, from review's fifth round. A reader near the foot of a
// long draft opened by More sees the card, Send on screen, and no row. They tap Send, or the draft is settled
// somewhere else, and the card goes. Chromium's own scroll anchoring then held the box under the card where it
// stood, and that left a reader 30 to 80px up Main or All, or to 100px up the call's view, exactly as far above
// the foot as they had been, past the slack. A second later the box took back its room with them held on their
// row, and the jump came with its room. They ended 132 to 210px above the foot, offered the jump, no longer
// following the list. This fixture has no scroll anchoring, and its clamp alone takes every such reader to the
// foot, as a browser with none does. So the test lays Chromium's anchoring on, as far as these readers meet it.

/** web/voice.css's box at the foot of the call's transcript in Chromium at 393x852: whole, and one line under
 * a draft. */
const VOICE_BOX = { whole: 206, draft: 96 };

/**
 * Chromium's scroll anchoring, as far as a reader at the foot of a list that ends in the send card meets it.
 * The first row of the list on screen is the anchor. With no row on screen, the box under the card is the
 * anchor while any of it is on screen. A change made through the returned function keeps the anchor where it
 * stood, and then the list is clamped, as a browser clamps it.
 */
function anchoredLikeChromium(main, rows, box) {
  const onScreen = (node) => {
    const r = node.getBoundingClientRect();
    return r.height > 0 && r.bottom > 0 && r.top < main.clientHeight;
  };
  return (mutate) => {
    const anchor = rows().find(onScreen) || (onScreen(box()) ? box() : null);
    const before = anchor ? anchor.getBoundingClientRect().top : 0;
    mutate();
    main.scrollTop += anchor ? anchor.getBoundingClientRect().top - before : 0;
  };
}

/**
 * Make every change the send card makes to the list it ends through `anchored`: its going, its moves into the
 * dock under the thread list and back, and the frozen card the call's view draws just above it as a tap sends.
 */
function anchorCardChanges(page, card, anchored) {
  let hidden = card.hidden;
  Object.defineProperty(card, "hidden", {
    configurable: true,
    get: () => hidden,
    set: (value) => {
      if (Boolean(value) !== hidden) anchored(() => { hidden = Boolean(value); });
    },
  });
  const transcript = page.el("transcript");
  const append = transcript.append.bind(transcript);
  transcript.append = (...kids) => anchored(() => append(...kids));
  for (const id of ["frame-body", "pane-discord", "pane-voice"]) {
    const parent = page.el(id);
    const insert = parent.insertBefore.bind(parent);
    parent.insertBefore = (node, ref) => (node === card ? anchored(() => insert(node, ref)) : insert(node, ref));
  }
}

test("`#269 approval-card-inline` x `#280 scroll-stability`: A READER WHO SEES ONLY THE SEND CARD AS IT GOES, a long draft opened by More, is on the newest line, and still there with no jump once the box takes back its room — Send tapped and the draft settled elsewhere, in Main, in All and on the call's view — though the browser's anchoring holds the box under the card where it stood; and a reader up the list is on their row", async () => {
  // Thirty lines of words in the fixture's layout: as More lays a long draft out, taller than the list.
  const TALL_DRAFT = "The release notes for this week, as agreed, with every target named. ".repeat(18);
  const run = async (script, { anchoring = true } = {}) => {
    const { page, card, main, settled } = await draftOverThreads(script, 60);
    const roomy = () => page.el("screen-main").hasAttribute("data-draft");
    page.el("channel-composer").height = () => (roomy() ? CHANNEL_BOX.draft : CHANNEL_BOX.whole);
    page.el("voice-composer").height = () => (roomy() ? VOICE_BOX.draft : VOICE_BOX.whole);
    const onCall = () => page.tab() === "voice";
    const anchored = anchoring
      ? anchoredLikeChromium(main,
        () => page.el(onCall() ? "transcript" : "discord-log").children.filter((li) => !li.hidden),
        () => page.el(onCall() ? "voice-composer" : "channel-composer"))
      : (mutate) => {
        mutate();
        main.scrollTop = main.scrollTop;
      };
    anchorCardChanges(page, card, anchored);
    const picker = page.el("thread-select");
    const gap = () => main.scrollHeight - main.scrollTop - main.clientHeight;
    const rowSeen = () => {
      const row = page.el(onCall() ? "transcript" : "discord-log").children
        .find((li) => !li.hidden && li.getBoundingClientRect().bottom > 0 && li.getBoundingClientRect().top < main.clientHeight);
      return row ? `${row.getAttribute("data-id") || row.className}@${row.getBoundingClientRect().top}` : null;
    };
    const toView = async (view) => {
      if (view === "call") {
        if (!onCall()) await page.el("view-switch").click();
      } else {
        if (onCall()) await page.el("view-switch").click();
        if (picker.value !== (view === "1" ? "main" : "flat")) await press(page, focused(page) || main, view, { metaKey: true });
      }
      await settled();
    };
    let serial = 70;
    const result = {};
    for (const view of ["1", "2", "call"]) {
      for (const how of ["Tap", "Elsewhere"]) {
        for (const up of [40, 80, "row"]) {
          await toView(view);
          if (card.hidden || page.el("post-confirm-text").textContent !== TALL_DRAFT) {
            serial += 1;
            page.propose(proposal({ serial, handle: `hAnDlE-alone-${String(serial).padStart(8, "0")}`, text: TALL_DRAFT }));
            await page.settle();
            page.expireTimers(2000);
            await settled();
          }
          if (card.hasAttribute("data-folded")) await page.el("post-confirm-more").click();
          await settled();
          const where = `${view === "call" ? "the call's view" : view === "1" ? "Main" : "All"}, ${how}, ${up}`;
          assert.ok(!card.hidden && !card.hasAttribute("data-folded") && card.parentNode === page.el(onCall() ? "pane-voice" : "pane-discord") &&
            card.outerHeight() > main.clientHeight + 100 && roomy() && !page.el("post-confirm-send").disabled,
          `the premise: the draft opened by More is the list's next message, taller than the list, Send ready (${where})`);
          // `row`: further up than the card is tall, so that the card's going leaves them where they were.
          main.scrollTop = main.scrollHeight - main.clientHeight - (up === "row" ? card.outerHeight() + 100 : up);
          await main.dispatch("scroll");
          await settled();
          const before = rowSeen();
          const send = page.el("post-confirm-send").getBoundingClientRect();
          assert.ok(up === "row"
            ? before !== null
            : gap() === up && up > BOTTOM_SLACK_PX && before === null && card.getBoundingClientRect().top < 0 &&
              send.top >= 0 && send.bottom <= main.clientHeight,
          `the premise: the reader ${up === "row" ? "sees a row" : `${up}px up sees the card, Send on screen, and no row`} (${where}, ${before})`);
          if (how === "Tap") await page.el("post-confirm-send").click();
          else page.settleProposal("committed");
          await page.settle();
          await settled();
          assert.ok(card.hidden && roomy(), `the premise: the card is gone and its room kept a moment (${where})`);
          const gone = up === "row" ? rowSeen() === before : gap() === 0;
          assert.equal(page.expireTimers(1000), 1, `the premise: the room is taken back a second after the card goes (${where})`);
          await settled();
          assert.ok(!roomy(), `the premise: the box has its room again (${where})`);
          result[`${view === "call" ? "Call" : view === "1" ? "Main" : "All"}${how}${up}`] =
            gone && (up === "row" ? rowSeen() === before : gap() === 0 && !jumpShown(page));
        }
      }
    }
    result.sent = page.proposalCommits.length === 9 && page.proposalCancels.length === 0;
    return result;
  };
  const every = (value) => ({
    ...Object.fromEntries(["Main", "All", "Call"].flatMap((view) => ["Tap", "Elsewhere"].flatMap((how) =>
      [40, 80, "row"].map((up) => [`${view}${how}${up}`, typeof value === "function" ? value(up) : value])))),
    sent: true,
  });
  assert.deepStrictEqual(await run(SCRIPT), every(true));

  // THE CONTROLS. The card's going with nothing to put the reader who saw only it on the newest line: the
  // anchoring leaves each of them as far above the foot as they were, and the room coming back leaves them
  // further. The reader up the list is not moved.
  const SENT_AWAY = "    if (alone) inColumn(columns.main, scrollToNewest);\n";
  const unplaced = brokenScript(SENT_AWAY, "");
  assert.deepStrictEqual(await run(unplaced), every((up) => up === "row"));
  // ...which a browser with no anchoring hides: its clamp takes each of them to the foot. The defect is the
  // anchoring's, and the test sees it only with the anchoring laid on.
  assert.deepStrictEqual(await run(unplaced, { anchoring: false }), every(true));
  // Every reader of a list the card ends taken to the newest line as it goes, whatever they could see: the
  // reader up the list is moved off their row.
  const always = await run(brokenScript("  return box.top <= top + 0.5 && box.bottom > top;\n", "  return true;\n"));
  assert.deepStrictEqual(always, every((up) => up !== "row"));
});

// ...and from the dock under the thread list. The place a trip through the list puts back is read as the list
// comes up (`listHeldPlace`), and for a reader who saw only the card it is the card. Gone from under the cards
// by the way back, Send tapped there or the draft settled elsewhere, it put nobody anywhere: in Chromium Esc or
// Cmd+1 left a reader who had been 40 or 60px up Main 182 or 202px above its foot, and Cmd+2 353px above All's,
// offered the jump and no longer following.

test("`#269 approval-card-inline` x `#280 scroll-stability`: A READER WHO SAW ONLY THE SEND CARD AS THE THREAD LIST CAME, a long draft opened by More, is on the newest line on the way back by Esc, Cmd-1 and Cmd-2 once the card has gone from under the cards — Send tapped there, or the draft settled elsewhere — and still there with no jump once the box takes back its room", async () => {
  const TALL_DRAFT = "The release notes for this week, as agreed, with every target named. ".repeat(18);
  const run = async (script) => {
    const { page, card, main, side, settled } = await draftOverThreads(script, 60);
    const roomy = () => page.el("screen-main").hasAttribute("data-draft");
    page.el("channel-composer").height = () => (roomy() ? CHANNEL_BOX.draft : CHANNEL_BOX.whole);
    // The card leaving the foot of Main or All for the dock: Chromium's anchoring holds the box under it where
    // it stood, and the reader as far above the foot as they were.
    anchorCardChanges(page, card, anchoredLikeChromium(main,
      () => page.el("discord-log").children.filter((li) => !li.hidden), () => page.el("channel-composer")));
    const picker = page.el("thread-select");
    const layout = () => page.el("screen-main").getAttribute("data-columns");
    const gap = () => main.scrollHeight - main.scrollTop - main.clientHeight;
    const key = async (name) => {
      await press(page, focused(page) || (layout() === "list" ? side : main), name, name === "Escape" ? {} : { metaKey: true });
      await settled();
    };
    let serial = 90;
    const result = {};
    for (const view of ["1", "2"]) {
      for (const how of ["Tap", "Elsewhere"]) {
        for (const back of ["Escape", "1", "2"]) {
          if (layout() !== null || picker.value !== (view === "1" ? "main" : "flat")) await key(view);
          if (card.hidden || page.el("post-confirm-text").textContent !== TALL_DRAFT) {
            serial += 1;
            page.propose(proposal({ serial, handle: `hAnDlE-listed-${String(serial).padStart(8, "0")}`, text: TALL_DRAFT }));
            await page.settle();
            page.expireTimers(2000);
            await settled();
          }
          if (card.hasAttribute("data-folded")) await page.el("post-confirm-more").click();
          await settled();
          const where = `${view === "1" ? "Main" : "All"}, ${how}, back by ${back}`;
          main.scrollTop = main.scrollHeight - main.clientHeight - 40;
          await main.dispatch("scroll");
          await settled();
          assert.ok(!card.hidden && card.parentNode === page.el("pane-discord") && card.outerHeight() > main.clientHeight + 100 &&
            gap() === 40 && rowOnScreen(page) === null && card.getBoundingClientRect().top < 0,
          `the premise: the reader 40px up sees the opened card and no row (${where}, ${rowOnScreen(page)})`);
          await key("3");
          assert.ok(layout() === "list" && !card.hidden && card.parentNode === page.el("frame-body") &&
            !page.el("post-confirm-send").disabled, `the premise: Cmd-3 docks the card under the cards, Send ready (${where})`);
          if (how === "Tap") await page.el("post-confirm-send").click();
          else page.settleProposal("committed");
          await page.settle();
          await settled();
          assert.ok(card.hidden && roomy() && layout() === "list", `the premise: the card is gone from under the cards (${where})`);
          await key(back);
          const to = back === "Escape" ? view : back;
          assert.ok(layout() === null && picker.value === (to === "1" ? "main" : "flat"),
            `the premise: ${back} left the list for ${to === "1" ? "Main" : "All"} within the second (${where})`);
          const kept = gap() === 0;
          assert.equal(page.expireTimers(1000), 1, `the premise: the room is taken back a second after the card goes (${where})`);
          await settled();
          assert.ok(!roomy(), `the premise: the box has its room again (${where})`);
          result[`${view === "1" ? "Main" : "All"}${how}${back}`] = kept && gap() === 0 && !jumpShown(page);
        }
      }
    }
    result.sent = page.proposalCommits.length === 6 && page.proposalCancels.length === 0;
    return result;
  };
  const every = (value) => ({
    ...Object.fromEntries(["Main", "All"].flatMap((view) => ["Tap", "Elsewhere"].flatMap((how) =>
      ["Escape", "1", "2"].map((back) => [`${view}${how}${back}`, typeof value === "function" ? value(view, how, back) : value])))),
    sent: true,
  });
  assert.deepStrictEqual(await run(SCRIPT), every(true));

  // THE CONTROL. The place read on a card that has since gone put back nowhere: the reader who came from Main
  // is where the card's going to the dock left Main behind the cards, as far above its foot as they were, and
  // not on its newest line. From All, the list draws Main behind its cards on
  // the rows All's reader could see once the card had gone to the dock, which here are All's newest replies
  // over a root Main ends on, so Main is at its foot behind the list and the way back finds the foot without
  // the place; in Chromium Main stood up its list there, and point 18 of tests/approval_card_browser.py takes
  // a reader from All that way.
  const unplaced = await run(brokenScript(
    '  if (typeof place.card === "number" && postWatch.shown === null) {\n', "  if (false) {\n"));
  assert.deepStrictEqual(unplaced, every((view) => view === "All"));
});

// `#269 approval-card-inline` x `#280 scroll-stability`, from review of the rebase onto #304 back-gesture-in-app.
// Two columns draw the left column at half the window's width, and the send card at the foot of its list
// with it: a long draft opened by More was 519px tall in All at 1280x800 in Chromium and 580px beside a
// thread. The change of layout read the reader's place only once the column had narrowed, so a reader at
// the foot of All, card open, read as 61px above it, on two replies Main lacks: Ctrl+4 put Main back where
// it was last left, 326px above its foot, Send off screen, and Ctrl+2 left All 436px above its foot. A
// reader at the foot of Main, whose view no change switches, was left 61px above it as the card grew.

/**
 * `draftOverThreads` at a desk wide enough for two columns, with a draft of 316 characters opened by More
 * on the card. In one column a line of its words holds the fixture's 40 characters, and in two columns'
 * left column 30, so it is 8 lines in one and 11 in two: 60px taller, past the slack.
 */
async function openedDraftColumns(script) {
  const DRAFT = "The release notes, as agreed: the runner fix landed and the nightly is green. ".repeat(4);
  const parts = await draftOverThreads(script, 60);
  const { page, card, main, settled } = parts;
  page.wideDesk = true;
  page.propose(proposal({ serial: 61, handle: "hAnDlE-wrap-000000000061", text: DRAFT }));
  await page.settle();
  const layout = () => page.el("screen-main").getAttribute("data-columns");
  page.el("post-confirm-text").wrapAt = () => (layout() === "two" ? 30 : CHARS_PER_LINE);
  await page.el("post-confirm-more").click();
  await settled();
  assert.ok(!card.hasAttribute("data-folded") && card.parentNode === page.el("pane-discord"),
    "the premise: the draft is opened, the next message of the channel's list");
  const picker = page.el("thread-select");
  const key = async (digit) => {
    await press(page, focused(page) || main, digit, { metaKey: true });
    await settled();
  };
  return {
    ...parts,
    layout,
    key,
    opened: card.outerHeight(),
    gap: () => main.scrollHeight - main.scrollTop - main.clientHeight,
    sendShown: () => {
      const box = page.el("post-confirm-send").getBoundingClientRect();
      const top = main.getBoundingClientRect().top;
      return box.height > 0 && box.top >= top && box.bottom <= top + main.clientHeight;
    },
    toView: async (view) => {
      if (layout() !== null || picker.value !== (view === "1" ? "main" : "flat")) await key(view);
    },
    /** The reader with message `id`'s row `offset` px above the top of the list. */
    onRow: async (id, offset) => {
      const row = page.el("discord-log").children.find((li) => li.getAttribute("data-id") === id);
      main.scrollTop = row.offsetTop() + offset;
      await main.dispatch("scroll");
      await settled();
    },
  };
}

test("`#269 approval-card-inline` x `#280 scroll-stability`: TWO COLUMNS COMING AND GOING, the send card at the foot of the left column drawn at their narrower width, keep a reader at the foot of Main or All at its foot with Send on screen, by Cmd-4 and back, Main held from a visit up the list; and one up either on their row", async () => {
  const run = async (script) => {
    const { page, card, main, settled, layout, key, opened, gap, sendShown, toView, onRow } = await openedDraftColumns(script);
    const result = {};
    for (const [name, view] of [["Main", "1"], ["All", "2"]]) {
      // At the foot, Main held from a visit up its list, so that where it was left is no help.
      await toView("1");
      main.scrollTop = main.scrollHeight - main.clientHeight - 200;
      await main.dispatch("scroll");
      await settled();
      await toView(view);
      main.scrollTop = main.scrollHeight;
      await main.dispatch("scroll");
      await settled();
      const seen = rowOnScreen(page);
      assert.ok(gap() === 0 && sendShown() && (view === "1" || /^\d*[13579]@/.test(String(seen))),
        `the premise: a reader at ${name}'s foot with Send on screen, in All on a reply Main lacks (${seen})`);
      await key("4");
      assert.ok(layout() === "two" && card.outerHeight() - opened > BOTTOM_SLACK_PX,
        `the premise: two columns, the card more than the slack taller in their left column (${card.outerHeight()} from ${opened})`);
      const two = gap() === 0 && sendShown() && page.el("jump-newest").hidden;
      await key(view);
      result[`foot${name}`] = two && layout() === null && gap() === 0 && sendShown() && page.el("jump-newest").hidden;

      // Up the list, on a thread's root, which Main draws too: on it, where it stood, in two columns and back.
      await toView(view);
      await onRow("326", 5);
      const before = rowOnScreen(page);
      assert.ok(before === "326@-5" && gap() > BOTTOM_SLACK_PX, `the premise: a reader up ${name} on a root (${before}, ${gap()}px up)`);
      await key("4");
      const kept = rowOnScreen(page) === before && layout() === "two";
      await key(view);
      result[`up${name}`] = kept && layout() === null && rowOnScreen(page) === before;
    }
    result.untouched = page.proposalCommits.length === 0 && page.proposalCancels.length === 0 && !card.hidden;
    return result;
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { footMain: true, upMain: true, footAll: true, upAll: true, untouched: true });

  // THE CONTROLS. The place read once the layout has changed: the card has grown under the reader at Main's
  // foot, who is left its growth above it, Send off screen. The one at All's foot reads as up All on a reply
  // Main lacks, and is at Main's foot only as a reader who could see the card and none of Main's rows.
  const READ_BEFORE = '  const left = screen.getAttribute("data-columns") !== want ? leftColumnPlace() : null;\n';
  const unread = await run(brokenScript(READ_BEFORE, "  const left = null;\n"));
  assert.deepStrictEqual(unread, { ...real, footMain: false });
  // As the page was before, neither: Main stays where it was left, and All is drawn as far up on the way back.
  const before = await run(brokenFurther(brokenScript(READ_BEFORE, "  const left = null;\n"),
    "    if (keepPlace && !keepReaderPlace(place) && place.cardSeen) scrollToNewest();\n",
    "    if (keepPlace) keepReaderPlace(place);\n"));
  assert.deepStrictEqual(before, { ...real, footMain: false, footAll: false });
  // The place read before, but not put back where the left column's view stays: All's reader is kept by the
  // switch to Main, and Main's is left as the card grows.
  const unkept = await run(brokenScript('  if (left && next !== "list") keepLeftColumnPlace(left, leftBack);\n', ""));
  assert.deepStrictEqual(unkept, { ...real, footMain: false });
});

// `#269 approval-card-inline` x `#280 scroll-stability`, from the same review. A change of layout keeps the
// place it reads, but not always where it goes: Main lacks All's replies, so a reader up All near its foot is
// at Main's foot in two columns, and the card is taller in their narrower left column, so a reader near
// Main's foot there is at its foot in one. The way back read them as on the newest line and took them to the
// foot: in Chromium a reader 40px up All with a long draft opened by More, and 30px up with no draft at all.
// And the reader up All, Main held from an earlier visit, was put where that visit left Main: 326px above its
// foot in Chromium, Send off screen.

test("`#269 approval-card-inline` x `#280 scroll-stability`: BACK FROM A CHANGE OF LAYOUT THAT COULD NOT KEEP THEIR PLACE, the left column's reader is where they were — up All near its foot, the card in view, at Main's foot by Cmd-4, Main held up its list, and on their row by Cmd-2, up Main near its foot in two columns by Cmd-1 and Cmd-4 — unless they moved in between; and Cmd-1 takes the reader up All to Main's foot as well", async () => {
  const run = async (script) => {
    const { page, card, main, settled, layout, key, gap, sendShown, toView, onRow } = await openedDraftColumns(script);
    const result = {};
    const cardInView = () => {
      const box = card.getBoundingClientRect();
      const top = main.getBoundingClientRect().top;
      return box.bottom > top && box.top < top + main.clientHeight;
    };
    // Main left up its list, so that where it was left is no help.
    await toView("1");
    main.scrollTop = main.scrollHeight - main.clientHeight - 200;
    await main.dispatch("scroll");
    await settled();
    // Up All on its last reply, which Main lacks, the card in view: at Main's foot in two columns, which is
    // only geometry, Send on screen.
    await toView("2");
    await onRow("331", 5);
    const upAll = rowOnScreen(page);
    assert.ok(upAll === "331@-5" && gap() > BOTTOM_SLACK_PX && cardInView(),
      `the premise: a reader up All on a reply, the card in view (${upAll}, ${gap()}px up)`);
    await key("4");
    result.allAway = layout() === "two" && gap() === 0 && sendShown();
    await key("2");
    result.allBack = layout() === null && rowOnScreen(page) === upAll;
    // ...and by Cmd-1, in one column, Main left up its list again.
    await toView("1");
    main.scrollTop = main.scrollHeight - main.clientHeight - 200;
    await main.dispatch("scroll");
    await settled();
    await toView("2");
    await onRow("331", 5);
    await key("1");
    result.allToMain = layout() === null && gap() === 0 && sendShown();

    // Up Main in two columns nearer its foot than the card grows there: at Main's foot in one column.
    await key("1");
    await key("4");
    main.scrollTop = main.scrollHeight - main.clientHeight - 40;
    await main.dispatch("scroll");
    await settled();
    const upMain = rowOnScreen(page);
    assert.ok(layout() === "two" && gap() === 40, `the premise: a reader 40px up Main in two columns (${upMain})`);
    await key("1");
    assert.ok(layout() === null && gap() === 0, `the premise: at Main's foot in one column, the card shorter (${gap()}px)`);
    await key("4");
    result.mainBack = layout() === "two" && rowOnScreen(page) === upMain;

    // One who moves between the two is placed where they moved to.
    await key("2");
    await onRow("331", 5);
    await key("4");
    main.scrollTop = main.scrollHeight - main.clientHeight - 100;
    await main.dispatch("scroll");
    await settled();
    const moved = rowOnScreen(page);
    await key("2");
    result.moved = layout() === null && rowOnScreen(page) === moved && gap() > BOTTOM_SLACK_PX;
    return result;
  };
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, { allAway: true, allBack: true, allToMain: true, mainBack: true, moved: true });

  // THE CONTROLS. No place held for the way back: both are taken to the foot they were left at.
  const unheld = await run(brokenScript(
    "  const leftBack = left && heldLeft && heldLeft.from === next && sameLeftPlace(left, heldLeft.placed) ? heldLeft.left : null;\n",
    "  const leftBack = null;\n"));
  assert.deepStrictEqual(unheld, { ...real, allBack: false, mainBack: false });
  // A switch that finds none of the reader's rows leaving Main where it was last left, the card in view or
  // not: 200px up its list, Send off screen, by Cmd-4 and by Cmd-1. The way back still finds All as it was.
  const left = await run(brokenScript(
    "    if (keepPlace && !keepReaderPlace(place) && place.cardSeen) scrollToNewest();\n",
    "    if (keepPlace) keepReaderPlace(place);\n"));
  assert.deepStrictEqual(left, { ...real, allAway: false, allToMain: false });
});

test("`#269 approval-card-inline` x `#280 scroll-stability`: BACK FROM SETTINGS, a reader of a long draft opened by More is where they were — at the foot of Main, of All and of the call's view with Send on screen, and one who saw only the card on it where it stood — by the gear or through Help, and back by Back or Esc, though the browser lays the list out as the screen is shown; with no card they were already", async () => {
  // Thirty lines of words in the fixture's layout: as More lays a long draft out, taller than the list.
  const TALL_DRAFT = "The release notes for this week, as agreed, with every target named. ".repeat(18);
  const routes = { gearBack: [false, "back"], gearEsc: [false, "esc"], helpBack: [true, "back"], helpEsc: [true, "esc"] };
  const run = async (script, drafted = true) => {
    const { page, card, main, settled } = await draftOverThreads(script, 60, { proposed: drafted });
    clampedAsShownLikeChromium(page, main);
    if (drafted) {
      page.propose(proposal({ serial: 70, handle: "hAnDlE-tall-0000000070", text: TALL_DRAFT }));
      await page.settle();
      await page.el("post-confirm-more").click();
      await settled();
      assert.ok(!card.hasAttribute("data-folded") && card.outerHeight() > main.clientHeight + 100,
        "the premise: the draft is opened, and the card is taller than the list");
    }
    const picker = page.el("thread-select");
    const gap = () => main.scrollHeight - main.scrollTop - main.clientHeight;
    const cardTop = () => card.getBoundingClientRect().top - main.getBoundingClientRect().top;
    const sendShown = () => {
      const box = page.el("post-confirm-send").getBoundingClientRect();
      return !card.hidden && box.height > 0 && box.top >= 0 && box.bottom <= main.clientHeight;
    };
    const place = () => ({ gap: gap(), row: rowOnScreen(page), card: drafted ? cardTop() : null, send: sendShown() });
    const trip = async ([throughHelp, back]) => {
      await page.el("open-settings").click();
      await settled();
      assert.ok(page.screen() === "settings" && (!drafted || card.parentNode === page.el("frame-body")),
        `the premise: Settings is up${drafted ? ", the card docked under it" : ""}`);
      if (throughHelp) {
        await page.el("open-help").click();
        await settled();
        assert.equal(page.screen(), "help", "the premise: Help is up");
        if (back === "back") await page.el("close-help").click();
        else await press(page, focused(page), "Escape");
        await settled();
        assert.equal(page.screen(), "settings", "the premise: Help went back to Settings");
      }
      if (back === "back") await page.el("close-settings").click();
      else await press(page, focused(page), "Escape");
      await settled();
      assert.equal(page.screen(), "main", "the premise: the main screen is back");
    };
    const result = {};
    for (const [view, digit] of [["Main", "1"], ["All", "2"]]) {
      for (const up of [0, 40, 150]) {
        for (const [route, how] of Object.entries(routes)) {
          if (picker.value !== (digit === "1" ? "main" : "flat")) {
            await press(page, focused(page) || main, digit, { metaKey: true });
            await settled();
          }
          main.scrollTop = main.scrollHeight - main.clientHeight - up;
          await main.dispatch("scroll");
          await settled();
          const before = place();
          if (drafted) {
            assert.ok(card.parentNode === page.el("pane-discord") && before.gap === up &&
              (up === 0 ? before.send : before.row === null && before.card < 0),
              `the premise: a reader ${up}px up ${view} ${up === 0 ? "with Send on screen" : "who sees the card and no row"} (${route})`);
          } else {
            assert.ok(before.gap === up && before.row !== null, `the premise: a reader ${up}px up ${view} on a row (${route})`);
          }
          await trip(how);
          result[`${view}${up}${route}`] = JSON.stringify(place()) === JSON.stringify(before) &&
            (!drafted || card.parentNode === page.el("pane-discord"));
        }
      }
    }
    // The call's view: the card is the transcript's next message, and its reader at the foot is at it again.
    await page.el("view-switch").click();
    fillTranscript(page, 4);
    await settled();
    for (const [route, how] of Object.entries(routes)) {
      main.scrollTop = main.scrollHeight;
      await main.dispatch("scroll");
      await settled();
      const before = place();
      assert.ok(page.el("pane-discord").hidden && before.gap === 0 && main.scrollTop > 0 &&
        (!drafted || (card.parentNode === page.el("pane-voice") && before.send)),
        `the premise: a reader at the foot of the call's view${drafted ? ", Send on screen" : ""} (${route})`);
      await trip(how);
      result[`call${route}`] = gap() === 0 && sendShown() === before.send;
    }
    result.untouched = page.proposalCommits.length === 0 && page.proposalCancels.length === 0 &&
      card.hidden === !drafted;
    return result;
  };
  const cases = [
    ...["Main", "All"].flatMap((view) => [0, 40, 150].flatMap((up) => Object.keys(routes).map((route) => `${view}${up}${route}`))),
    ...Object.keys(routes).map((route) => `call${route}`),
  ];
  const every = (value) => ({ ...Object.fromEntries(cases.map((name) => [name, value(name)])), untouched: true });
  const real = await run(SCRIPT);
  assert.deepStrictEqual(real, every(() => true));

  // THE CONTROLS. With no card the model moves nobody: every reader was where they were before this.
  assert.deepStrictEqual(await run(SCRIPT, false), every(() => true));
  // The card put back only once the main screen is shown: every reader is moved, at the foot and seeing
  // only the card alike, on every way back.
  const late = await run(brokenScript('  if (name === "main" && previous !== "main") postCardBackToMain();\n', ""));
  assert.deepStrictEqual(late, every(() => false));
});

// `#269 approval-card-inline` x `#280 scroll-stability`, from the final review. A trip to Help opened by its key,
// "?" or Ctrl+/ (Ctrl+? since `#344 search-shortcut`), still moved a reader who had not moved, on a narrow screen with a keyboard, when the long draft
// opened by More was settled elsewhere, sent from under Help or replaced while Help was up. In Chromium at
// 393x852 a reader at the foot of Main came back 58px above it, by Esc, by Help's Back or by the browser's Back,
// and 200px above it with the jump offered once the box took back its room. showScreen showed the main screen
// and only then hid Help's list, which held the focus after the key, and Help's Back, which held it after a
// click, and only after that showed the control pane. Chromium blurs a control hidden while it holds the focus
// on the spot, and lays the page out to do it, so the list was laid out there for the first time since it was
// hidden: its reader's place came back clamped to a window 66px taller than its own. With the card still up
// the list was no shorter than the reader's place and nothing stayed moved; with the card gone it was shorter,
// and the clamp stood. Settings' Back and Esc let go of the focus before anything is shown, and never did this.

/**
 * A browser's own handling of a list on a screen hidden and shown again, exact about WHEN the list is laid out
 * (`clampedAsShownLikeChromium` lays it out as the screen is shown). Its place is kept while the screen is
 * hidden, and put back clamped to the list and its window as they stand the first time the page is laid out
 * after the screen is shown: the page reading or moving the list, or Chromium blurring a control the page hides
 * while the focus is in it, which Chromium does on the spot and lays the page out first to do. And the list's
 * window is `pane` px taller while #control-pane is hidden, the pane's row giving its room to the body.
 */
function laidOutLikeChromium(page, main, pane) {
  const screen = page.el("screen-main");
  const controls = page.el("control-pane");
  const position = Object.getOwnPropertyDescriptor(FakeElement.prototype, "scrollTop");
  const room = Object.getOwnPropertyDescriptor(main, "clientHeight") ||
    Object.getOwnPropertyDescriptor(FakeElement.prototype, "clientHeight");
  let hidden = screen.hidden;
  let due = false;
  const layout = () => {
    if (!due) return;
    due = false;
    position.set.call(main, position.get.call(main));
  };
  Object.defineProperty(main, "clientHeight", {
    configurable: true,
    get: () => {
      layout();
      return room.get.call(main) + (controls.hidden ? pane : 0);
    },
    set: (value) => room.set.call(main, value),
  });
  Object.defineProperty(main, "scrollTop", {
    configurable: true,
    get: () => {
      layout();
      return position.get.call(main);
    },
    set: (value) => {
      if (hidden) return;
      layout();
      position.set.call(main, value);
    },
  });
  Object.defineProperty(screen, "hidden", {
    configurable: true,
    get: () => hidden,
    set: (value) => {
      if (hidden && !value) due = true;
      hidden = Boolean(value);
    },
  });
  // Everything else showScreen hides, any of which may hold the focus as it does.
  for (const id of ["screen-signin", "screen-settings", "screen-help", "close-settings", "close-help"]) {
    const node = page.el(id);
    let off = node.hidden;
    Object.defineProperty(node, "hidden", {
      configurable: true,
      get: () => off,
      set: (value) => {
        off = Boolean(value);
        if (off && nodeWithinFixture(node, FakeElement.focused)) {
          layout();
          FakeElement.focused = null;
        }
      },
    });
  }
}

test("`#269 approval-card-inline` x `#280 scroll-stability`: BACK FROM HELP OPENED BY ITS KEY, a reader of a long draft opened by More who did not move is on the newest line — at the foot of Main, of All or of the call's view, or seeing only the card — when the draft is settled elsewhere, sent from under Help or replaced by a short one while Help is up, by Esc, by Help's Back and by the browser's Back, and still there with no jump once the box takes back its room; though Chromium lays the page out as it blurs a control hidden with the focus in it", async () => {
  // Thirty lines of words in the fixture's layout: as More lays a long draft out, taller than the list.
  const TALL_DRAFT = "The release notes for this week, as agreed, with every target named. ".repeat(18);
  const SHORT_DRAFT = "on it, back in ten";
  // What the control pane's row gives the list on a 393x852 phone in Chromium while the pane is hidden.
  const PANE = 66;
  const HELP = [["?", "Esc"], ["?", "Back"], ["?", "Browser"]];
  // Main's reader at its foot by the other key too (Ctrl+/ until `#344 search-shortcut`).
  const SLASH = [["Ctrl+?", "Esc"], ["Ctrl+?", "Back"], ["Ctrl+?", "Browser"]];
  // Settings by the gear and its Back, which lets go of the focus before anything is shown.
  const GEAR = [["gear", "Back"]];
  const readers = [["Main", 0], ["Main", 40], ["Main", 150], ["All", 0], ["All", 40], ["All", 150], ["Call", 0]];
  const changes = ["Elsewhere", "Sent", "Shorter"];
  const tripsOf = (view, up) => [...HELP, ...(view === "Main" && up === 0 ? SLASH : []), ...(up === 0 ? GEAR : [])];
  const run = async (script) => {
    const { page, card, main, settled } = await draftOverThreads(script, 60, { arrange: (p) => p.enableHistory() });
    laidOutLikeChromium(page, main, PANE);
    const roomy = () => page.el("screen-main").hasAttribute("data-draft");
    page.el("channel-composer").height = () => (roomy() ? CHANNEL_BOX.draft : CHANNEL_BOX.whole);
    page.el("voice-composer").height = () => (roomy() ? VOICE_BOX.draft : VOICE_BOX.whole);
    const onCall = () => page.tab() === "voice";
    const picker = page.el("thread-select");
    const gap = () => main.scrollHeight - main.scrollTop - main.clientHeight;
    const sendShown = () => {
      const box = page.el("post-confirm-send").getBoundingClientRect();
      return !card.hidden && box.height > 0 && box.top >= 0 && box.bottom <= main.clientHeight;
    };
    // A click takes the focus, in Chromium, before the control it lands on acts.
    const tap = async (id) => {
      page.el(id).focus();
      await page.el(id).click();
    };
    let serial = 80;
    const draft = async (text) => {
      serial += 1;
      page.propose(proposal({ serial, handle: `hAnDlE-helpkey-${String(serial).padStart(8, "0")}`, text }));
      await page.settle();
    };
    const toView = async (view) => {
      if ((view === "Call") !== onCall()) await page.el("view-switch").click();
      await settled();
      if (view !== "Call" && picker.value !== (view === "Main" ? "main" : "flat")) {
        await press(page, focused(page) || main, view === "Main" ? "1" : "2", { metaKey: true });
        await settled();
      }
    };
    const result = {};
    for (const [view, up] of readers) {
      for (const change of changes) {
        for (const [key, back] of tripsOf(view, up)) {
          const where = `${view} ${up}px up, ${change}, ${key} and ${back}`;
          await toView(view);
          if (card.hidden || page.el("post-confirm-text").textContent !== TALL_DRAFT) {
            await draft(TALL_DRAFT);
            page.expireTimers(2000);
            await settled();
          }
          if (card.hasAttribute("data-folded")) await page.el("post-confirm-more").click();
          await settled();
          assert.ok(!card.hidden && !card.hasAttribute("data-folded") &&
            card.parentNode === page.el(onCall() ? "pane-voice" : "pane-discord") &&
            card.outerHeight() > main.clientHeight + 100 && !page.el("post-confirm-send").disabled,
          `the premise: the draft opened by More is the list's next message, taller than the list, Send ready (${where})`);
          main.scrollTop = main.scrollHeight - main.clientHeight - up;
          await main.dispatch("scroll");
          await settled();
          assert.ok(gap() === up && (up === 0 ? sendShown() : rowOnScreen(page) === null && card.getBoundingClientRect().top < 0),
            `the premise: the reader ${up === 0 ? "at the foot, Send on screen" : `${up}px up sees the card and no row`} (${where})`);
          if (key === "gear") await tap("open-settings");
          else await press(page, focused(page) || main, "?", key === "?" ? { shiftKey: true } : { ctrlKey: true, shiftKey: true });
          await settled();
          const away = key === "gear" ? "settings" : "help";
          assert.ok(page.screen() === away && card.parentNode === page.el("frame-body") && !card.hidden,
            `the premise: ${away} is up, the card docked under it (${where})`);
          if (change === "Elsewhere") page.settleProposal("committed");
          else if (change === "Sent") await page.el("post-confirm-send").click();
          else await draft(SHORT_DRAFT);
          await page.settle();
          await settled();
          assert.ok(change === "Shorter" ? !card.hidden && page.el("post-confirm-text").textContent === SHORT_DRAFT : card.hidden,
            `the premise: the draft is ${change === "Shorter" ? "replaced by a short one" : "gone"} while ${away} is up (${where})`);
          if (back === "Esc") await press(page, focused(page), "Escape");
          else if (back === "Back") await tap(key === "gear" ? "close-settings" : "close-help");
          else await page.browserBack();
          await settled();
          assert.equal(page.screen(), "main", `the premise: the main screen is back (${where})`);
          // The short draft: its reader at the foot with its Send on screen, and then it goes too.
          const now = change === "Shorter" ? gap() <= BOTTOM_SLACK_PX && sendShown() : gap() === 0;
          if (change === "Shorter") {
            page.settleProposal("committed");
            await page.settle();
            await settled();
          }
          assert.ok(card.hidden && roomy(), `the premise: the card is gone and its room kept a moment (${where})`);
          assert.equal(page.expireTimers(1000), 1, `the premise: the room is taken back a second after the card goes (${where})`);
          await settled();
          assert.ok(!roomy(), `the premise: the box has its room again (${where})`);
          result[`${view}${up}${change}${key}${back}`] = now && gap() === 0 && !jumpShown(page);
        }
      }
    }
    result.untouched = page.proposalCancels.length === 0 &&
      page.proposalCommits.length === readers.reduce((sum, [view, up]) => sum + tripsOf(view, up).length, 0);
    return result;
  };
  const cases = readers.flatMap(([view, up]) => changes.flatMap((change) =>
    tripsOf(view, up).map(([key, back]) => [`${view}${up}${change}${key}${back}`, key])));
  const every = (value) => ({ ...Object.fromEntries(cases.map(([name, key]) => [name, value(key)])), untouched: true });
  assert.deepStrictEqual(await run(SCRIPT), every(() => true));

  // THE CONTROL. The main screen shown before what holds the focus is hidden and before the control pane is,
  // as before this: every trip to Help moves its reader, whatever went from under Help and whichever way back.
  // A trip by the gear and Settings' Back is kept even so, since Settings lets go of the focus before anything
  // is shown and nothing lays the list out before the pane is back: the model clamps where Chromium lays out.
  const first = await run(brokenScript(
    "    if (screen !== name) el(`screen-${screen}`).hidden = true;\n",
    "    el(`screen-${screen}`).hidden = screen !== name;\n"));
  assert.deepStrictEqual(first, every((key) => key === "gear"));
});

// --- `#473 replication-c7-server-versions`: the page orders copies by the server's versions --------
//
// The replication model's C7. The server says how new each copy it serves is (`observed_ms`) and, on a
// newest page, the stretch whose absence proves a message gone (`settled_below_ms`). The page decides by
// those where both sides carry them, instead of by when it sent the reads: so a page answered from older
// state takes away nothing observed after its read began, with or without `#466`'s interim grace.

/** Every timeline answer of `page` stamped as the server stamps it, its read begun at `clock.at()`. */
function serverVersioned(page, clock) {
  const serve = page.timeline;
  page.timeline = async (path, options) => {
    const answer = await serve(path, options);
    if (!answer || !answer.ok) return answer;
    const body = JSON.parse(await answer.text());
    const url = new URL(path, "http://fixture.test");
    const newest = !url.searchParams.get("before") && !url.searchParams.get("after");
    const observed = clock.at();
    body.observed_ms = observed;
    if (newest) body.settled_below_ms = observed - 120000;
    for (const held of body.messages || []) {
      if (typeof held.observed_ms !== "number") held.observed_ms = observed;
    }
    return json(answer.status, body);
  };
}

/** The text of message `id` as the page keeps it for this channel. */
const storedText = (page, id) => {
  const kept = savedScopes(page)[scopeKey(CHANNEL.id)];
  const copy = kept && kept.messages.find((m) => m.id === id);
  return copy ? copy.content : undefined;
};

/** `#473`: the page deciding by read-send order alone, as before C7. */
const SERVER_VERSIONS_IGNORED = { file: "src/core/channels/versions.ts",
  find: "export function serverVersions(payload: unknown): ServerVersions | null {\n",
  replace: "export function serverVersions(payload: unknown): ServerVersions | null {\n  return null;\n" };

test("`#473 replication-c7-server-versions`: a newest page answered from older state cannot take away a reply observed after its read began, without the interim grace", async () => {
  const run = async (script) => {
    const clock = { now: Date.parse("2026-10-23T12:00:00Z"), at() { return this.now; } };
    const { page, data } = await freshMessagePage((p) => {
      p.repliesInThread = true;
      serverVersioned(p, clock);
    }, script);
    const posted = clock.now + 5000;
    const mine = message({ id: "9473000000000000001", content: "my answer to the fresh message", observed_ms: posted,
      thread: { id: data.lone.id, root_message_id: "210", is_root: false, reply_count: null, reply_count_exact: false } });
    page.replyResponse = async () => json(200, { posted: mine, parts: [mine] });
    await replyButton(rowOf(page, "210")).click();
    await page.el("reply-text").setValue("my answer to the fresh message");
    await page.el("reply-send").click();
    await page.settle();
    // The service answers from a memory a minute old, which says so: its reads began before the post.
    clock.now = posted - 60000;
    await deliver(page, page.stream(), sseMessage(mine, { self_posted: true }));
    await page.settle();
    const afterEcho = shownIds(page).includes(mine.id);
    await reReadChannel(page);
    const afterReread = shownIds(page).includes(mine.id);
    return { page, mine, clock, afterEcho, afterReread };
  };
  const noGrace = brokenAlso(SCRIPT, OWN_POST_FORGOTTEN);
  const { page, mine, clock, afterEcho, afterReread } = await run(noGrace);
  assert.ok(afterEcho, "a page whose read began before the post took the reply away");
  assert.ok(afterReread, "the next stale page took it away");
  await pickThread(page, "main");
  assert.equal(tagOf(rowOf(page, "210")).text, "Thread(1)", "the message answered went back to starting a thread");
  // A newest page begun after the reply was observed, that still lacks it, says it is gone.
  await pickThread(page, "flat");
  clock.now = mine.observed_ms + 60000;
  await reReadChannel(page);
  assert.ok(!shownIds(page).includes(mine.id), "a page begun after the reply that lacks it did not take it away");

  // THE CONTROL: deciding by read-send order, with the interim grace gone too, the stale page takes it away.
  const control = await run(await brokenModule([SERVER_VERSIONS_IGNORED], noGrace));
  assert.ok(!(control.afterEcho && control.afterReread),
    "without the server's versions the reply still survived, so this test cannot see them");
});

test("`#473 replication-c7-server-versions`: an edit the server observed is not replaced by an earlier one, nor a message in the unsettled band dropped", async () => {
  const clock = { now: Date.parse("2026-10-23T12:00:00Z"), at() { return this.now; } };
  const { page, data } = await freshMessagePage((p) => serverVersioned(p, clock));
  const target = page.messages.find((held) => held.id === "210");
  // A read sees the edit; a later page answered from a memory older than the edit carries the old text
  // with an older edit time.
  page.messages = page.messages.map((held) => (held === target
    ? { ...held, content: "edited text", edited_timestamp: "2026-10-23T11:59:00Z" } : held));
  await pollOnce(page);
  assert.equal(storedText(page, "210"), "edited text", "the premise is the edit in the store");
  page.messages = page.messages.map((held) => (held.id === "210"
    ? { ...held, content: "older text", edited_timestamp: "2026-10-23T11:58:00Z" } : held));
  clock.now += 60000;
  await pollOnce(page);
  assert.equal(storedText(page, "210"), "edited text", "an earlier edit replaced a later one");
  // A message posted a moment ago, which a newest page does not list yet: in the band, not gone.
  const fresh = message({ id: "9473000000000000002", content: "just now", timestamp: new Date(clock.now - 1000).toISOString(),
    observed_ms: clock.now - 5000 });
  await deliver(page, page.stream(), sseMessage(fresh));
  await page.settle();
  assert.ok(shownIds(page).includes(fresh.id), "the premise is the streamed message drawn");
  clock.now += 10000;
  await pollOnce(page);
  assert.ok(shownIds(page).includes(fresh.id), "a message in the unsettled band was dropped by its absence");
  void data;
});

// --- `#469 replication-model-tests`: the page against random interleavings --------------------------
//
// The replication model's §8 on the page, with the server's versions (`#473`). A seeded xorshift picks
// events: the service creates, edits and deletes messages; the stream delivers a deletion at once, late
// or never; and newest reads land fresh, or answered from a memory up to ten minutes old that says so
// (`observed_ms` earlier). After every step: no message's text goes back to an earlier version, none comes
// back once the page was told it was removed, and nothing is invented. At quiescence (a fresh read begun
// well past every change) the page holds exactly what the service holds. A failure names its seed and its
// steps, which replay exactly.

/** xorshift64*, as the server's model tests use, in BigInt. */
function xorshift(seed) {
  let state = BigInt(seed) * 0x9e3779b97f4a7c15n | 1n;
  const mask = (1n << 64n) - 1n;
  return (n) => {
    state ^= state >> 12n;
    state = (state ^ (state << 25n)) & mask;
    state ^= state >> 27n;
    return Number(((state * 0x2545f4914f6cdd1dn) & mask) % BigInt(Math.max(1, n)));
  };
}

/**
 * One seeded run of the page against the service model, with `script` as web/voice.js. Answers the
 * first property broken, with its seed and steps, or null.
 */
async function pageModelRun(seed, script) {
  {
    const pick = xorshift(seed);
    const log = [];
    const fail = (what) => { throw new Error(`seed ${seed}: ${what}\nsteps:\n  ${log.join("\n  ")}`); };
    const clock = { now: Date.parse("2026-10-23T12:00:00Z"), at() { return this.now; } };
    /** The service's history: every state, with when it began, for reads answered from older state. */
    const history = [];
    let readAsOf = null;
    const { page } = await freshMessagePage((p) => {
      const serve = p.timeline;
      p.timeline = async (path, options) => {
        const asOf = readAsOf === null ? clock.now : readAsOf;
        const state = [...history].reverse().find((entry) => entry.at <= asOf) || history[0];
        const live = p.messages;
        p.messages = state.messages;
        try {
          const answer = await serve(path, options);
          if (!answer || !answer.ok) return answer;
          const body = JSON.parse(await answer.text());
          const url = new URL(path, "http://fixture.test");
          body.observed_ms = asOf;
          if (!url.searchParams.get("before") && !url.searchParams.get("after")) body.settled_below_ms = asOf - 120000;
          for (const held of body.messages || []) held.observed_ms = asOf;
          return json(answer.status, body);
        } finally {
          p.messages = live;
        }
      };
    }, script);
    const record = () => history.push({ at: clock.now, messages: page.messages });
    record();
    /** By id, the versions the service gave it, and whether the page was told it was removed. */
    const versions = new Map();
    const removed = new Set();
    let next = 0;
    const versionOf = (text) => {
      const found = /^m(\d+) v(\d+)$/.exec(String(text));
      return found ? Number(found[2]) : null;
    };
    const ownedIds = () => [...versions.keys()];
    const highest = new Map();
    const check = () => {
      const kept = savedScopes(page)[scopeKey(CHANNEL.id)];
      for (const copy of (kept ? kept.messages : [])) {
        const id = String(copy.id);
        if (!versions.has(id)) continue;
        if (removed.has(id)) fail(`${id} came back after the page was told it was removed`);
        const version = versionOf(copy.content);
        if (version === null || !versions.get(id).includes(copy.content)) fail(`${id} holds text the service never gave it: ${copy.content}`);
        if (version < (highest.get(id) ?? -1)) fail(`${id} went back from v${highest.get(id)} to v${version}`);
        highest.set(id, Math.max(version, highest.get(id) ?? -1));
      }
    };
    for (let step = 0; step < 24; step += 1) {
      clock.now += 1000 + pick(20) * 1000;
      const live = ownedIds().filter((id) => page.messages.some((m) => m.id === id));
      const choice = pick(100);
      if (choice < 25 || live.length === 0) {
        next += 1;
        const id = `98000000000000${String(next).padStart(5, "0")}`;
        const text = `m${next} v0`;
        versions.set(id, [text]);
        page.messages = [...page.messages, message({ id, content: text, timestamp: new Date(clock.now).toISOString() })];
        record();
        log.push(`${clock.now} create ${id}`);
      } else if (choice < 50) {
        const id = live[pick(live.length)];
        const texts = versions.get(id);
        const text = `m${Number(id.slice(-5))} v${texts.length}`;
        texts.push(text);
        page.messages = page.messages.map((m) => (m.id === id
          ? { ...m, content: text, edited_timestamp: new Date(clock.now).toISOString() } : m));
        record();
        log.push(`${clock.now} edit ${id} to ${text}`);
      } else if (choice < 65) {
        const id = live[pick(live.length)];
        page.messages = page.messages.filter((m) => m.id !== id);
        record();
        log.push(`${clock.now} delete ${id}`);
        if (pick(2) === 0) {
          await deliver(page, page.stream(), sseDelete(`del-${id}-${step}`, CHANNEL.id, id, { observed_ms: clock.now }));
          await page.settle();
          removed.add(id);
          log.push(`${clock.now} stream delivers the deletion of ${id}`);
        }
      } else {
        const stale = pick(3) === 0 ? pick(10) * 60000 : 0;
        readAsOf = clock.now - stale;
        await reReadChannel(page);
        readAsOf = null;
        log.push(`${clock.now} read as of ${clock.now - stale}`);
      }
      check();
    }
    // Quiescence: a fresh read begun past every change and the band.
    clock.now += 600000;
    record();
    await reReadChannel(page);
    check();
    const kept = savedScopes(page)[scopeKey(CHANNEL.id)];
    const held = new Map((kept ? kept.messages : []).filter((m) => versions.has(String(m.id))).map((m) => [String(m.id), m.content]));
    const service = new Map(page.messages.filter((m) => versions.has(m.id)).map((m) => [m.id, m.content]));
    if (JSON.stringify([...held].sort()) !== JSON.stringify([...service].sort())) {
      fail(`did not converge:\n  page    ${JSON.stringify([...held].sort())}\n  service ${JSON.stringify([...service].sort())}`);
    }
  }
}

test("`#469 replication-model-tests`: the page agrees with the service under random interleavings of edits, deletions, stream removals and stale reads", async () => {
  for (let seed = 1; seed <= 12; seed += 1) {
    try {
      await pageModelRun(seed, SCRIPT);
    } catch (error) {
      assert.fail(error.message);
    }
  }
  // THE CONTROL: ordering copies by read-send order alone, some seed breaks a property.
  let caught = null;
  const ignored = await brokenModule([SERVER_VERSIONS_IGNORED]);
  for (let seed = 1; seed <= 12 && caught === null; seed += 1) {
    try {
      await pageModelRun(seed, ignored);
    } catch (error) {
      caught = error.message;
    }
  }
  assert.ok(caught !== null, "without the server's versions no seed broke a property, so this test cannot see them");
});

// --- `#611 timeline-read-failures` -----------------------------------------------------------------

test("`#611 timeline-read-failures`: a channel read the server fails ends its loading line and says so", async () => {
  // The owner, 2026-10-27: "My vibe talk app is stuck. It is stuck loading messages." A view no read
  // has covered shows the loading line while its read is out; when the server fails that read, the
  // line goes and the failure is on screen, rather than "Loading threads…" standing.
  const page = await threadPage("main");
  page.timeline = errorResponse(502, "chat_error", "the chat service is limiting how fast it can be read");
  const pending = page.el("channel-view-threads").click();
  await page.settle();
  await pending;
  await page.settle();
  assert.equal(page.el("channel-loading").hidden, true, "the loading line outlived the failed read");
  const said = `${freshness(page).hidden ? "" : freshness(page).textContent} ${page.el("error").textContent}`;
  assert.match(said, /Refresh failed|could not|failed/i, `nothing on screen said the read failed: ${said}`);
});

test("`#611 timeline-read-failures`: a page the server answered from its own copy lands, and the pill says it could not refresh", async () => {
  const page = await threadPage("main");
  const standard = page.timeline;
  // Older than two minutes by the page's clock: `#617 cold-load-first-paint` keeps a younger copy quiet.
  const asOf = new Date(page.clock() - 5 * 60_000).toISOString();
  page.timeline = async (path, options) => {
    const response = await standard(path, options);
    const { delta: _delta, ...newest } = JSON.parse(await response.text());
    // The server's copy lags: it lacks the oldest message on screen.
    const messages = (newest.messages || []).slice(1);
    return json(200, { ...newest, messages, as_of: asOf,
      refresh_failed: "The chat service is limiting how fast it can be read." });
  };
  const before = shownIds(page);
  assert.ok(before.length > 1, "the premise is a list of rows");
  await reReadChannel(page);
  assert.deepStrictEqual(shownIds(page), before, "a row the server's copy lacks was taken off the screen");
  assert.equal(page.el("channel-loading").hidden, true);
  assert.equal(freshness(page).hidden, false);
  assert.equal(freshness(page).getAttribute("data-state"), "failed");
  assert.match(freshness(page).textContent, /^Refresh failed · showing messages from \d{2}:\d{2}$/);

  // The next read the service answers says the rows are current again: armed for three seconds
  // on, not the next poll (`#617 cold-load-first-paint`).
  page.timeline = standard;
  assert.ok(page.expireTimers(3000) > 0, "no soon read was armed");
  await page.settle();
  assert.equal(freshness(page).getAttribute("data-state"), "fresh");
});

// --- `#614 timeline-read-failures` -----------------------------------------------------------------

for (const failed of [true, false]) {
  test(`\`#614 timeline-read-failures\`: a thread list ${failed ? "from the server's own copy keeps" : "the provider answered takes away"} a card it lacks`, async () => {
    const page = await threadPage("main");
    await page.el("channel-view-threads").click();
    await page.settle();
    const standard = page.timeline;
    assert.match(page.el("thread-list").text(), /Second discussion/, "the premise is two cards");
    page.timeline = async (path, options) => {
      const response = await standard(path, options);
      if (new URL(path, "http://fixture.test").searchParams.get("view") !== "threads") return response;
      const { delta: _delta, ...newest } = JSON.parse(await response.text());
      const threads = (newest.threads || []).filter((thread) => thread.title !== "Second discussion");
      return json(200, { ...newest, threads, has_more: false, next_before: null,
        ...(failed ? { refresh_failed: "The chat service is limiting how fast it can be read." } : {}) });
    };
    await reReadChannel(page);
    assert.match(page.el("thread-list").text(), /First discussion/);
    if (failed) {
      assert.match(page.el("thread-list").text(), /Second discussion/, "a card the server's copy lacks was taken away");
    } else {
      assert.doesNotMatch(page.el("thread-list").text(), /Second discussion/, "the negative control: a full list removes");
    }
  });
}

// --- `#650 cache-first-older-reads` ----------------------------------------------------------------

const COUNTS_SOON_MS = sourceConstant("COUNTS_SOON_MS");

/**
 * A channel of ten messages read on the timeline route, whose newest reads the server answers from its own
 * copy (`refresh_failed`), saying there is more (`stillMore`) as a server before `#650` did, or not, as one
 * that holds the view from its start does; a step back walks the provider and finds nothing older. The
 * reader at the top of the short list, where every redraw's scroll event asks for the step back. The reads
 * each of four polls made, as `reason:cursor`.
 */
async function pollsOverAShortCopy(stillMore) {
  const page = newPage();
  pagedTimeline(page, { steps: 1, size: 10 });
  await signIn(page);
  await showDiscord(page, []);
  const area = page.el("scroll-area");
  const standard = page.timeline;
  page.timeline = async (path, options) => {
    const url = new URL(path, "http://fixture.test");
    if (url.searchParams.get("before")) {
      return json(200, timelineAnswer({ view: url.searchParams.get("view"), messages: [], limit: 10,
        has_threads: true, has_more: false, next_before: null }));
    }
    const { delta: _delta, ...newest } = JSON.parse(await (await standard(path, options)).text());
    return json(200, { ...newest, has_more: stillMore, next_before: stillMore ? "vt-served.x" : null,
      as_of: new Date(page.clock() - 1000).toISOString(),
      refresh_failed: "Answered from this server's copy while the chat service is read." });
  };
  const polls = [];
  for (let i = 0; i < 4; i += 1) {
    const before = page.timelineCalls.length;
    // The poll, and the soon reads after a page from the copy (`#617 cold-load-first-paint`).
    for (const due of [DISCORD_POLL_MS, 3000, 10000]) {
      page.expireTimers(due);
      await page.settle();
      // A redraw in a real browser scrolls the list, and the reader is at its top.
      area.scrollTop = 0;
      await area.dispatch("scroll");
      await page.settle();
    }
    polls.push(page.timelineCalls.slice(before).map((call) => {
      const url = new URL(call, "http://fixture.test");
      return `${url.searchParams.get("reason")}:${url.searchParams.get("before") ? "before" : "newest"}`;
    }));
  }
  return polls;
}

test("`#650 cache-first-older-reads`: a short view the server's copy holds from its start is never stepped back from at the next poll", async () => {
  // Live after `#622`: one open page read a step back every forty-five seconds, each walking Slack for
  // 2.6 s to learn there was nothing older. The copy said there was more; the page at the top of its
  // short list stepped back after every poll's redraw, learned there was nothing, and the next poll's
  // copy said there was more again. A copy that says there is nothing older is believed.
  const polls = await pollsOverAShortCopy(false);
  assert.ok(polls.every((reads) => reads.length > 0 && reads.every((read) => read.endsWith(":newest"))),
    `a poll stepped back: ${JSON.stringify(polls)}`);

  // THE CONTROL, the server as it was: a step back after every poll.
  const control = await pollsOverAShortCopy(true);
  assert.ok(control.every((reads) => reads.includes("older:before")), JSON.stringify(control));
});

/**
 * Main with a root whose count outruns the store, so each read of Main past the interval reads All behind
 * it for its counts; the server answers those from its own copy while `fromCopy()` says so. The counts
 * reads it made, by the time of each.
 */
async function countsFromCopy(script, fromCopy) {
  const { page, server, held, quietReply } = await heldAllReads(script);
  const serve = page.timeline;
  const counted = [];
  page.timeline = async (path, options) => {
    const url = new URL(path, "http://fixture.test");
    if (url.searchParams.get("reason") !== "counts") return serve(path, options);
    counted.push(page.clock());
    const answer = server.answer(path);
    const body = JSON.parse(await (await answer).text());
    if (!fromCopy()) return json(200, body);
    // The copy says nothing of the second discussion: it leaves the counts outrunning what the page holds.
    const messages = (body.messages || []).filter((m) => !["203", "204", "250"].includes(String(m.id)));
    return json(200, { ...body, messages,
      refresh_failed: "Answered from this server's copy while the chat service is read." });
  };
  void held;
  page.setClock(page.clock() + COUNTS_ALL_READ_MS + 1000);
  quietReply();
  await pollOnce(page);
  for (let i = 0; i < 3; i += 1) await page.settle();
  return { page, counted };
}

test("`#650 cache-first-older-reads`: a counts read the server answered from its copy that leaves the counts outrunning is asked once more soon, and one it trusts ends the asking", async () => {
  let copy = true;
  const { page, counted } = await countsFromCopy(SCRIPT, () => copy);
  assert.equal(counted.length, 1, "fixture: All was not read for Main's counts");
  // The read the server made behind that answer has landed by the next one.
  copy = false;
  assert.ok(page.expireTimers(COUNTS_SOON_MS) > 0, "no soon counts read was armed");
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(counted.length, 2, "the counts were not asked again soon");
  page.expireTimers(COUNTS_SOON_MS);
  for (let i = 0; i < 3; i += 1) await page.settle();
  page.setClock(page.clock() + DISCORD_POLL_MS);
  await pollOnce(page);
  for (let i = 0; i < 3; i += 1) await page.settle();
  assert.equal(counted.length, 2, "a counts read the page trusts did not end the asking");

  // THE CONTROL, the page as it was: one counts read, then nothing for five minutes.
  const control = await countsFromCopy(brokenScript(
    "    else if (!soon) setTimeout(() => void readAllForCounts(channel, true).catch(() => {}), COUNTS_SOON_MS);\n", ""),
  () => true);
  control.page.expireTimers(COUNTS_SOON_MS);
  for (let i = 0; i < 3; i += 1) await control.page.settle();
  assert.equal(control.counted.length, 1, JSON.stringify(control.counted));
});

// The owner, 2026-10-29 (#624 voice-page-modularity): replies seen in Slack to a message posted moments
// earlier, not seen here. Every way they may come in, by stream with the thread record, by stream
// without one (left unplaced until a read places it), or by the poll's read alone, puts each reply in
// All and counts it on the root's Thread(N) in Main.
for (const how of ["stream", "bare-stream", "poll-only"]) {
  test(`replies (${how}) to a message that arrived live moments earlier are in All and counted on Main`, async () => {
    const { page } = await freshMessagePage();
    await pickThread(page, "flat");
    const thread = { id: "spaces/A/threads/live", root_message_id: "300", is_root: true, reply_count: 0, reply_count_exact: true };
    const root = message({ id: "300", content: "the owner's new question", author: "owner", author_id: "1000000000000000099",
      author_is_bot: false, thread });
    page.messages = [...page.messages, root];
    await deliver(page, page.stream(), sseMessage(root));
    await page.settle();
    for (const n of [1, 2, 3]) {
      page.setClock(page.clock() + 27000);
      const reply = message({ id: `30${n}`, content: `answer ${n}`, author: "bot", author_id: "1000000000000000088", author_is_bot: true,
        thread: { ...thread, is_root: false, reply_count: null, reply_count_exact: false } });
      page.messages = [...page.messages.map((m) => (m.id === "300" ? { ...m, thread: { ...thread, reply_count: n } } : m)), reply];
      if (how === "stream") await deliver(page, page.stream(), sseMessage(reply));
      if (how === "bare-stream") {
        const { thread: _thread, ...bare } = reply;
        await deliver(page, page.stream(), sseMessage(bare));
      }
      for (let i = 0; i < 3; i += 1) await page.settle();
    }
    page.setClock(page.clock() + DISCORD_POLL_MS);
    await pollOnce(page);
    for (let i = 0; i < 3; i += 1) await page.settle();
    assert.deepStrictEqual(shownIds(page).filter((id) => id.startsWith("30")), ["300", "301", "302", "303"], `${how}: All lost a reply`);
    await pickThread(page, "main");
    assert.ok(!shownIds(page).includes("301"), `${how}: a reply was drawn on Main`);
    assert.equal(tagOf(rowOf(page, "300")).text, "Thread(3)", `${how}: Main's root does not count its replies`);
  });
}
